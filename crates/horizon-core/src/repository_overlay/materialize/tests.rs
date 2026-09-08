use super::*;

#[test]
fn invalid_request_and_early_cancellation_do_not_create_anything() {
    let fixture = tempfile::tempdir().unwrap();
    let missing = fixture.path().join("missing-private-marker");
    let digest = ArtifactDigest::sha256(b"synthetic");
    let mut request = MaterializationRequest {
        objects_directory: &missing,
        bundle_store: &missing,
        bundle_manifest: &digest,
        scratch_parent: &missing,
        destination: "valid",
    };
    let failure = materialize_repository(&request, || true).unwrap_err();
    assert!(matches!(
        failure.problem,
        MaterializationProblem::Source(SeedError::Cancelled)
    ));
    assert!(failure.source_metadata().is_none());
    for name in ["../escape", "", ".git", "two/names"] {
        request.destination = name;
        let failure = materialize_repository(&request, || false).unwrap_err();
        assert!(matches!(failure.problem, MaterializationProblem::InvalidRequest));
        assert!(failure.source_metadata().is_none());
        assert!(!format!("{failure:?} {failure}").contains("private-marker"));
    }
    assert!(!missing.exists());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_has_no_filesystem_effects() {
    let fixture = tempfile::tempdir().unwrap();
    let missing = fixture.path().join("missing");
    let digest = ArtifactDigest::sha256(b"synthetic");
    let request = MaterializationRequest {
        objects_directory: &missing,
        bundle_store: &missing,
        bundle_manifest: &digest,
        scratch_parent: &missing,
        destination: "valid",
    };
    let failure = materialize_repository(&request, || false).unwrap_err();
    assert!(matches!(
        failure.problem,
        MaterializationProblem::Source(SeedError::Unsupported)
    ));
    assert!(failure.source_metadata().is_none());
    assert!(!missing.exists());
}

#[cfg(target_os = "linux")]
mod supported {
    use super::*;
    use crate::{
        cloud_run::{GitCommitSha, GitSource},
        repository_overlay::{
            OverlayChange, OverlayContent, RepositoryOverlayPlan,
            bundle::{RepositoryOverlayBundle, VerifiedOverlayBlob, store::RepositoryBundleStore},
            checkout::{PrivateCheckoutError, publication::PublicationError},
        },
    };
    use git2::{Oid, Repository, Signature};
    use std::{cell::Cell, fs, os::unix::fs::PermissionsExt};

    struct Fixture {
        source: tempfile::TempDir,
        store: tempfile::TempDir,
        parent: tempfile::TempDir,
        objects: PathBuf,
        base: Oid,
        digest: ArtifactDigest,
    }

    fn private() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }

    impl Fixture {
        fn new(missing_base: bool) -> Self {
            let source = private();
            let repository = Repository::init(source.path()).unwrap();
            let blob = repository.blob(b"base\0raw\xff").unwrap();
            let mut tree = repository.treebuilder(None).unwrap();
            tree.insert("file", blob, 0o100_644).unwrap();
            let tree = repository.find_tree(tree.write().unwrap()).unwrap();
            let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
            let base = repository
                .commit(None, &signature, &signature, "base", &tree, &[])
                .unwrap();
            let staged = VerifiedOverlayBlob::new(b"staged\0raw".to_vec()).unwrap();
            let working = VerifiedOverlayBlob::new(b"working\0raw\r\n".to_vec()).unwrap();
            let change = |blob: &VerifiedOverlayBlob| {
                OverlayChange::new(
                    "file".into(),
                    OverlayContent::File {
                        sha256: blob.sha256().clone(),
                        bytes: blob.bytes().len() as u64,
                        executable: true,
                    },
                )
                .unwrap()
            };
            let plan = RepositoryOverlayPlan::new(
                GitSource {
                    repository: "synthetic/project".into(),
                    commit: GitCommitSha::parse(if missing_base { "a".repeat(40) } else { base.to_string() }).unwrap(),
                    branch: None,
                },
                [change(&staged)],
                [change(&working)],
            )
            .unwrap();
            let bundle = RepositoryOverlayBundle::new(plan, [staged, working]).unwrap();
            let store = private();
            let digest = RepositoryBundleStore::open(store.path()).unwrap().put(&bundle).unwrap();
            let objects = repository.path().join("objects");
            Self {
                source,
                store,
                parent: private(),
                objects,
                base,
                digest,
            }
        }

        fn request(&self) -> MaterializationRequest<'_> {
            MaterializationRequest {
                objects_directory: &self.objects,
                bundle_store: self.store.path(),
                bundle_manifest: &self.digest,
                scratch_parent: self.parent.path(),
                destination: "ready",
            }
        }
    }

    #[test]
    fn absent_bundle_source_and_cancelled_read_have_no_new_residues() {
        let fixture = Fixture::new(false);
        let missing = ArtifactDigest::sha256(b"missing");
        let mut request = fixture.request();
        request.bundle_manifest = &missing;
        let failure = materialize_repository(&request, || false).unwrap_err();
        assert!(matches!(
            failure.problem,
            MaterializationProblem::Bundle(BundleStoreError::Missing)
        ));
        assert!(failure.source_metadata().is_none());
        let calls = Cell::new(0);
        let failure = materialize_repository(&request, || {
            calls.set(calls.get() + 1);
            calls.get() > 1
        })
        .unwrap_err();
        assert_eq!(calls.get(), 2);
        assert!(matches!(
            failure.problem,
            MaterializationProblem::Source(SeedError::Cancelled)
        ));
        request.bundle_manifest = &fixture.digest;
        let absent = fixture.source.path().join("absent");
        request.objects_directory = &absent;
        let failure = materialize_repository(&request, || false).unwrap_err();
        assert!(matches!(failure.problem, MaterializationProblem::Source(_)));
        assert!(failure.source_metadata().is_none());
        assert_eq!(fs::read_dir(fixture.parent.path()).unwrap().count(), 0);
    }

    #[test]
    fn namespace_failure_retains_only_source_metadata_even_after_drop() {
        let fixture = Fixture::new(true);
        let failure = materialize_repository(&fixture.request(), || false).unwrap_err();
        assert!(matches!(failure.problem, MaterializationProblem::Namespace(_)));
        let metadata = failure.source_metadata().unwrap().to_owned();
        assert!(!format!("{failure:?} {failure}").contains(metadata.to_str().unwrap()));
        drop(failure);
        assert!(metadata.join("HEAD").exists());
        assert_eq!(fs::read_dir(fixture.parent.path()).unwrap().count(), 1);
        assert!(!fixture.parent.path().join("ready").exists());
    }

    #[test]
    fn preparation_cancellation_reports_both_retained_directories() {
        let fixture = Fixture::new(false);
        let failure = materialize_repository(&fixture.request(), || {
            fs::read_dir(fixture.parent.path()).unwrap().count() >= 2
        })
        .unwrap_err();
        let metadata = failure.source_metadata().unwrap().to_owned();
        let MaterializationProblem::Preparation(preparation) = &failure.problem else {
            panic!("unexpected failure: {failure}");
        };
        assert_eq!(preparation.reason, PrivateCheckoutError::Seed(SeedError::Cancelled));
        let residue = preparation.residue().unwrap().to_owned();
        assert_ne!(metadata, residue);
        drop(failure);
        assert!(metadata.exists() && residue.exists());
        assert_eq!(fs::read_dir(fixture.parent.path()).unwrap().count(), 2);
    }

    #[test]
    fn cancellation_after_actual_rename_retains_published_unsynchronized_receipt() {
        let fixture = Fixture::new(false);
        let destination = fixture.parent.path().join("ready");
        let failure = materialize_repository(&fixture.request(), || destination.exists()).unwrap_err();
        let metadata = failure.source_metadata().unwrap().to_owned();
        let MaterializationProblem::Publication(publication) = &failure.problem else {
            panic!("unexpected failure: {failure}");
        };
        if matches!(
            publication.as_ref(),
            PublicationFailure::Unpublished {
                reason: PublicationError::Unsupported,
                ..
            }
        ) {
            eprintln!("SKIP post-rename cancellation: unqualified filesystem");
            assert!(!destination.exists());
            return;
        }
        let PublicationFailure::PublishedUnsynchronized { reason, checkout } = publication.as_ref() else {
            panic!("wrong publication state: {failure}");
        };
        assert_eq!(*reason, PublicationError::Cancelled);
        assert_eq!(checkout.path(), destination);
        assert_eq!(checkout.base_commit(), fixture.base);
        assert_eq!(checkout.manifest_sha256(), &fixture.digest);
        drop(failure);
        assert!(metadata.join("HEAD").exists() && destination.join(".git/HEAD").exists());
        assert_eq!(fs::read_dir(fixture.parent.path()).unwrap().count(), 2);
    }

    #[test]
    fn publication_receipts_retain_exact_layers_and_never_replace_existing_data() {
        for existing in [false, true] {
            let fixture = Fixture::new(false);
            let destination = fixture.parent.path().join("ready");
            if existing {
                fs::write(&destination, b"sentinel").unwrap();
            }
            let result = materialize_repository(&fixture.request(), || false);
            let (metadata, checkout) = match &result {
                Ok(repository) => {
                    assert!(!existing);
                    assert_eq!(repository.checkout().path(), destination);
                    assert_eq!(repository.checkout().base_commit(), fixture.base);
                    assert_eq!(repository.checkout().manifest_sha256(), &fixture.digest);
                    (repository.source_metadata(), repository.checkout().path())
                }
                Err(failure) => {
                    let MaterializationProblem::Publication(publication) = &failure.problem else {
                        panic!("unexpected failure: {failure}");
                    };
                    let PublicationFailure::Unpublished { reason, checkout } = publication.as_ref() else {
                        panic!("unexpected publication state: {failure}");
                    };
                    // An unsupported CI filesystem is not successful durability proof.
                    assert!(
                        *reason == PublicationError::Unsupported
                            || (existing && *reason == PublicationError::DestinationExists)
                    );
                    if *reason == PublicationError::Unsupported {
                        eprintln!("SKIP synchronized publication: unqualified filesystem");
                    }
                    assert_eq!(checkout.base_commit(), fixture.base);
                    assert_eq!(checkout.manifest_sha256(), &fixture.digest);
                    (failure.source_metadata().unwrap(), checkout.path())
                }
            };
            assert_eq!(fs::read(checkout.join("file")).unwrap(), b"working\0raw\r\n");
            assert_ne!(
                fs::metadata(checkout.join("file")).unwrap().permissions().mode() & 0o111,
                0
            );
            let repository = Repository::open(checkout).unwrap();
            assert_eq!(repository.head().unwrap().target(), Some(fixture.base));
            let index = repository.index().unwrap();
            let entry = index.get_path(Path::new("file"), 0).unwrap();
            assert_eq!(entry.mode, 0o100_755);
            assert_eq!(repository.find_blob(entry.id).unwrap().content(), b"staged\0raw");
            let (metadata, checkout) = (metadata.to_owned(), checkout.to_owned());
            drop(result);
            assert!(metadata.join("HEAD").exists() && checkout.join(".git/HEAD").exists());
            if existing {
                assert_eq!(fs::read(destination).unwrap(), b"sentinel");
            }
        }
    }
}
