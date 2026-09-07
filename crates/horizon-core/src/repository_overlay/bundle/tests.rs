use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::OverlayChange,
};

fn source() -> GitSource {
    GitSource {
        repository: "team/repo".into(),
        commit: GitCommitSha::parse("a".repeat(40)).expect("commit"),
        branch: Some("feature/work".into()),
    }
}

fn file(path: &str, bytes: &[u8], executable: bool) -> OverlayChange {
    described_file(path, ArtifactDigest::sha256(bytes), bytes.len(), executable)
}

fn described_file(path: &str, sha256: ArtifactDigest, bytes: usize, executable: bool) -> OverlayChange {
    OverlayChange::new(
        path.into(),
        OverlayContent::File {
            sha256,
            bytes: u64::try_from(bytes).expect("size"),
            executable,
        },
    )
    .expect("file")
}

fn remove(path: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Remove).expect("removal")
}

fn link(path: &str, target: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Symlink { target: target.into() }).expect("link")
}

fn plan(index: Vec<OverlayChange>, working_tree: Vec<OverlayChange>) -> RepositoryOverlayPlan {
    RepositoryOverlayPlan::new(source(), index, working_tree).expect("plan")
}

fn blob(bytes: &[u8]) -> VerifiedOverlayBlob {
    VerifiedOverlayBlob::new(bytes.to_vec()).expect("blob")
}

#[test]
fn staged_working_and_untracked_bytes_stay_distinct_and_literal() {
    let index = b"version https://git-lfs.github.com/spec/v1\noid sha256:synthetic\nsize 4\n";
    let working = b"\0\xff\x80\n";
    let untracked = b"local-new-file\n";
    let plan = plan(
        vec![file("asset", index, false), remove("old-name")],
        vec![
            file("asset", working, true),
            file("new-name", untracked, false),
            link("alias", "asset"),
        ],
    );
    let bundle =
        RepositoryOverlayBundle::new(plan.clone(), [blob(working), blob(index), blob(untracked)]).expect("bundle");
    assert_eq!(bundle.plan(), &plan);
    for bytes in [index.as_slice(), working.as_slice(), untracked.as_slice()] {
        assert_eq!(bundle.blob(&ArtifactDigest::sha256(bytes)), Some(bytes));
    }
    assert_eq!(bundle.blobs().len(), 3);
    assert_eq!(bundle.file_bytes(), index.len() + working.len() + untracked.len());
    assert!(bundle.blob(&ArtifactDigest::sha256(b"not selected")).is_none());
}

#[test]
fn shared_content_requires_one_blob_across_paths_and_layers() {
    let plan = plan(
        vec![file("a", b"same", false)],
        vec![file("a", b"same", true), file("b", b"same", false)],
    );
    let bundle = RepositoryOverlayBundle::new(plan, [blob(b"same")]).expect("bundle");
    assert_eq!(bundle.file_bytes(), 4);
    assert_eq!(bundle.plan().content_bytes(), 12);
    assert_eq!(bundle.blobs().len(), 1);
    assert_eq!(bundle.blobs().next().expect("blob").bytes(), b"same");
}

#[test]
fn empty_files_and_metadata_only_bundles_are_complete() {
    for plan in [
        plan(vec![], vec![]),
        plan(vec![remove("gone")], vec![link("link", "destination")]),
    ] {
        let bundle = RepositoryOverlayBundle::new(plan, []).expect("metadata bundle");
        assert_eq!(bundle.file_bytes(), 0);
        assert_eq!(bundle.blobs().len(), 0);
    }
    let plan = plan(vec![file("empty", b"", false)], vec![]);
    assert_eq!(
        RepositoryOverlayBundle::new(plan.clone(), []),
        Err(OverlayBundleError::MissingBlob)
    );
    let bundle = RepositoryOverlayBundle::new(plan, [blob(b"")]).expect("empty file");
    assert_eq!(bundle.file_bytes(), 0);
    assert_eq!(bundle.blobs().len(), 1);
}

#[test]
fn missing_unplanned_substituted_and_duplicate_payloads_fail() {
    let plan = plan(vec![file("selected", b"expected", false)], vec![]);
    assert_eq!(
        RepositoryOverlayBundle::new(plan.clone(), []),
        Err(OverlayBundleError::MissingBlob)
    );
    assert_eq!(
        RepositoryOverlayBundle::new(plan.clone(), [blob(b"different")]),
        Err(OverlayBundleError::UnexpectedBlob)
    );
    assert_eq!(
        RepositoryOverlayBundle::new(plan.clone(), [blob(b"expected"), blob(b"extra")]),
        Err(OverlayBundleError::UnexpectedBlob)
    );
    assert_eq!(
        RepositoryOverlayBundle::new(plan, [blob(b"expected"), blob(b"expected")]),
        Err(OverlayBundleError::DuplicateBlob)
    );
}

#[test]
fn declared_lengths_must_match_hash_verified_payloads() {
    let plan = plan(
        vec![described_file("wrong-size", ArtifactDigest::sha256(b"abc"), 2, false)],
        vec![],
    );
    assert_eq!(
        RepositoryOverlayBundle::new(plan, [blob(b"abc")]),
        Err(OverlayBundleError::SizeMismatch)
    );
}

#[test]
fn conflicting_sizes_for_one_digest_fail_before_consuming_payloads() {
    let plan = plan(
        vec![file("a", b"abc", false)],
        vec![described_file("b", ArtifactDigest::sha256(b"abc"), 2, false)],
    );
    let contents = std::iter::once_with(|| panic!("invalid metadata must not consume payloads"));
    assert_eq!(
        RepositoryOverlayBundle::new(plan, contents),
        Err(OverlayBundleError::SizeMismatch)
    );
}

#[test]
fn declared_payload_limits_fail_before_consuming_payloads() {
    let oversized_file = plan(
        vec![described_file(
            "huge",
            ArtifactDigest::sha256(b"a"),
            MAX_READ_BYTES + 1,
            false,
        )],
        vec![],
    );
    let oversized_bundle = plan(
        vec![
            described_file("a", ArtifactDigest::sha256(b"a"), MAX_READ_BYTES, false),
            described_file("b", ArtifactDigest::sha256(b"b"), MAX_READ_BYTES, false),
            described_file("c", ArtifactDigest::sha256(b"c"), 1, false),
        ],
        vec![],
    );
    for (plan, expected) in [
        (oversized_file, OverlayBundleError::FileLimit),
        (oversized_bundle, OverlayBundleError::BundleLimit),
    ] {
        let contents = std::iter::once_with(|| panic!("over-budget metadata must not consume payloads"));
        assert_eq!(RepositoryOverlayBundle::new(plan, contents), Err(expected));
    }
}

#[test]
fn exact_aggregate_limit_is_admitted_and_shared_references_are_not_double_charged() {
    let plan = plan(
        vec![
            described_file("a", ArtifactDigest::sha256(b"a"), MAX_READ_BYTES, false),
            described_file("b", ArtifactDigest::sha256(b"b"), MAX_READ_BYTES, false),
        ],
        vec![described_file("a", ArtifactDigest::sha256(b"a"), MAX_READ_BYTES, true)],
    );
    assert_eq!(RequiredBlobs::new(&plan).expect("budget").bytes, MAX_BUNDLE_BYTES);
    assert_eq!(
        RepositoryOverlayBundle::new(plan, []),
        Err(OverlayBundleError::MissingBlob)
    );
}

#[test]
fn blob_does_not_retain_unbounded_spare_allocation_capacity() {
    for content in [b"abc".as_slice(), b"".as_slice()] {
        let mut bytes = Vec::with_capacity(4096);
        bytes.extend_from_slice(content);
        let blob = VerifiedOverlayBlob::new(bytes).expect("bounded storage");
        assert_eq!(blob.bytes(), content);
        assert_eq!(blob.sha256(), &ArtifactDigest::sha256(content));
        assert_eq!(blob.bytes.into_vec().capacity(), content.len());
    }
}

#[test]
fn actual_blob_limit_includes_the_exact_boundary() {
    assert!(VerifiedOverlayBlob::new(vec![0; MAX_READ_BYTES]).is_ok());
    assert_eq!(
        VerifiedOverlayBlob::new(vec![0; MAX_READ_BYTES + 1]),
        Err(OverlayBundleError::FileLimit)
    );
}

#[test]
fn input_order_does_not_change_the_immutable_bundle_or_fingerprint() {
    let first = plan(
        vec![file("z", b"z", true), file("a", b"a", false)],
        vec![link("link", "z"), remove("old")],
    );
    let second = plan(
        vec![file("a", b"a", false), file("z", b"z", true)],
        vec![remove("old"), link("link", "z")],
    );
    let first = RepositoryOverlayBundle::new(first, [blob(b"z"), blob(b"a")]).expect("first");
    let second = RepositoryOverlayBundle::new(second, [blob(b"a"), blob(b"z")]).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.manifest_sha256(), second.manifest_sha256());
}

#[test]
fn versioned_fingerprint_has_a_fixed_canonical_encoding_and_independent_digest() {
    let plan = plan(
        vec![file("staged.txt", b"abc", true), remove("gone.txt")],
        vec![file("staged.txt", b"", false), link("link", "staged.txt")],
    );
    let expected = concat!(
        r#"{"domain":"horizon.repository-overlay","version":1,"repository":"team/repo","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","branch":"feature/work","index":["#,
        r#"{"path":"gone.txt","kind":"remove"},{"path":"staged.txt","kind":"file","sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","bytes":3,"executable":true}],"working_tree":["#,
        r#"{"path":"link","kind":"symlink","target":"staged.txt"},{"path":"staged.txt","kind":"file","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","bytes":0,"executable":false}]}"#,
    );
    assert_eq!(fingerprint::encode(&plan).expect("encoding"), expected.as_bytes());
    let bundle = RepositoryOverlayBundle::new(plan, [blob(b""), blob(b"abc")]).expect("bundle");
    assert_eq!(
        bundle.manifest_sha256().as_str(),
        "8227e6d7087c20ddd446055fa52373082d3ef1bea3b514d8ffdc5af169955daf"
    );
}

#[test]
fn fingerprint_encoding_preserves_literal_quotes_spaces_and_unicode() {
    let path = "nested/quoted \" ø.txt";
    let plan = plan(vec![file(path, b"data", false)], vec![link("alias", path)]);
    let encoded: serde_json::Value =
        serde_json::from_slice(&fingerprint::encode(&plan).expect("encoding")).expect("json");
    assert_eq!(encoded["index"][0]["path"], path);
    assert_eq!(encoded["working_tree"][0]["target"], path);
}

#[test]
fn fingerprint_binds_source_exact_base_branch_and_each_layer_semantic() {
    let original = plan(
        vec![file("file", b"abc", false)],
        vec![link("alias", "file"), remove("gone")],
    );
    let digest = fingerprint::digest(&original).expect("digest");
    let mut variants = vec![
        plan(
            vec![],
            vec![file("file", b"abc", false), link("alias", "file"), remove("gone")],
        ),
        plan(
            vec![file("renamed", b"abc", false)],
            vec![link("alias", "file"), remove("gone")],
        ),
        plan(
            vec![file("file", b"xyz", false)],
            vec![link("alias", "file"), remove("gone")],
        ),
        plan(
            vec![file("file", b"abc", true)],
            vec![link("alias", "file"), remove("gone")],
        ),
        plan(
            vec![file("file", b"abc", false)],
            vec![link("alias", "other"), remove("gone")],
        ),
        plan(vec![file("file", b"abc", false)], vec![link("alias", "file")]),
        plan(
            vec![described_file("file", ArtifactDigest::sha256(b"abc"), 4, false)],
            vec![link("alias", "file"), remove("gone")],
        ),
    ];
    for changed_source in [
        GitSource {
            repository: "other/repo".into(),
            ..source()
        },
        GitSource {
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            ..source()
        },
        GitSource {
            branch: None,
            ..source()
        },
        GitSource {
            branch: Some("different".into()),
            ..source()
        },
    ] {
        variants.push(
            RepositoryOverlayPlan::new(
                changed_source,
                original.index().to_vec(),
                original.working_tree().to_vec(),
            )
            .expect("variant"),
        );
    }
    for variant in variants {
        assert_ne!(fingerprint::digest(&variant).expect("digest"), digest);
    }
}

#[test]
fn diagnostics_do_not_include_source_paths_digests_or_contents() {
    let payload = blob(b"private-content");
    let digest = payload.sha256().as_str().to_owned();
    let debug = format!("{payload:?}");
    let bundle = RepositoryOverlayBundle::new(
        plan(
            vec![file("private-name", b"private-content", false)],
            vec![link("private-link", "private-target")],
        ),
        [payload],
    )
    .expect("bundle");
    let debug = format!("{debug} {bundle:?}");
    for private in [
        "private-content",
        "private-name",
        "private-link",
        "private-target",
        source().repository.as_str(),
        digest.as_str(),
    ] {
        assert!(!debug.contains(private));
    }
}
