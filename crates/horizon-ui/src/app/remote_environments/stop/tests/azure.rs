//! Azure saved-Stop check in the overview: offered only for retained persistent Azure
//! workers with saved intent, never a first Stop, and painting dispatches nothing.
use super::*;
use crate::app::test_support::raw_input;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_run::interactive_worker::InteractiveWorkerLifecycle;

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
        assert!(!supported(&expected), "existing intent is checked, never resent");
        assert_eq!(
            start_supported(&expected),
            matches!(phase, RemoteRuntimePhase::Stopped { .. }),
            "only a verified Stop can be started: {phase:?}"
        );
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
        assert!(
            supported(&expected),
            "a retained worker without intent may be stopped: {phase:?}"
        );
        let mut faults = vec![expected.clone(); 3];
        faults[0].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        faults[1].worker_identity = None;
        faults[2].worker_identity.as_mut().expect("worker").provider = CloudProvider::RunPod;
        for fault in faults {
            assert!(!supported(&fault), "{phase:?}");
        }
    }
}

#[test]
fn azure_stop_confirmation_discloses_the_deallocation_and_never_submits_implicitly() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = azure_summary(RemoteRuntimePhase::Ready);
    let ctx = Context::default();
    let mut state = StopState::default();
    for size in [[1200.0, 900.0], [960.0, 720.0]] {
        state.prepare(&expected, &config(), &ctx);
        assert!(state.confirmation.is_some());
        let mut input = raw_input(size, None);
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(input, |ui| show(ui, &state, &expected, true, &mut action));
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        for required in [
            "Unsaved process memory is lost",
            "Deallocates the worker VM",
            "retained data disk keeps /workspace",
            "continues to be billed",
            "immutable saved binding",
            "no private SSH key",
            "not filesystem durability",
            "Azure CLI login",
            "never resend",
            "exiting Horizon",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
        assert!(!text.contains("exact saved HPS"));
        assert!(!text.contains("Retains this local container"));
        assert!(matches!(action, InventoryAction::None));
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
            Some(true)
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-check-enabled-test"))),
            Some(false)
        );
        state.cancel_confirmation();
        assert!(!state.start(&home, &config(), &expected, &ctx));
    }
    assert!(!state.is_pending());
    assert!(!home.root().exists());
}

#[test]
fn azure_stop_results_require_exact_two_step_completion_and_pending_label_warns() {
    let expected = azure_summary(RemoteRuntimePhase::Ready);
    assert!(StopNotice::new(expected.clone(), Ok(completed(&expected))).succeeded);
    let mut changes = vec![completed(&expected); 7];
    changes[0].revision += 1;
    changes[1].workflow_id = Some(CloudWorkflowId::new());
    changes[2].repository = "foreign/repository".into();
    changes[3].saved_phase = Some(RemoteRuntimePhase::Stopped {
        requested_at_millis: 3,
        observed_at_millis: 2,
    });
    changes[4].generation += 1;
    changes[5].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
    changes[6].saved_phase = Some(RemoteRuntimePhase::Ready);
    for changed in changes {
        assert!(!StopNotice::new(expected.clone(), Ok(changed)).succeeded);
    }
    let failed = StopNotice::new(
        expected.clone(),
        Err(StopError::Azure(ConfiguredAzureStopError::Stop(
            RemoteWorkspaceStopError::ProviderUnavailable,
        ))),
    );
    assert!(!failed.succeeded && !failed.checked);
    assert!(failed.message.contains("Check"), "{}", failed.message);
    let mut state = view(&expected);
    let _sender = pending(&mut state.stop, &expected);
    assert!(state.stop.pending_label().contains("Azure Stop is pending"));
    assert!(state.stop.pending_label().contains("never resend"));
    assert!(!state.stop.check(
        &HorizonHome::from_root("/nonexistent-stop-fixture".into()),
        &config(),
        &expected,
        &Context::default()
    ));
}

#[test]
fn azure_stop_callback_never_creates_or_migrates_the_store_and_reaches_named_profile_admission() {
    let fixture = tempfile::tempdir().expect("fixture");
    let missing = HorizonHome::from_root(fixture.path().join("missing"));
    let expected = azure_summary(RemoteRuntimePhase::Ready);
    assert!(matches!(
        execute(&missing, &config(), &expected),
        Err(StopError::StorageUnavailable)
    ));
    assert!(!missing.root().exists());
    let home = HorizonHome::from_root(fixture.path().join("current"));
    let store = CloudWorkflowStore::open(&home).expect("owned current store");
    let before = std::fs::read(store.path()).expect("snapshot");
    assert!(matches!(
        execute(&home, &config(), &expected),
        Err(StopError::Azure(ConfiguredAzureStopError::Configuration(_)))
    ));
    assert_eq!(std::fs::read(store.path()).expect("retained"), before);
    let mut intent = expected;
    intent.saved_phase = Some(RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
    assert!(matches!(
        execute(&home, &config(), &intent),
        Err(StopError::Azure(ConfiguredAzureStopError::UnsupportedProvider))
    ));
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

fn azure_config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        azure: vec![horizon_core::cloud_run::azure::AzureProfile {
            name: "cpu".into(),
            subscription_id: "11111111-1111-4111-8111-111111111111".into(),
            location: "northeurope".into(),
            vm_size: "Standard_D4s_v3".into(),
            image_pull_identity_id: "/subscriptions/11111111-1111-4111-8111-111111111111/resourceGroups/synthetic/providers/Microsoft.ManagedIdentity/userAssignedIdentities/pull".into(),
            declared_hourly_cost_micros: 123_456,
            registry_login_server: "synthetic.azurecr.io".into(),
            disk_sku: horizon_core::cloud_run::azure::AzureDiskSku::StandardSsdLrs,
        }],
        ..Default::default()
    }
}

fn stopped_summary() -> RemoteEnvironmentSummary {
    azure_summary(RemoteRuntimePhase::Stopped {
        requested_at_millis: 1,
        observed_at_millis: 2,
    })
}

fn started_result(
    expected: &RemoteEnvironmentSummary,
    lifecycle: InteractiveWorkerLifecycle,
    already_running: bool,
) -> ConfiguredAzureStart {
    let mut saved = expected.clone();
    saved.revision += match expected.saved_phase {
        Some(RemoteRuntimePhase::Starting { .. }) => 1,
        _ => 2,
    };
    saved.saved_phase = Some(RemoteRuntimePhase::Reconciling);
    ConfiguredAzureStart {
        saved,
        lifecycle,
        already_running,
    }
}

#[test]
fn azure_start_is_offered_only_for_a_saved_stop_or_start_intent_and_a_starting_record_offers_nothing_else() {
    let stopped = stopped_summary();
    assert!(start_supported(&stopped));
    let starting = azure_summary(RemoteRuntimePhase::Starting { requested_at_millis: 3 });
    assert!(start_supported(&starting), "an existing Start intent is retried");
    assert!(!supported(&starting), "Stop never races a start");
    assert!(!check_supported(&starting), "no Stop intent to check");
    for phase in [
        RemoteRuntimePhase::Ready,
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Failed,
    ] {
        assert!(!start_supported(&azure_summary(phase)), "{phase:?}");
    }
    let mut faults = vec![stopped.clone(); 4];
    faults[0].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
    faults[1].worker_identity = None;
    faults[2].worker_identity.as_mut().expect("worker").provider = CloudProvider::RunPod;
    faults[3].provider = CloudProvider::RunPod;
    for (index, fault) in faults.iter().enumerate() {
        assert!(!start_supported(fault), "fault {index}");
    }
    // Painting a saved-Stopped row offers Start and the check, never a first Stop, and
    // dispatches nothing on its own.
    let ctx = Context::default();
    let state = StopState::default();
    for size in [[1200.0, 900.0], [760.0, 560.0]] {
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input(size, None), |ui| {
            show(ui, &state, &stopped, true, &mut action);
        });
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        assert!(text.contains("Starts the retained compute"), "{text}");
        assert!(text.contains("declared hourly cost"), "{text}");
        assert!(matches!(action, InventoryAction::None));
        for (id, enabled) in [
            ("start-request-enabled-test", true),
            ("stop-request-enabled-test", false),
            ("stop-check-enabled-test", true),
        ] {
            assert_eq!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new(id))),
                Some(enabled),
                "{id}"
            );
        }
    }
}

#[test]
fn azure_start_confirmation_discloses_cost_and_identity_and_never_submits_implicitly() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = stopped_summary();
    let ctx = Context::default();
    let mut state = StopState::default();
    // A record that cannot be started never opens the Start confirmation.
    state.prepare_start(&azure_summary(RemoteRuntimePhase::Ready), &azure_config(), &ctx);
    assert!(state.confirmation.is_none());
    for size in [[1200.0, 900.0], [960.0, 720.0]] {
        state.prepare_start(&expected, &azure_config(), &ctx);
        assert!(
            state
                .confirmation
                .as_ref()
                .is_some_and(|c| c.operation == Operation::Start)
        );
        let mut input = raw_input(size, None);
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(input, |ui| show(ui, &state, &expected, true, &mut action));
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        for required in [
            "Start this environment?",
            "0.12 currency units per hour",
            "not a provider quote",
            "retained data disk keeps billing",
            "already runs is not re-posted",
            "did not survive the stop",
            "nothing resumes a task",
            "no private SSH key",
            "Azure CLI login",
            "Reconnect session panels",
            "exiting Horizon",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
        assert!(!text.contains("Stop this environment?"));
        assert!(matches!(action, InventoryAction::None));
        state.cancel_confirmation();
        assert!(!state.start(&home, &azure_config(), &expected, &ctx));
    }
    // Without the profile in the configuration the cost line stays honest.
    state.prepare_start(&expected, &config(), &ctx);
    let mut action = InventoryAction::None;
    let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
        show(ui, &state, &expected, true, &mut action);
    });
    let text = visible_text(&output.shapes);
    let _ = output.discard_textures();
    assert!(text.contains("declared hourly cost (declared"), "{text}");
    // Config or selection drift between preparing and confirming submits nothing.
    for config_drift in [false, true] {
        state.prepare_start(&expected, &azure_config(), &ctx);
        let mut selected = expected.clone();
        if !config_drift {
            selected.revision += 1;
        }
        let config = if config_drift { config() } else { azure_config() };
        assert!(!state.start(&home, &config, &selected, &ctx));
    }
    assert!(!state.is_pending());
    assert!(!home.root().exists());
}

#[test]
fn azure_start_results_require_reconciling_with_the_exact_revision_and_label_the_outcome() {
    use InteractiveWorkerLifecycle::{Provisioning, Ready};
    let stopped = stopped_summary();
    let starting = azure_summary(RemoteRuntimePhase::Starting { requested_at_millis: 3 });
    for expected in [&stopped, &starting] {
        for (lifecycle, already_running, phrase) in [
            (Ready, false, "Compute started for the same worker"),
            (Provisioning, false, "not attested yet"),
            (Ready, true, "already running; nothing was re-posted"),
        ] {
            let notice = StopNotice::started(
                expected.clone(),
                Ok(started_result(expected, lifecycle, already_running)),
            );
            assert!(notice.succeeded && notice.started && !notice.checked, "{phrase}");
            assert!(notice.message.contains(phrase), "{}", notice.message);
            assert!(notice.message.contains("Reconciling"), "{}", notice.message);
            assert!(notice.message.contains("Nothing resumed a task"), "{}", notice.message);
        }
    }
    let valid = started_result(&stopped, Ready, false);
    let mut wrong = vec![valid.clone(); 6];
    wrong[0].saved.revision += 1;
    wrong[1].saved.revision -= 1;
    wrong[2].saved.saved_phase = Some(RemoteRuntimePhase::Ready);
    wrong[3].saved.saved_phase = stopped.saved_phase;
    wrong[4].saved.workflow_id = Some(CloudWorkflowId::new());
    wrong[5].saved.generation += 1;
    for (index, result) in wrong.into_iter().enumerate() {
        assert!(
            !StopNotice::started(stopped.clone(), Ok(result)).succeeded,
            "wrong {index}"
        );
    }
    // A start reply for a record that was not startable is never a success.
    let ready = azure_summary(RemoteRuntimePhase::Ready);
    assert!(!StopNotice::started(ready.clone(), Ok(started_result(&ready, Ready, false))).succeeded);
    let failed = StopNotice::started(
        stopped.clone(),
        Err(StopError::AzureStart(ConfiguredAzureStartError::Start(
            horizon_core::remote_workspace::start::RemoteWorkspaceStartError::ProviderUnavailable,
        ))),
    );
    assert!(!failed.succeeded && failed.started && failed.unverified);
    let state = StopState {
        notice: Some(failed),
        ..Default::default()
    };
    let ctx = Context::default();
    let mut action = InventoryAction::None;
    let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
        show(ui, &state, &stopped, true, &mut action);
    });
    let text = visible_text(&output.shapes);
    let _ = output.discard_textures();
    assert!(text.contains("Last explicit Start result:"), "{text}");
    assert!(text.contains("press Start again"), "{text}");
    assert!(text.contains("Azure CLI is not signed in"), "{text}");
    assert!(text.contains("Compute may already be billing"), "{text}");
    // Wrong callback kinds are never a Start success, and a Stop reply is not a Start.
    let wrong_kind = StopNotice::finish(
        stopped.clone(),
        Operation::Start,
        Ok(StopResult::Stopped(stopped.clone())),
    );
    assert!(!wrong_kind.succeeded && wrong_kind.started);
    let wrong_kind = StopNotice::finish(
        stopped.clone(),
        Operation::Stop,
        Ok(StopResult::Started(started_result(&stopped, Ready, false))),
    );
    assert!(!wrong_kind.succeeded && !wrong_kind.started);
    // The pending label names Azure Start and warns against assuming.
    let mut view_state = view(&stopped);
    let _sender = pending(&mut view_state.stop, &stopped);
    view_state.stop.pending.as_mut().expect("pending").operation = Operation::Start;
    assert!(view_state.stop.pending_label().contains("Azure Start is pending"));
    assert!(view_state.stop.pending_label().contains("never re-posts"));
}

#[test]
fn azure_start_callback_never_creates_or_migrates_the_store_and_reaches_named_profile_admission() {
    let fixture = tempfile::tempdir().expect("fixture");
    let missing = HorizonHome::from_root(fixture.path().join("missing"));
    let expected = stopped_summary();
    assert!(matches!(
        execute_start(&missing, &config(), &expected),
        Err(StopError::StorageUnavailable)
    ));
    assert!(!missing.root().exists());
    let home = HorizonHome::from_root(fixture.path().join("current"));
    let store = CloudWorkflowStore::open(&home).expect("owned current store");
    let before = std::fs::read(store.path()).expect("snapshot");
    assert!(matches!(
        execute_start(&home, &config(), &expected),
        Err(StopError::AzureStart(ConfiguredAzureStartError::Configuration(_)))
    ));
    assert_eq!(std::fs::read(store.path()).expect("retained"), before);
    assert!(matches!(
        execute_start(&home, &azure_config(), &azure_summary(RemoteRuntimePhase::Ready)),
        Err(StopError::AzureStart(ConfiguredAzureStartError::UnsupportedProvider))
    ));
}
