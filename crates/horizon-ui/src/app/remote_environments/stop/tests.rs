mod azure;
mod runpod;

use super::super::{InventoryPage, RemoteEnvironments, paint as inventory_paint};
use super::*;
use horizon_core::cloud_run::{
    CloudJobId, CloudWorkflowId, WorkerLifetime, interactive_worker::InteractiveWorkerIdentity,
    local_docker::LocalDockerProfile,
};

fn summary() -> RemoteEnvironmentSummary {
    let workflow_id = CloudWorkflowId::new();
    let job_id = CloudJobId::new();
    RemoteEnvironmentSummary {
        workspace_local_id: "synthetic-workspace".into(),
        owning_session_id: "00000000-0000-4000-8000-000000000001".into(),
        revision: 1,
        repository: "example/project".into(),
        provider: CloudProvider::LocalDocker,
        profile: "development".into(),
        lifetime: WorkerLifetime::Persistent,
        generation: 1,
        saved_phase: Some(RemoteRuntimePhase::Reconciling),
        workflow_id: Some(workflow_id),
        job_id: Some(job_id),
        worker_identity: Some(InteractiveWorkerIdentity {
            provider: CloudProvider::LocalDocker,
            workflow_id,
            job_id,
            resource_id: "a".repeat(64),
        }),
        checkpoint: None,
        panel_count: 1,
    }
}

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: vec![LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///unused-stop-fixture/docker.sock".into(),
        }],
        ..Default::default()
    }
}

fn completed(expected: &RemoteEnvironmentSummary) -> RemoteEnvironmentSummary {
    let mut saved = expected.clone();
    saved.revision += 2;
    saved.saved_phase = Some(RemoteRuntimePhase::Stopped {
        requested_at_millis: 1,
        observed_at_millis: 2,
    });
    saved
}

fn pending(
    state: &mut StopState,
    expected: &RemoteEnvironmentSummary,
) -> mpsc::SyncSender<Result<StopResult, StopError>> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingStop {
        rx,
        expected: expected.clone(),
        operation: Operation::Stop,
        discard: false,
    });
    tx
}

fn page(expected: &RemoteEnvironmentSummary) -> InventoryPage {
    let mut other = expected.clone();
    other.workspace_local_id = "second-workspace".into();
    InventoryPage {
        rows: vec![
            inventory_paint::InventoryRow::new(expected.clone()),
            inventory_paint::InventoryRow::new(other),
        ],
        next_cursor: None,
    }
}

fn view(expected: &RemoteEnvironmentSummary) -> RemoteEnvironments {
    RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(page(expected)),
        ..Default::default()
    }
}

#[test]
fn confirmation_is_inert_and_exact_selection_and_config_are_required() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = StopState::default();
    assert!(!state.start(&home, &config(), &expected, &ctx));
    state.prepare(&expected, &config(), &ctx);
    assert!(state.confirmation.is_some());
    assert!(!home.cloud_workflow_store_path().exists());
    state.cancel_confirmation();
    assert!(!state.start(&home, &config(), &expected, &ctx));
    state.prepare(&expected, &config(), &ctx);
    let mut changed = expected.clone();
    changed.revision += 1;
    assert!(!state.start(&home, &config(), &changed, &ctx));
    state.prepare(&expected, &config(), &ctx);
    changed = expected.clone();
    changed.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
    assert!(!state.start(&home, &config(), &changed, &ctx));
    state.prepare(&expected, &config(), &ctx);
    assert!(!state.start(&home, &RemoteProviderConfig::default(), &expected, &ctx));
    changed = expected.clone();
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        changed.provider = provider;
        state.prepare(&changed, &config(), &ctx);
        assert!(state.confirmation.is_none());
    }
    changed = expected;
    changed.worker_identity = None;
    state.prepare(&changed, &config(), &ctx);
    assert!(state.confirmation.is_none());
    assert!(!state.is_pending());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn retained_timed_workers_never_offer_or_prepare_stop_in_any_saved_stop_phase() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    for phase in [
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
    ] {
        let ctx = Context::default();
        let mut expected = summary();
        expected.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        expected.saved_phase = Some(phase);
        assert!(expected.worker_identity.is_some());
        assert!(!supported(&expected));
        let mut state = StopState::default();
        let mut action = InventoryAction::None;
        let _ = ctx
            .run_ui(raw_input([1100.0, 780.0], None), |ui| {
                show(ui, &state, &expected, true, &mut action);
            })
            .discard_textures();
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
            Some(false)
        );
        assert!(matches!(action, InventoryAction::None));
        state.prepare(&expected, &config(), &ctx);
        assert!(state.confirmation.is_none());
        assert!(!state.start(&home, &config(), &expected, &ctx));
        assert!(!state.is_pending());
        assert!(!home.cloud_workflow_store_path().exists());
    }
}

#[test]
fn saved_deletion_never_offers_prepares_or_dispatches_stop_for_any_provider() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    for provider in [CloudProvider::LocalDocker, CloudProvider::RunPod, CloudProvider::Azure] {
        for phase in [
            RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 },
            RemoteRuntimePhase::Deleted {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
        ] {
            let ctx = Context::default();
            let mut expected = summary();
            expected.provider = provider;
            expected.worker_identity.as_mut().expect("worker").provider = provider;
            let mut previous = StopState::default();
            previous.prepare(&expected, &config(), &ctx);
            assert_eq!(previous.confirmation.is_some(), supported(&expected));
            expected.saved_phase = Some(phase);
            assert!(!supported(&expected));
            let mut state = StopState::default();
            let mut action = InventoryAction::None;
            let _ = ctx
                .run_ui(raw_input([1100.0, 780.0], None), |ui| {
                    show(ui, &state, &expected, true, &mut action);
                })
                .discard_textures();
            assert_eq!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
                Some(false)
            );
            assert!(matches!(action, InventoryAction::None));
            state.prepare(&expected, &config(), &ctx);
            assert!(state.confirmation.is_none());
            assert!(!state.start(&home, &config(), &expected, &ctx));
            assert!(!previous.start(&home, &config(), &expected, &ctx));
            assert!(!state.is_pending());
            assert!(!previous.is_pending());
            assert!(!home.root().exists());
        }
    }
}

#[test]
fn cancel_selection_page_and_close_clear_only_unconfirmed_intent() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    for action in [
        InventoryAction::CancelStop,
        InventoryAction::Select(1),
        InventoryAction::Close,
    ] {
        let mut state = view(&expected);
        state.stop_action(InventoryAction::RequestStop, &home, &config(), &ctx);
        assert!(state.stop.confirmation.is_some());
        state.apply(action, &home, &ctx);
        state.stop_action(InventoryAction::ConfirmStop, &home, &config(), &ctx);
        assert!(state.stop.confirmation.is_none());
        assert!(!state.stop.is_pending());
    }
    let mut state = view(&expected);
    state.stop.prepare(&expected, &config(), &ctx);
    state.accept_result(None, Ok(page(&expected)));
    assert!(state.stop.confirmation.is_none());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn confirmed_operation_stays_single_flight_through_close_and_invalidation() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = view(&expected);
    let tx = pending(&mut state.stop, &expected);
    for _ in 0..100 {
        state.close();
        state.open = true;
        state.stop.prepare(&expected, &config(), &ctx);
        assert!(!state.stop.start(&home, &config(), &expected, &ctx));
        state.start_observation(&home, &config(), &ctx);
        assert!(!state.observation.is_pending());
        assert!(!state.stop.drain_result());
        assert!(state.stop.is_pending());
    }
    tx.send(Ok(StopResult::Stopped(completed(&expected)))).expect("send");
    assert!(state.stop.drain_result());
    assert!(
        state.stop.notice.is_none(),
        "closed selection discards presentation only"
    );
    assert!(!state.stop.is_pending());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn result_remains_readable_after_saved_page_refresh_but_is_bound_to_exact_target() {
    let expected = summary();
    for succeed in [true, false] {
        let mut state = view(&expected);
        let result = if succeed {
            Ok(completed(&expected))
        } else {
            Err(StopError::StorageUnavailable)
        };
        pending(&mut state.stop, &expected)
            .send(result.map(StopResult::Stopped))
            .expect("send");
        assert!(state.stop.drain_result());
        state.accept_result(None, Ok(page(&completed(&expected))));
        let notice = state.stop.notice.as_ref().expect("last explicit result");
        assert_eq!(notice.succeeded, succeed);
        assert!(same_target(&notice.expected, &completed(&expected)));
        let mut another = expected.clone();
        another.worker_identity.as_mut().expect("identity").resource_id = "b".repeat(64);
        assert!(!same_target(&notice.expected, &another));
        another = expected.clone();
        another.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
        assert!(!same_target(&notice.expected, &another));
        assert!(!notice.message.is_empty());
    }
}

#[test]
fn manual_page_actions_discard_notices_but_retain_the_confirmed_operation() {
    use super::super::{LoadError, LoadFailure};
    let fixture = tempfile::tempdir().expect("fixture");
    let expected = summary();
    let ctx = Context::default();
    for (index, action) in [
        InventoryAction::Refresh,
        InventoryAction::First,
        InventoryAction::Next,
        InventoryAction::Retry,
    ]
    .into_iter()
    .enumerate()
    {
        let home = HorizonHome::from_root(fixture.path().join(format!("navigation-{index}")));
        let mut state = view(&expected);
        state.page.as_mut().expect("page").next_cursor = Some("next-workspace".into());
        state.page_cursor = Some("previous-workspace".into());
        state.failure = Some(LoadFailure {
            error: LoadError::ReadPage,
            cursor: None,
        });
        state.stop.notice = Some(StopNotice::new(expected.clone(), Ok(completed(&expected))));
        let tx = pending(&mut state.stop, &expected);
        state.apply(action, &home, &ctx);
        assert!(state.stop.notice.is_none());
        assert!(state.stop.is_pending());
        let load = state.pending.take().expect("explicit page read");
        assert!(load.rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
        tx.send(Ok(StopResult::Stopped(completed(&expected)))).expect("send");
        assert!(state.stop.drain_result());
        assert!(
            state.stop.notice.is_none(),
            "navigation discards the late presentation, not the operation"
        );
    }
}

#[test]
fn unexpected_completion_and_disconnected_worker_never_claim_success() {
    let expected = summary();
    let mut changes = vec![completed(&expected); 4];
    changes[0].workspace_local_id = "foreign-workspace".into();
    changes[1].generation += 1;
    changes[2].saved_phase = Some(RemoteRuntimePhase::Ready);
    changes[3].revision = 0;
    for changed in changes {
        let notice = StopNotice::new(expected.clone(), Ok(changed));
        assert!(!notice.succeeded);
        assert!(notice.message.contains("does not match"));
    }
    let mut state = StopState::default();
    drop(pending(&mut state, &expected));
    assert!(state.drain_result());
    let notice = state.notice.as_ref().expect("worker failure");
    assert!(!notice.succeeded);
    assert!(notice.message.contains("Refresh saved inventory"));
}

#[test]
fn provider_reload_invalidates_confirmation_but_unrelated_visual_reload_does_not() {
    let (_fixture, mut app) = crate::app::test_support::test_app();
    let expected = summary();
    app.remote_environments = view(&expected);
    app.remote_environments
        .stop
        .prepare(&expected, &app.template_config.remote, &Context::default());
    let mut updated = app.template_config.clone();
    updated.window.width += 1.0;
    app.apply_runtime_config(&updated);
    assert!(app.remote_environments.stop.confirmation.is_some());
    let tx = pending(&mut app.remote_environments.stop, &expected);
    updated.remote = config();
    app.apply_runtime_config(&updated);
    assert!(app.remote_environments.stop.confirmation.is_none());
    assert!(app.remote_environments.stop.is_pending());
    tx.send(Ok(StopResult::Stopped(completed(&expected)))).expect("send");
    assert!(app.remote_environments.stop.drain_result());
    assert!(app.remote_environments.stop.notice.is_none());
}

#[test]
fn completion_queues_saved_refresh_behind_inventory_read_and_closed_view_does_not_load() {
    use super::super::PendingLoad;
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = view(&expected);
    let (inventory_tx, inventory_rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingLoad {
        rx: inventory_rx,
        cursor: None,
        discard: false,
    });
    pending(&mut state.stop, &expected)
        .send(Ok(StopResult::Stopped(completed(&expected))))
        .expect("send");
    state.drain_stop(&home, &ctx);
    assert!(state.refresh_after_stop);
    assert!(state.stop.notice.as_ref().is_some_and(|notice| notice.succeeded));
    assert!(!home.cloud_workflow_store_path().exists());
    inventory_tx.send(Ok(page(&expected))).expect("page");
    state.drain_result();
    state.drain_stop(&home, &ctx);
    assert!(!state.refresh_after_stop);
    let refresh = state.pending.take().expect("queued refresh started");
    assert!(refresh.rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
    assert!(state.stop.notice.is_some());
    let untouched = HorizonHome::from_root(fixture.path().join("closed"));
    let tx = pending(&mut state.stop, &expected);
    state.close();
    tx.send(Ok(StopResult::Stopped(completed(&expected)))).expect("send");
    state.drain_stop(&untouched, &ctx);
    assert!(!state.refresh_after_stop);
    assert!(!untouched.cloud_workflow_store_path().exists());
}

#[test]
fn confirmation_never_consumes_enter_as_an_implicit_stop() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    let ctx = Context::default();
    let expected = summary();
    let mut state = StopState::default();
    state.prepare(&expected, &config(), &ctx);
    for enter in [false, true] {
        let mut input = raw_input([1100.0, 780.0], None);
        if enter {
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }
        let mut action = InventoryAction::None;
        let _ = ctx
            .run_ui(input, |ui| show(ui, &state, &expected, true, &mut action))
            .discard_textures();
        assert!(matches!(action, InventoryAction::None));
        assert!(state.confirmation.is_some());
        assert!(!state.is_pending());
    }
}

fn check_summary() -> RemoteEnvironmentSummary {
    let mut expected = summary();
    expected.provider = CloudProvider::RunPod;
    expected.worker_identity.as_mut().expect("worker").provider = CloudProvider::RunPod;
    expected.saved_phase = Some(RemoteRuntimePhase::Stopping { requested_at_millis: 1 });
    expected
}

#[test]
fn checking_requires_existing_persistent_runpod_intent_and_paint_never_dispatches() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    let expected = check_summary();
    let mut candidates = vec![expected.clone(); 6];
    candidates[1].saved_phase = Some(RemoteRuntimePhase::Ready);
    candidates[2].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
    candidates[3].worker_identity = None;
    candidates[4].provider = CloudProvider::Azure;
    candidates[5].provider = CloudProvider::LocalDocker;
    let ctx = Context::default();
    let state = StopState::default();
    for (index, candidate) in candidates.iter().enumerate() {
        for size in [[1100.0, 780.0], [760.0, 560.0]] {
            let mut action = InventoryAction::None;
            let _ = ctx
                .run_ui(raw_input(size, None), |ui| {
                    show(ui, &state, candidate, true, &mut action);
                })
                .discard_textures();
            assert_eq!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-check-enabled-test"))),
                Some(index == 0 && cfg!(target_os = "linux"))
            );
            assert!(matches!(action, InventoryAction::None));
            assert!(!state.is_pending());
        }
    }
}

#[cfg(target_os = "linux")]
mod checks {
    use super::*;
    use InteractiveWorkerStopObservation::{Absent, Pending, RetainedStopped};

    fn checked(
        expected: &RemoteEnvironmentSummary,
        observation: InteractiveWorkerStopObservation,
    ) -> ConfiguredStopConfirmation {
        let mut saved = expected.clone();
        if observation == RetainedStopped && matches!(expected.saved_phase, Some(RemoteRuntimePhase::Stopping { .. })) {
            saved.revision += 1;
            saved.saved_phase = Some(RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            });
        }
        ConfiguredStopConfirmation { saved, observation }
    }

    fn pending_check(
        state: &mut StopState,
        expected: &RemoteEnvironmentSummary,
    ) -> mpsc::SyncSender<Result<StopResult, StopError>> {
        let sender = pending(state, expected);
        state.pending.as_mut().expect("pending").operation = Operation::Check;
        sender
    }

    #[test]
    fn typed_results_never_conflate_absence_pending_or_an_old_saved_stop_with_fresh_success() {
        let stopping = check_summary();
        let stopped = checked(&stopping, RetainedStopped).saved;
        for expected in [&stopping, &stopped] {
            for observation in [Pending, Absent, RetainedStopped] {
                let result = checked(expected, observation);
                let notice = StopNotice::checked(expected.clone(), Ok(result.clone()));
                assert!(notice.checked);
                assert_eq!(notice.succeeded, observation == RetainedStopped);
                assert!(notice.message.contains(match observation {
                    Pending => "not yet confirmed",
                    Absent => "absent",
                    RetainedStopped => "confirmed at this check",
                }));
                if observation != RetainedStopped {
                    assert_eq!(&result.saved, expected);
                }
                let mut wrong = result;
                wrong.saved.generation += 1;
                assert!(!StopNotice::checked(expected.clone(), Ok(wrong)).succeeded);
            }
        }
        let valid = checked(&stopping, RetainedStopped);
        let mut wrong = vec![valid.clone(); 5];
        wrong[0].saved.revision += 1;
        wrong[1].saved.workflow_id = Some(CloudWorkflowId::new());
        wrong[2].saved.repository = "other/repository".into();
        wrong[3].saved.saved_phase = Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: 2,
            observed_at_millis: 2,
        });
        wrong[4].saved.saved_phase = Some(RemoteRuntimePhase::Ready);
        for result in wrong {
            assert!(!StopNotice::checked(stopping.clone(), Ok(result)).succeeded);
        }
        let mut renewed = checked(&stopped, RetainedStopped);
        renewed.saved.saved_phase = Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 3,
        });
        assert!(
            !StopNotice::checked(stopped, Ok(renewed)).succeeded,
            "saved Stop timestamps are immutable"
        );
    }

    #[test]
    fn check_is_single_flight_and_close_navigation_or_config_change_discard_only_presentation() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("unused"));
        let expected = check_summary();
        let ctx = Context::default();
        for action in [
            InventoryAction::Close,
            InventoryAction::Select(1),
            InventoryAction::None,
        ] {
            let mut state = view(&expected);
            let sender = pending_check(&mut state.stop, &expected);
            assert!(state.stop.pending_label().contains("No Stop request is sent"));
            for _ in 0..20 {
                state.stop_action(InventoryAction::CheckStop, &home, &config(), &ctx);
                assert!(!state.stop.drain_result());
                state.start_observation(&home, &config(), &ctx);
                assert!(!state.observation.is_pending());
            }
            if matches!(action, InventoryAction::None) {
                state.invalidate_provider_state();
            } else {
                state.apply(action, &home, &ctx);
            }
            sender
                .send(Ok(StopResult::Checked(checked(&expected, RetainedStopped))))
                .expect("send");
            assert!(state.stop.drain_result());
            assert!(state.stop.notice.is_none());
            assert!(!state.stop.is_pending());
        }
        assert!(!home.root().exists());
    }

    #[test]
    fn check_notice_survives_own_saved_refresh_and_rejects_wrong_result_kind() {
        let expected = check_summary();
        let mut state = view(&expected);
        let result = checked(&expected, RetainedStopped);
        pending_check(&mut state.stop, &expected)
            .send(Ok(StopResult::Checked(result.clone())))
            .expect("send");
        assert!(state.stop.drain_result());
        state.accept_result(None, Ok(page(&result.saved)));
        assert!(
            state
                .stop
                .notice
                .as_ref()
                .is_some_and(|notice| notice.succeeded && notice.checked)
        );
        pending_check(&mut state.stop, &expected)
            .send(Ok(StopResult::Stopped(result.saved)))
            .expect("wrong callback kind");
        assert!(state.stop.drain_result());
        assert!(state.stop.notice.as_ref().is_some_and(|notice| !notice.succeeded));
        drop(pending_check(&mut state.stop, &expected));
        assert!(state.stop.drain_result());
        assert!(
            state
                .stop
                .notice
                .as_ref()
                .expect("error")
                .message
                .contains("does not resend Stop")
        );
    }

    #[test]
    fn missing_or_insecure_existing_store_is_not_created_or_repaired_by_check() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("missing"));
        let expected = check_summary();
        assert!(matches!(
            execute_check(&home, &config(), &expected),
            Err(StopError::StorageUnavailable)
        ));
        assert!(!home.root().exists());
        let store = CloudWorkflowStore::open(&home).expect("owned empty fixture");
        let before = std::fs::read(store.path()).expect("snapshot");
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).expect("insecure fixture");
        assert!(matches!(
            execute_check(&home, &config(), &expected),
            Err(StopError::StorageUnavailable)
        ));
        assert_eq!(
            std::fs::metadata(store.path()).expect("metadata").permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(std::fs::read(store.path()).expect("retained"), before);
    }

    #[derive(Eq, PartialEq)]
    struct StoreSnapshot {
        bytes: Vec<u8>,
        version: i64,
        journal: String,
    }

    fn store_snapshot(path: &std::path::Path) -> StoreSnapshot {
        let reader = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only fixture snapshot");
        StoreSnapshot {
            bytes: std::fs::read(path).expect("committed bytes"),
            version: reader
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("version"),
            journal: reader
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .expect("journal mode"),
        }
    }

    #[test]
    fn check_refuses_legacy_and_drifted_store_without_schema_or_journal_writes() {
        let fixture = tempfile::tempdir().expect("fixture");
        for (name, sql) in [
            (
                "legacy-six",
                "DROP TABLE remote_provider_bindings; PRAGMA user_version = 6;",
            ),
            ("missing-index", "DROP INDEX remote_workspaces_session;"),
            (
                "unexpected-trigger",
                "CREATE TRIGGER unexpected_workflow_update AFTER UPDATE ON cloud_workflows
                 BEGIN DELETE FROM remote_workspaces; END;",
            ),
        ] {
            let home = HorizonHome::from_root(fixture.path().join(name));
            let store = CloudWorkflowStore::open(&home).expect("owned fixture");
            let connection = rusqlite::Connection::open(store.path()).expect("fixture writer");
            connection.execute_batch(sql).expect("synthetic schema change");
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("committed fixture");
            drop(connection);
            CloudWorkflowStore::open_read_only(&home).expect("existing reader admits this fixture");
            let before = store_snapshot(store.path());
            let result = execute_check(&home, &config(), &check_summary());
            let after = store_snapshot(store.path());
            assert!(
                before == after,
                "{name}: {result:?}; version {} -> {}; journal {} -> {}; bytes equal: {}",
                before.version,
                after.version,
                before.journal,
                after.journal,
                before.bytes == after.bytes
            );
            assert!(
                matches!(result, Err(StopError::StorageUnavailable)),
                "{name}: {result:?}"
            );
        }
    }

    #[test]
    fn current_store_check_reaches_configured_admission_without_storage_changes() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("current"));
        let store = CloudWorkflowStore::open(&home).expect("owned current store");
        let connection = rusqlite::Connection::open(store.path()).expect("fixture writer");
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .expect("committed fixture");
        drop(connection);
        let before = store_snapshot(store.path());
        assert!(matches!(
            execute_check(&home, &config(), &check_summary()),
            Err(StopError::Check(ConfiguredStopConfirmationError::Configuration(_)))
        ));
        assert!(store_snapshot(store.path()) == before);
    }
}
