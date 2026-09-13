//! Azure saved-Stop check in the overview: offered only for retained persistent Azure
//! workers with saved intent, never a first Stop, and painting dispatches nothing.
use super::*;
use crate::app::test_support::raw_input;
use crate::test_egui::DiscardTextures;

fn azure_summary(phase: RemoteRuntimePhase) -> RemoteEnvironmentSummary {
    let mut expected = summary();
    expected.provider = CloudProvider::Azure;
    expected.profile = "cpu".into();
    expected.worker_identity.as_mut().expect("worker").provider = CloudProvider::Azure;
    expected.saved_phase = Some(phase);
    expected
}

fn visible_text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn append(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(shape) => {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    append(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in shapes {
        append(&shape.shape, &mut text);
    }
    text
}

#[test]
fn azure_check_requires_saved_intent_on_a_retained_persistent_azure_worker_and_never_offers_stop() {
    for phase in [
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
    ] {
        let expected = azure_summary(phase);
        assert!(check_supported(&expected), "{phase:?}");
        assert!(!supported(&expected), "a first Azure Stop is a later slice");
        let mut faults = vec![expected.clone(); 4];
        faults[0].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        faults[1].worker_identity = None;
        faults[2].worker_identity.as_mut().expect("worker").provider = CloudProvider::RunPod;
        faults[3].provider = CloudProvider::LocalDocker;
        for (index, fault) in faults.iter().enumerate() {
            assert!(!check_supported(fault), "fault {index}");
            if index < 3 {
                assert!(!supported(fault), "fault {index}");
            }
        }
    }
    for phase in [
        RemoteRuntimePhase::Ready,
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Provisioning,
        RemoteRuntimePhase::Failed,
    ] {
        let expected = azure_summary(phase);
        assert!(!check_supported(&expected), "{phase:?}");
        assert!(!supported(&expected), "{phase:?}");
    }
}

#[test]
fn painting_an_azure_row_offers_only_the_check_and_dispatches_nothing() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = azure_summary(RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
    let ctx = Context::default();
    let mut state = StopState::default();
    for size in [[1200.0, 900.0], [760.0, 560.0]] {
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input(size, None), |ui| {
            show(ui, &state, &expected, true, &mut action);
        });
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        for required in [
            "existing Azure Stop intent",
            "no private SSH key",
            "immutable saved binding",
            "Azure CLI login",
            "not task, filesystem, billing or live SSH proof",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
        assert!(!text.contains("existing RunPod Stop intent"));
        assert!(matches!(action, InventoryAction::None));
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-check-enabled-test"))),
            Some(true)
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
            Some(false)
        );
    }
    // Preparing a first Stop for an Azure row is refused before any confirmation exists.
    state.prepare(&expected, &config(), &ctx);
    assert!(state.confirmation.is_none());
    assert!(!state.start(&home, &config(), &expected, &ctx));
    assert!(!state.is_pending());
    assert!(!home.root().exists());
}

#[test]
fn azure_check_results_follow_the_shared_typing_and_an_unverified_check_explains_the_cli() {
    use InteractiveWorkerStopObservation::{Absent, Pending, RetainedStopped};
    let stopping = azure_summary(RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
    let mut confirmed = stopping.clone();
    confirmed.revision += 1;
    confirmed.saved_phase = Some(RemoteRuntimePhase::Stopped {
        requested_at_millis: 1,
        observed_at_millis: 2,
    });
    for (observation, saved) in [
        (Pending, stopping.clone()),
        (Absent, stopping.clone()),
        (RetainedStopped, confirmed.clone()),
    ] {
        let notice = StopNotice::checked(stopping.clone(), Ok(ConfiguredStopConfirmation { saved, observation }));
        assert!(notice.checked);
        assert_eq!(notice.succeeded, observation == RetainedStopped);
    }
    // A retained-stopped answer that did not save completion is not a success.
    assert!(
        !StopNotice::checked(
            stopping.clone(),
            Ok(ConfiguredStopConfirmation {
                saved: stopping.clone(),
                observation: RetainedStopped,
            }),
        )
        .succeeded
    );
    // Only a failed provider observation (credential or control plane) earns the CLI
    // hint; pending, absence and local refusals do not.
    let hint = |result: Result<ConfiguredStopConfirmation, StopError>| {
        let state = StopState {
            notice: Some(StopNotice::checked(stopping.clone(), result)),
            ..Default::default()
        };
        let ctx = Context::default();
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
            show(ui, &state, &stopping, true, &mut action);
        });
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        assert!(matches!(action, InventoryAction::None));
        assert!(text.contains("Last saved Stop check:"), "{text}");
        text.contains("Azure CLI is not signed in") && text.contains("nothing was sent to the worker")
    };
    assert!(hint(Err(StopError::Check(ConfiguredStopConfirmationError::Stop(
        RemoteWorkspaceStopError::ProviderUnavailable,
    )))));
    for result in [
        Ok(ConfiguredStopConfirmation {
            saved: stopping.clone(),
            observation: Pending,
        }),
        Ok(ConfiguredStopConfirmation {
            saved: stopping.clone(),
            observation: Absent,
        }),
        Err(StopError::Check(ConfiguredStopConfirmationError::InvalidBinding)),
        Err(StopError::Check(ConfiguredStopConfirmationError::Stop(
            RemoteWorkspaceStopError::StateChanged,
        ))),
        Err(StopError::StorageUnavailable),
        Err(StopError::WorkerUnavailable),
    ] {
        assert!(!hint(result));
    }
}

#[test]
fn azure_check_callback_reaches_named_profile_admission_without_storage_changes() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("current"));
    let expected = azure_summary(RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
    assert!(matches!(
        execute_check(&home, &config(), &expected),
        Err(StopError::StorageUnavailable)
    ));
    assert!(!home.root().exists(), "a missing store is never created by a check");
    let store = CloudWorkflowStore::open(&home).expect("owned current store");
    let before = std::fs::read(store.path()).expect("snapshot");
    assert!(matches!(
        execute_check(&home, &config(), &expected),
        Err(StopError::Check(ConfiguredStopConfirmationError::Configuration(_)))
    ));
    assert_eq!(std::fs::read(store.path()).expect("retained"), before);
}
