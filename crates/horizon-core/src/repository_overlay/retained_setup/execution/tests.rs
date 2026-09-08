use super::*;

#[test]
fn claim_errors_keep_execution_specific_classification_and_redacted_messages() {
    for (claim, boundary) in [
        (SetupClaimError::Unsupported, SetupBoundaryError::Unsupported),
        (SetupClaimError::UnsafeRoot, SetupBoundaryError::UnsafeRoot),
        (SetupClaimError::Read, SetupBoundaryError::ClaimRead),
        (SetupClaimError::InvalidIntent, SetupBoundaryError::InvalidClaim),
        (SetupClaimError::InvalidRecord, SetupBoundaryError::InvalidClaim),
        (SetupClaimError::Conflict, SetupBoundaryError::InvalidClaim),
        (SetupClaimError::Storage, SetupBoundaryError::Storage),
    ] {
        assert_eq!(SetupBoundaryError::from(claim), boundary);
        let error = SetupExecutionError::from(boundary);
        assert!(!format!("{error:?} {error}").contains("no execution grant was issued"));
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_execution_does_not_fall_back_to_materialization() {
    let grant = SetupGrant {
        intent: super::super::tests::intent(),
    };
    assert!(matches!(
        grant.materialize(|| false),
        Err(SetupExecutionError::Boundary(SetupBoundaryError::Unsupported))
    ));
}

#[cfg(target_os = "linux")]
mod supported {
    use super::*;
    use crate::{
        cloud_run::ArtifactDigest,
        repository_overlay::{
            materialize::{MaterializationProblem, tests::supported::Fixture},
            retained_setup::{RetainedSetup, SCRATCH_NAME, SetupAdmission, SetupIntent, SetupObservation, codec},
        },
    };
    use git2::Repository;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    fn intent(fixture: &Fixture) -> SetupIntent {
        let request = fixture.request();
        SetupIntent::new(
            "workspace_1".into(),
            request.objects_directory.to_owned(),
            request.bundle_store.to_owned(),
            request.bundle_manifest.clone(),
            request.destination.into(),
        )
        .unwrap()
    }

    fn store(path: &Path) -> Option<RetainedSetup> {
        match RetainedSetup::open(path) {
            Err(SetupClaimError::Unsupported) => {
                eprintln!("SKIP real execution: requires qualified journaled ext4");
                None
            }
            other => Some(other.unwrap()),
        }
    }

    fn grant(store: &RetainedSetup, expected: &SetupIntent) -> SetupGrant {
        let SetupAdmission::Fresh(grant) = store.admit(expected.clone()).unwrap() else {
            panic!("fresh grant");
        };
        grant
    }

    fn assert_layers(path: &Path) {
        assert_eq!(fs::read(path.join("file")).unwrap(), b"working\0raw\r\n");
        assert_ne!(fs::metadata(path.join("file")).unwrap().permissions().mode() & 0o111, 0);
        let repository = Repository::open(path).unwrap();
        let index = repository.index().unwrap();
        let entry = index.get_path(Path::new("file"), 0).unwrap();
        assert_eq!(entry.mode, 0o100_755);
        assert_eq!(repository.find_blob(entry.id).unwrap().content(), b"staged\0raw");
    }

    #[test]
    fn qualified_consumption_retains_exact_layers_and_process_reopen_cannot_replay() {
        let fixture = Fixture::new(false);
        let root = fixture.request().scratch_parent.to_owned();
        let Some(store) = store(&root) else {
            return;
        };
        let expected = intent(&fixture);
        let grant = grant(&store, &expected);
        drop(store);
        let result = grant.materialize(|| false).unwrap();
        let checkout = root.join(SCRATCH_NAME).join("ready");
        assert_eq!(result.checkout().path(), checkout);
        assert_eq!(result.checkout().base_commit(), fixture.base);
        assert_eq!(result.checkout().manifest_sha256(), &expected.bundle_manifest);
        assert_eq!(
            result.source_metadata().parent(),
            Some(root.join(SCRATCH_NAME).as_path())
        );
        assert_layers(&checkout);
        let metadata = result.source_metadata().to_owned();
        drop(result);
        assert!(metadata.join("HEAD").exists());
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repository_overlay::retained_setup::execution::tests::supported::reopen_child",
                "--nocapture",
            ])
            .env("HORIZON_SETUP_MATERIALIZATION_FIXTURE", &root)
            .output()
            .unwrap();
        assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stderr));
        assert!(String::from_utf8_lossy(&child.stdout).contains("MATERIALIZATION_REOPEN_NO_REPLAY"));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert_eq!(fs::read_dir(root.join(SCRATCH_NAME)).unwrap().count(), 2);
        assert_layers(&checkout);
    }

    #[test]
    fn reopen_child() {
        let Some(root) = std::env::var_os("HORIZON_SETUP_MATERIALIZATION_FIXTURE") else {
            return;
        };
        let root = Path::new(&root);
        let expected = codec::decode(&fs::read(root.join("setup-claim.json")).unwrap()).unwrap();
        let store = RetainedSetup::open(root).unwrap();
        assert_eq!(store.observe(&expected), Ok(SetupObservation::ClaimedUnknown));
        assert!(matches!(store.admit(expected), Ok(SetupAdmission::Existing)));
        assert_layers(&root.join(SCRATCH_NAME).join("ready"));
        println!("MATERIALIZATION_REOPEN_NO_REPLAY");
    }

    #[test]
    fn cancellation_before_and_after_scratch_creation_consumes_without_replay() {
        for after_create in [false, true] {
            let fixture = Fixture::new(false);
            let root = fixture.request().scratch_parent;
            let Some(store) = store(root) else {
                return;
            };
            let expected = intent(&fixture);
            let scratch = root.join(SCRATCH_NAME);
            let result = grant(&store, &expected).materialize(|| !after_create || scratch.exists());
            assert!(matches!(
                result,
                Err(SetupExecutionError::Boundary(SetupBoundaryError::Cancelled))
            ));
            assert_eq!(scratch.exists(), after_create);
            if after_create {
                assert_eq!(fs::read_dir(scratch).unwrap().count(), 0);
            }
            assert!(matches!(store.admit(expected), Ok(SetupAdmission::Existing)));
        }
    }

    #[test]
    fn missing_bundle_and_namespace_failure_keep_typed_receipts_and_residues() {
        for missing_base in [false, true] {
            let fixture = Fixture::new(missing_base);
            let root = fixture.request().scratch_parent;
            let Some(store) = store(root) else {
                return;
            };
            let mut expected = intent(&fixture);
            if !missing_base {
                expected.bundle_manifest = ArtifactDigest::sha256(b"missing");
            }
            let result = grant(&store, &expected).materialize(|| false);
            let Err(SetupExecutionError::Materialization(failure)) = result else {
                panic!("component failure");
            };
            if missing_base {
                assert!(matches!(failure.problem, MaterializationProblem::Namespace(_)));
            } else {
                assert!(matches!(failure.problem, MaterializationProblem::Bundle(_)));
            }
            let metadata = failure.source_metadata().map(Path::to_owned);
            assert_eq!(metadata.is_some(), missing_base);
            assert!(!format!("{failure:?} {failure}").contains(root.to_str().unwrap()));
            drop(failure);
            if let Some(metadata) = metadata {
                assert!(metadata.join("HEAD").exists());
            }
            assert_eq!(
                fs::read_dir(root.join(SCRATCH_NAME)).unwrap().count(),
                usize::from(missing_base)
            );
            assert!(matches!(store.admit(expected), Ok(SetupAdmission::Existing)));
        }
    }

    #[test]
    fn changed_claim_root_and_preexisting_scratch_never_start_materialization() {
        for kind in 0..5 {
            let fixture = Fixture::new(false);
            let root = fixture.request().scratch_parent;
            let Some(store) = store(root) else {
                return;
            };
            let expected = intent(&fixture);
            let grant = grant(&store, &expected);
            let claim = root.join("setup-claim.json");
            let scratch = root.join(SCRATCH_NAME);
            let error = match kind {
                0 => {
                    fs::remove_file(&claim).unwrap();
                    SetupBoundaryError::InvalidClaim
                }
                1 => {
                    fs::write(&claim, b"invalid").unwrap();
                    SetupBoundaryError::InvalidClaim
                }
                2 => {
                    fs::set_permissions(&claim, fs::Permissions::from_mode(0o644)).unwrap();
                    SetupBoundaryError::ClaimRead
                }
                3 => {
                    fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
                    SetupBoundaryError::UnsafeRoot
                }
                _ => {
                    fs::create_dir(&scratch).unwrap();
                    SetupBoundaryError::ExistingScratch
                }
            };
            let result = grant.materialize(|| false);
            assert!(matches!(result, Err(SetupExecutionError::Boundary(actual)) if actual == error));
            assert_eq!(scratch.exists(), kind == 4);
            if kind == 4 {
                assert_eq!(fs::read_dir(scratch).unwrap().count(), 0);
            }
        }
    }
}
