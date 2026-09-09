use super::*;
use crate::repository_overlay::{
    namespace::resolve_namespaces_from_source,
    seed::{GitObjectInspector, GitObjectMetadata, GitObjectSource, GitObjectStream, SeedError},
};
use std::{cell::Cell, io::Cursor};

#[test]
fn canonical_base_overlay_identity_is_cached_and_checks_mode() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let (change, blob) = file("item", b"base", false);
    let local = fixture.resolve(vec![change], vec![], vec![blob]);
    let remote = fixture.resolve(vec![], vec![], vec![]);
    let mut identities = Identities::new(&|| false);
    for _ in 0..1_000 {
        assert!(
            identities
                .equal(
                    local.index().entry("item"),
                    local.bundle(),
                    remote.index().entry("item"),
                    remote.bundle()
                )
                .unwrap()
        );
    }
    assert_eq!(identities.hashes, 1);
    let report = compare_recovery(&local, &remote, || false).unwrap();
    assert!(report.index().paths().is_empty());
    assert!(report.working_tree().paths().is_empty());
}

#[test]
fn cancellation_before_after_hash_and_during_traversal_preserves_both_inputs() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let (change, blob) = file("item", b"base", false);
    let local = fixture.resolve(vec![change], vec![], vec![blob]);
    let remote = fixture.resolve(vec![], vec![], vec![]);
    let original = codec::encode(local.bundle()).unwrap();
    for threshold in [0, 2, 10] {
        let calls = Cell::new(0);
        assert_eq!(
            compare_recovery(&local, &remote, || {
                let count = calls.replace(calls.get() + 1);
                count >= threshold
            })
            .unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(codec::encode(local.bundle()).unwrap(), original);
    }
    let calls = Cell::new(0);
    let cancelled = || calls.replace(calls.get() + 1) > 0;
    let mut identities = Identities::new(&cancelled);
    assert_eq!(
        identities.equal(
            local.index().entry("item"),
            local.bundle(),
            remote.index().entry("item"),
            remote.bundle()
        ),
        Err(Error::Cancelled)
    );
    assert_eq!(identities.hashes, 1);
    assert!(remote.bundle().blobs().next().is_none());
}

#[test]
fn shared_deep_prefixes_use_bounded_scans_not_all_prefixes_or_pairs() {
    let fixture = Fixture::new(&[]);
    let prefix = "d/".repeat(1_000);
    let make = |start| {
        let blob = VerifiedOverlayBlob::new(b"same".to_vec()).unwrap();
        let changes = (start..start + 1_000)
            .map(|n| {
                OverlayChange::new(
                    format!("{prefix}{n:04}"),
                    OverlayContent::File {
                        sha256: blob.sha256().clone(),
                        bytes: 4,
                        executable: false,
                    },
                )
                .unwrap()
            })
            .collect();
        fixture.resolve(changes, vec![], vec![blob])
    };
    let local = make(0);
    let remote = make(1_000);
    let calls = Cell::new(0);
    let report = compare_recovery(&local, &remote, || {
        calls.set(calls.get() + 1);
        false
    })
    .unwrap();
    assert_eq!(report.index().paths().len(), 2_000);
    assert!(report.working_tree().structural_conflicts().is_empty());
    assert!(calls.get() < 20_000, "{} traversal checks", calls.get());
}

struct HeaderSource<'a> {
    repository: &'a Repository,
    length: u64,
    opened: usize,
}

impl GitObjectSource for HeaderSource<'_> {
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        self.opened += 1;
        let database = self.repository.odb().map_err(|_| SeedError::Source)?;
        let object = database.read(object).map_err(|_| SeedError::Source)?;
        Ok(GitObjectStream {
            kind: object.kind(),
            bytes: object.len() as u64,
            reader: Box::new(Cursor::new(object.data().to_vec())),
        })
    }
}

impl GitObjectInspector for HeaderSource<'_> {
    fn inspect(&mut self, object: Oid) -> Result<GitObjectMetadata, SeedError> {
        let (_, kind) = self
            .repository
            .odb()
            .and_then(|database| database.read_header(object))
            .map_err(|_| SeedError::Source)?;
        Ok(GitObjectMetadata {
            kind,
            bytes: self.length,
        })
    }
}

#[test]
fn large_base_references_are_not_read_and_inconsistent_base_headers_reject() {
    // Synthetic header claims only; this does not verify a large blob's payload.
    let fixture = Fixture::new(&[("large", b"unread", 0o100_644)]);
    let mut source = HeaderSource {
        repository: &fixture.repository,
        length: 65 * 1024 * 1024 + 1,
        opened: 0,
    };
    let local = resolve_namespaces_from_source(&mut source, fixture.bundle(vec![], vec![], vec![]), || false).unwrap();
    let remote = resolve_namespaces_from_source(&mut source, fixture.bundle(vec![], vec![], vec![]), || false).unwrap();
    assert_eq!(source.opened, 4, "only commit and tree payloads per resolver");
    assert!(
        compare_recovery(&local, &remote, || false)
            .unwrap()
            .index()
            .paths()
            .is_empty()
    );
    assert_eq!(source.opened, 4);
    source.length += 1;
    let inconsistent =
        resolve_namespaces_from_source(&mut source, fixture.bundle(vec![], vec![], vec![]), || false).unwrap();
    assert_eq!(
        compare_recovery(&local, &inconsistent, || false).unwrap_err(),
        Error::BaseMismatch
    );
}
