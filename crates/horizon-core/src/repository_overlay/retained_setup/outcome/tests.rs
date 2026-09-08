use super::super::{SCRATCH_NAME, tests::intent};
use super::*;
use SetupCompletionState as State;
use serde_json::json;
use std::path::Path;

pub(in super::super) fn snapshot(root: &Path, intent: &SetupIntent, state: State) -> SetupCompletion {
    let scratch = root.join(SCRATCH_NAME);
    let published = state == State::Published;
    let rejected = state == State::Rejected;
    let checkout_name = if matches!(state, State::Published | State::PublishedUnsynchronized) {
        &intent.destination
    } else {
        "stage"
    };
    SetupCompletion {
        data: snapshot::CompletionData {
            state,
            reason: (!published).then(|| "synthetic redacted failure".into()),
            source_metadata: (!rejected).then(|| scratch.join("metadata")),
            checkout: (!rejected).then(|| scratch.join(checkout_name)),
            possible_destination: (state == State::RenameUnconfirmed).then(|| scratch.join(&intent.destination)),
            base_commit: (!rejected).then(|| "a".repeat(40)),
            bundle_manifest: (!rejected).then(|| intent.bundle_manifest.clone()),
        },
    }
}

#[test]
fn canonical_states_round_trip_without_disclosing_paths_or_confusing_publication() {
    let root = std::env::temp_dir();
    for (state, destination) in [
        (State::Rejected, "repository"),
        (State::Unpublished, "repository"),
        (State::Unpublished, "stage"),
        (State::Unpublished, "metadata"),
        (State::Published, "repository"),
        (State::PublishedUnsynchronized, "repository"),
        (State::RenameUnconfirmed, "repository"),
        (State::RenameUnconfirmed, "metadata"),
    ] {
        let mut intent = intent();
        intent.destination = destination.into();
        let expected = snapshot(&root, &intent, state);
        let bytes = codec::encode(&root, &intent, &expected).unwrap();
        assert_eq!(codec::decode(&root, &intent, &bytes).unwrap(), expected);
        assert!(!format!("{expected:?}").contains("metadata"));
        let mut changed = intent.clone();
        changed.workspace_local_id.push('2');
        assert_eq!(
            codec::decode(&root, &changed, &bytes),
            Err(SetupRecordError::InvalidRecord)
        );
    }
}

#[test]
fn corrupt_noncanonical_and_inconsistent_records_are_never_completion() {
    let root = std::env::temp_dir();
    let intent = intent();
    let valid = codec::encode(&root, &intent, &snapshot(&root, &intent, State::Published)).unwrap();
    let text = String::from_utf8(valid.clone()).unwrap();
    for bad in [
        String::new(),
        "{}".into(),
        format!("{text}\n"),
        text.replace("\"version\":1", "\"version\":2"),
        text.replace("\"version\":1", "\"version\":1,\"version\":1"),
        text.replace("\"version\":1", "\"version\":1,\"extra\":0"),
        text[..text.len() - 1].into(),
        "x".repeat(codec::MAX_BYTES + 1),
    ] {
        assert_eq!(
            codec::decode(&root, &intent, bad.as_bytes()),
            Err(SetupRecordError::InvalidRecord)
        );
    }
    for (field, replacement) in [
        ("state", json!("unknown")),
        ("reason", json!("false success")),
        ("source_metadata", json!(null)),
        ("base_commit", json!("invalid")),
        ("base_commit", json!("A".repeat(40))),
        ("bundle_manifest", json!("b".repeat(64))),
        ("checkout", json!(root.join("outside"))),
        ("possible_destination", json!(root.join(SCRATCH_NAME).join("other"))),
        ("extra", json!(true)),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&valid).unwrap();
        value["completion"][field] = replacement;
        if let Ok(data) = serde_json::from_value::<snapshot::CompletionData>(value["completion"].clone()) {
            assert_eq!(
                codec::encode(&root, &intent, &SetupCompletion { data }),
                Err(SetupRecordError::InvalidRecord)
            );
        }
        assert_eq!(
            codec::decode(&root, &intent, &serde_json::to_vec(&value).unwrap()),
            Err(SetupRecordError::InvalidRecord)
        );
    }
    let mut largest = snapshot(&root, &intent, State::RenameUnconfirmed);
    largest.data.reason = Some("x".repeat(1025));
    assert_eq!(
        codec::encode(&root, &intent, &largest),
        Err(SetupRecordError::InvalidRecord)
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_cannot_observe_or_record() {
    assert_eq!(
        RetainedSetup {}.completion(&intent()),
        Err(SetupBoundaryError::Unsupported.into())
    );
    let failure = SetupGrant { intent: intent() }
        .materialize_recorded(|| false)
        .unwrap_err();
    assert_eq!(failure.problem, SetupBoundaryError::Unsupported.into());
    assert!(failure.execution().is_none());
}

#[test]
fn maximum_escaped_retained_paths_fit_the_bounded_record() {
    // Serialization-only proof; no filesystem node with this component is created.
    let root = std::env::temp_dir().join("\u{1}".repeat(4000));
    let intent = intent();
    let mut value = snapshot(&root, &intent, State::RenameUnconfirmed);
    value.data.reason = Some("x".repeat(1024));
    let bytes = codec::encode(&root, &intent, &value).unwrap();
    assert!(bytes.len() > 64 * 1024 && bytes.len() < codec::MAX_BYTES);
    assert_eq!(codec::decode(&root, &intent, &bytes).unwrap(), value);
}

#[cfg(target_os = "linux")]
mod supported {
    use super::*;
    use crate::{
        cloud_run::ArtifactDigest,
        repository_overlay::{
            checkout::publication::PublicationFailure,
            materialize::{MaterializationProblem, tests::supported::Fixture},
            retained_setup::{SetupAdmission, SetupObservation, codec as claim},
        },
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fixture(missing: bool) -> Option<(Fixture, RetainedSetup, SetupIntent)> {
        let fixture = Fixture::new(missing);
        let request = fixture.request();
        let store = match RetainedSetup::open(request.scratch_parent) {
            Err(SetupClaimError::Unsupported) => {
                eprintln!("SKIP recorded setup: requires qualified journaled ext4");
                return None;
            }
            other => other.unwrap(),
        };
        let intent = SetupIntent::new(
            "workspace_1".into(),
            request.objects_directory.to_owned(),
            request.bundle_store.to_owned(),
            request.bundle_manifest.clone(),
            request.destination.into(),
        )
        .unwrap();
        Some((fixture, store, intent))
    }

    fn grant(store: &RetainedSetup, intent: &SetupIntent) -> SetupGrant {
        let SetupAdmission::Fresh(grant) = store.admit(intent.clone()).unwrap() else {
            panic!("fresh grant");
        };
        grant
    }

    fn synthetic_rename(result: SetupMaterializationResult, destination: &Path) -> SetupCompletion {
        // Projection-only uncertainty from a real unpublished receipt, not a rename.
        let Err(SetupExecutionError::Materialization(mut failure)) = result else {
            panic!("component failure");
        };
        let problem = std::mem::replace(&mut failure.problem, MaterializationProblem::InvalidRequest);
        let MaterializationProblem::Publication(publication) = problem else {
            panic!("publication failure");
        };
        let PublicationFailure::Unpublished { checkout, .. } = *publication else {
            panic!("unpublished receipt");
        };
        failure.problem = MaterializationProblem::Publication(Box::new(PublicationFailure::RenameUnconfirmed {
            checkout,
            destination: destination.to_owned(),
        }));
        SetupCompletion::from_execution(&Err(SetupExecutionError::Materialization(failure)))
    }

    #[test]
    fn qualified_success_and_failures_survive_process_reopen_without_replay() {
        for kind in 0..6 {
            let Some((fixture, store, mut intent)) = fixture(kind == 2) else {
                return;
            };
            let root = fixture.request().scratch_parent;
            if kind == 1 {
                intent.bundle_manifest = ArtifactDigest::sha256(b"missing");
            }
            let destination = root.join(SCRATCH_NAME).join(&intent.destination);
            let result = grant(&store, &intent)
                .materialize_recorded(|| {
                    if kind == 5 && destination.parent().unwrap().exists() && !destination.exists() {
                        fs::write(&destination, b"sentinel").unwrap();
                    }
                    kind == 3 || (kind == 4 && destination.exists())
                })
                .unwrap();
            let completion = store.completion(&intent).unwrap().unwrap();
            assert_eq!(
                completion.state(),
                match kind {
                    0 => State::Published,
                    3 => State::Rejected,
                    4 => State::PublishedUnsynchronized,
                    _ => State::Unpublished,
                }
            );
            assert_eq!(result.is_ok(), kind == 0);
            assert_eq!(completion.source_metadata().is_some(), matches!(kind, 0 | 2 | 4 | 5));
            assert_eq!(completion.checkout().is_some(), matches!(kind, 0 | 4 | 5));
            if kind >= 4 {
                assert_eq!(completion.base_commit(), Some(fixture.base.to_string().as_str()));
                assert_eq!(completion.bundle_manifest(), Some(&intent.bundle_manifest));
                assert!(completion.checkout().unwrap().join("file").exists());
                assert!(completion.source_metadata().unwrap().join("HEAD").exists());
            }
            if let Ok(repository) = &result {
                assert_eq!(completion.checkout(), Some(repository.checkout().path()));
                assert_eq!(completion.base_commit(), Some(fixture.base.to_string().as_str()));
                assert_eq!(
                    fs::read(repository.checkout().path().join("file")).unwrap(),
                    b"working\0raw\r\n"
                );
            }
            if kind == 5 {
                let projected = synthetic_rename(result, &destination);
                assert_eq!(projected.state(), State::RenameUnconfirmed);
                assert_eq!(projected.checkout(), completion.checkout());
                assert_eq!(projected.possible_destination(), Some(destination.as_path()));
                assert_eq!(projected.base_commit(), completion.base_commit());
                assert_eq!(projected.bundle_manifest(), completion.bundle_manifest());
                let bytes = codec::encode(root, &intent, &projected).unwrap();
                assert_eq!(codec::decode(root, &intent, &bytes).unwrap(), projected);
            } else {
                drop(result);
            }
            let before = fs::read(root.join("setup-result.json")).unwrap();
            drop(store);
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "repository_overlay::retained_setup::outcome::tests::supported::reopen_child",
                    "--nocapture",
                ])
                .env("HORIZON_SETUP_OUTCOME_FIXTURE", root)
                .output()
                .unwrap();
            assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stderr));
            assert!(String::from_utf8_lossy(&child.stdout).contains("RECORDED_SETUP_NO_REPLAY"));
            assert_eq!(fs::read(root.join("setup-result.json")).unwrap(), before);
            assert_eq!(
                fs::metadata(root.join("setup-result.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
        }
    }

    #[test]
    fn reopen_child() {
        let Some(root) = std::env::var_os("HORIZON_SETUP_OUTCOME_FIXTURE") else {
            return;
        };
        let root = Path::new(&root);
        let intent = claim::decode(&fs::read(root.join("setup-claim.json")).unwrap()).unwrap();
        let store = RetainedSetup::open(root).unwrap();
        assert!(store.completion(&intent).unwrap().is_some());
        assert_eq!(store.observe(&intent), Ok(SetupObservation::ClaimedUnknown));
        assert!(matches!(store.admit(intent), Ok(SetupAdmission::Existing)));
        println!("RECORDED_SETUP_NO_REPLAY");
    }

    #[test]
    fn uncertain_recording_keeps_full_actual_execution_and_unsafe_preflight_never_runs() {
        for preflight in [true, false] {
            let Some((fixture, store, intent)) = fixture(false) else {
                return;
            };
            let root = fixture.request().scratch_parent;
            let grant = grant(&store, &intent);
            let result_path = root.join("setup-result.json");
            if preflight {
                fs::write(&result_path, b"invalid").unwrap();
            }
            let result = grant
                .materialize_recorded(|| {
                    if !result_path.exists() {
                        fs::write(&result_path, b"invalid").unwrap();
                    }
                    false
                })
                .unwrap_err();
            assert_eq!(result.execution().is_some(), !preflight);
            assert_eq!(root.join(SCRATCH_NAME).exists(), !preflight);
            if let Some(result) = result.into_execution() {
                let repository = result.unwrap();
                assert!(repository.checkout().path().join("file").exists());
                assert!(repository.source_metadata().join("HEAD").exists());
            }
            assert_eq!(fs::read(&result_path).unwrap(), b"invalid");
            assert!(matches!(store.admit(intent), Ok(SetupAdmission::Existing)));
        }
    }
}
