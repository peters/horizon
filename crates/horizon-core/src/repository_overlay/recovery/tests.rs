use super::{ChangeRelation, ComparisonSide, RecoveryComparisonError as Error, compare_recovery, identity::Identities};
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        OverlayChange, OverlayContent, RepositoryOverlayPlan,
        bundle::{RepositoryOverlayBundle, VerifiedOverlayBlob, codec},
        namespace::{NamespaceEntry, ResolvedRepositoryOverlay, resolve_namespaces},
    },
};
use git2::{Index, IndexEntry, IndexTime, Oid, Repository, RepositoryInitOptions, Signature, Time};

mod bounds;
mod layers;

struct Fixture {
    repository: Repository,
    directory: tempfile::TempDir,
    commit: Oid,
}

impl Fixture {
    fn new(files: &[(&str, &[u8], u32)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut options = RepositoryInitOptions::new();
        options.no_reinit(true).external_template(false).initial_head("fixture");
        let repository = Repository::init_opts(directory.path(), &options).unwrap();
        let mut index = Index::new().unwrap();
        for (path, bytes, mode) in files {
            index
                .add(&IndexEntry {
                    ctime: IndexTime::new(0, 0),
                    mtime: IndexTime::new(0, 0),
                    dev: 0,
                    ino: 0,
                    mode: *mode,
                    uid: 0,
                    gid: 0,
                    file_size: u32::try_from(bytes.len()).unwrap(),
                    id: repository.blob(bytes).unwrap(),
                    flags: 0,
                    flags_extended: 0,
                    path: path.as_bytes().to_vec(),
                })
                .unwrap();
        }
        let tree = index.write_tree_to(&repository).unwrap();
        let signature = Signature::new("Fixture", "fixture@example.invalid", &Time::new(1, 0)).unwrap();
        let commit = repository
            .commit(
                None,
                &signature,
                &signature,
                "Synthetic",
                &repository.find_tree(tree).unwrap(),
                &[],
            )
            .unwrap();
        Self {
            repository,
            directory,
            commit,
        }
    }

    fn source(&self) -> GitSource {
        GitSource {
            repository: "synthetic/project".into(),
            commit: GitCommitSha::parse(self.commit.to_string()).unwrap(),
            branch: None,
        }
    }

    fn bundle(
        &self,
        index: Vec<OverlayChange>,
        working: Vec<OverlayChange>,
        blobs: Vec<VerifiedOverlayBlob>,
    ) -> RepositoryOverlayBundle {
        RepositoryOverlayBundle::new(
            RepositoryOverlayPlan::new(self.source(), index, working).unwrap(),
            blobs,
        )
        .unwrap()
    }

    fn resolve(
        &self,
        index: Vec<OverlayChange>,
        working: Vec<OverlayChange>,
        blobs: Vec<VerifiedOverlayBlob>,
    ) -> ResolvedRepositoryOverlay {
        resolve_namespaces(&self.repository, self.bundle(index, working, blobs)).unwrap()
    }
}

fn file(path: &str, bytes: &[u8], executable: bool) -> (OverlayChange, VerifiedOverlayBlob) {
    let blob = VerifiedOverlayBlob::new(bytes.to_vec()).unwrap();
    let change = OverlayChange::new(
        path.into(),
        OverlayContent::File {
            sha256: blob.sha256().clone(),
            bytes: bytes.len() as u64,
            executable,
        },
    )
    .unwrap();
    (change, blob)
}

fn remove(path: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Remove).unwrap()
}
fn link(path: &str, target: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Symlink { target: target.into() }).unwrap()
}

#[test]
fn comparison_borrows_originals_and_works_after_source_removal() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let (change, blob) = file("item", b"local", false);
    let local = fixture.resolve(vec![change], vec![], vec![blob]);
    let remote = fixture.resolve(vec![], vec![], vec![]);
    let original = [
        codec::encode(local.bundle()).unwrap(),
        codec::encode(remote.bundle()).unwrap(),
    ];
    let path = fixture.directory.path().to_owned();
    drop(fixture);
    assert!(!path.exists());
    let comparison = compare_recovery(&local, &remote, || false).unwrap();
    assert!(std::ptr::eq(comparison.local(), &raw const local));
    assert!(std::ptr::eq(comparison.remote(), &raw const remote));
    assert_eq!(comparison.index().paths()[0].relation, ChangeRelation::LocalOnly);
    assert_eq!(
        original,
        [
            codec::encode(local.bundle()).unwrap(),
            codec::encode(remote.bundle()).unwrap()
        ]
    );
    let debug = format!("{comparison:?} {:?}", comparison.index().paths()[0]);
    for private in ["item", "synthetic/project", "local", "base"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn exact_source_and_commit_mismatch_leave_bundles_available() {
    let fixture = Fixture::new(&[]);
    let local = fixture.resolve(vec![], vec![], vec![]);
    for variant in 0..3 {
        let other = Fixture::new(&[("item", b"different", 0o100_644)]);
        let mut source = fixture.source();
        match variant {
            0 => source.repository = "Synthetic/project".into(),
            1 => source.branch = Some("other-branch".into()),
            _ => source = other.source(),
        }
        let bundle =
            RepositoryOverlayBundle::new(RepositoryOverlayPlan::new(source, vec![], vec![]).unwrap(), vec![]).unwrap();
        let repository = if variant == 2 {
            &other.repository
        } else {
            &fixture.repository
        };
        let remote = resolve_namespaces(repository, bundle).unwrap();
        let original = codec::encode(remote.bundle()).unwrap();
        assert_eq!(
            compare_recovery(&local, &remote, || false).unwrap_err(),
            Error::SourceMismatch
        );
        assert_eq!(codec::encode(remote.bundle()).unwrap(), original);
        assert!(local.index().entries().next().is_none());
    }
}
