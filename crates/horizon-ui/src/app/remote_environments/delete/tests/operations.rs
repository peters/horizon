use super::*;

#[test]
fn delete_and_retry_each_require_fresh_exact_consent_while_check_never_confirms_delete() {
    let mut fixture = Fixture::new(CloudProvider::RunPod);
    let mut state = DeleteState::default();
    for (action, operation) in [(Action::Request, Operation::Delete), (Action::Retry, Operation::Retry)] {
        if operation == Operation::Retry {
            fixture.scope.expected.saved_phase = Some(requested());
        }
        assert!(fixture.action(&mut state, action).is_none());
        assert!(!state.confirmation.as_ref().expect("consent").acknowledged);
        assert!(fixture.action(&mut state, Action::Confirm).is_none());
        fixture.action(&mut state, action);
        state.confirmation.as_mut().expect("consent").acknowledged = true;
        assert_eq!(
            fixture
                .action(&mut state, Action::Confirm)
                .expect("one request")
                .operation,
            operation
        );
        assert!(fixture.action(&mut state, Action::Confirm).is_none());
        fixture.action(&mut state, action);
        fixture.action(&mut state, Action::Cancel);
        assert!(fixture.action(&mut state, Action::Confirm).is_none());
    }
    fixture.action(&mut state, Action::Retry);
    assert_eq!(
        fixture
            .action(&mut state, Action::Check)
            .expect("manual check")
            .operation,
        Operation::Check
    );
    assert!(state.confirmation.is_none());
    assert!(!fixture.scope.home.root().exists());
}

#[test]
fn consent_and_pending_results_are_discarded_on_home_config_or_exact_selection_drift() {
    for drift in 0..3 {
        let mut fixture = Fixture::new(CloudProvider::Azure);
        let mut state = DeleteState::default();
        fixture.action(&mut state, Action::Request);
        state.confirmation.as_mut().expect("consent").acknowledged = true;
        let tx = fixture.pending(&mut state, Operation::Delete);
        tx.send(Ok(fixture.result(2, deleted(), true)))
            .expect("synthetic result");
        match drift {
            0 => fixture.scope.home = HorizonHome::from_root(fixture.scope.home.root().join("other")),
            1 => fixture
                .scope
                .config
                .local_docker
                .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
                    name: "other".into(),
                    docker_host: "unix:///synthetic-unused.sock".into(),
                }),
            _ => fixture.scope.expected.revision += 1,
        }
        state.sync(
            &fixture.scope.home,
            &fixture.scope.config,
            Some(&fixture.scope.expected),
        );
        assert!(state.confirmation.is_none());
        assert!(state.is_pending());
        assert!(state.drain());
        assert!(state.notice.is_none());
        assert!(fixture.action(&mut state, Action::Confirm).is_none());
    }
}

#[test]
fn only_exact_operation_revision_phase_and_metadata_transitions_can_show_success() {
    for (operation, initial, delta, phase, verified, status) in [
        (
            Operation::Delete,
            RemoteRuntimePhase::Ready,
            1,
            requested(),
            false,
            Status::Pending,
        ),
        (
            Operation::Delete,
            RemoteRuntimePhase::Ready,
            2,
            deleted(),
            true,
            Status::Verified,
        ),
        (Operation::Check, requested(), 0, requested(), false, Status::Pending),
        (Operation::Check, requested(), 1, deleted(), true, Status::Verified),
        (Operation::Check, deleted(), 0, deleted(), true, Status::Historical),
        (Operation::Retry, requested(), 1, requested(), false, Status::Pending),
        (Operation::Retry, requested(), 1, deleted(), true, Status::Verified),
        (Operation::Retry, requested(), 2, deleted(), true, Status::Verified),
    ] {
        let mut fixture = Fixture::new(CloudProvider::Azure);
        fixture.scope.expected.saved_phase = Some(initial);
        let result = fixture.result(delta, phase, verified);
        let saved = result.saved.clone();
        let notice = Notice::finish(fixture.request(operation), Ok(result));
        assert_eq!(notice.status, status);
        assert!(notice.matches(&fixture.scope.home, &fixture.scope.config, Some(&saved)));
        for fault in 0..7 {
            let mut result = fixture.result(delta, phase, verified);
            match fault {
                0 => result.saved.revision += 3,
                1 => result.saved.generation += 1,
                2 => result.saved.profile.push('x'),
                3 => result.saved.worker_identity = None,
                4 => result.saved.repository.push('x'),
                5 => result.absence_verified = !verified,
                _ => {
                    result.saved.saved_phase = Some(RemoteRuntimePhase::Deleted {
                        requested_at_millis: -1,
                        observed_at_millis: -2,
                    });
                }
            }
            assert_eq!(
                Notice::finish(fixture.request(operation), Ok(result)).status,
                Status::Unverified
            );
        }
    }
    let mut fixture = Fixture::new(CloudProvider::RunPod);
    fixture.scope.expected.revision = u64::MAX;
    let mut result = fixture.result(0, deleted(), true);
    result.saved.revision = 1;
    assert_eq!(
        Notice::finish(fixture.request(Operation::Delete), Ok(result)).status,
        Status::Unverified
    );
}

#[test]
fn failed_or_disconnected_operation_never_replays_and_closure_retains_inflight_work() {
    let fixture = Fixture::new(CloudProvider::RunPod);
    let ctx = Context::default();
    let mut view = fixture.view();
    let tx = fixture.pending(&mut view.delete, Operation::Delete);
    for action in [Action::Request, Action::Check, Action::Retry, Action::Confirm] {
        assert!(fixture.action(&mut view.delete, action).is_none());
    }
    view.close();
    assert!(view.delete.is_pending());
    view.open(&fixture.scope.home, &ctx);
    assert!(view.pending.is_none());
    assert!(view.refresh_when_idle);
    tx.send(Err(Error::Core(
        ConfiguredEnvironmentDeleteError::UnverifiedRunPodContext,
    )))
    .expect("failure");
    view.drain_delete(&fixture.scope.home, &fixture.scope.config);
    assert!(!view.delete.is_pending());
    assert!(view.delete.notice.is_none());
    assert!(!fixture.scope.home.root().exists());
    let mut hidden = fixture.view();
    drop(fixture.pending(&mut hidden.delete, Operation::Check));
    hidden.close();
    hidden.drain_delete(&fixture.scope.home, &fixture.scope.config);
    assert!(!hidden.delete.is_pending());
    assert!(!hidden.refresh_when_idle);
    let mut state = DeleteState::default();
    drop(fixture.pending(&mut state, Operation::Delete));
    assert!(state.drain());
    assert_eq!(state.notice.as_ref().expect("unknown").status, Status::Unverified);
    assert!(!state.drain());
}

#[test]
fn central_dispatch_blocks_every_competing_action_and_selection_discards_completion() {
    let fixture = Fixture::new(CloudProvider::Azure);
    let mut view = fixture.view();
    let tx = fixture.pending(&mut view.delete, Operation::Delete);
    for action in [
        InventoryAction::Refresh,
        InventoryAction::First,
        InventoryAction::Next,
        InventoryAction::Retry,
        InventoryAction::Observe,
        InventoryAction::RequestStop,
        InventoryAction::ConfirmStop,
        InventoryAction::CheckStop,
        InventoryAction::RequestStart,
        InventoryAction::ConfirmStart,
        InventoryAction::ListReconnectViews,
        InventoryAction::ListReopenPanels,
        InventoryAction::ReopenView(0),
        InventoryAction::InspectTask(0),
        InventoryAction::PrepareTaskStart(0),
        InventoryAction::ConfirmTaskStart,
        InventoryAction::PrepareRepository,
        InventoryAction::ConfirmRepository,
        InventoryAction::InspectRepository,
        InventoryAction::Reconnect(horizon_core::PanelId(1)),
        InventoryAction::WorkspaceSetup(super::super::super::setup::Action::New),
        InventoryAction::WorkspaceSetup(super::super::super::setup::Action::Confirm),
        InventoryAction::Delete(Action::Retry),
    ] {
        assert!(matches!(view.guard_delete_action(action), InventoryAction::None));
    }
    assert!(matches!(
        view.guard_delete_action(InventoryAction::Close),
        InventoryAction::Close
    ));
    view.apply(InventoryAction::Select(1), &fixture.scope.home, &Context::default());
    tx.send(Ok(fixture.result(2, deleted(), true))).expect("result");
    view.drain_delete(&fixture.scope.home, &fixture.scope.config);
    assert!(view.delete.notice.is_none());
    let (_tx, rx) = mpsc::sync_channel(1);
    view.pending = Some(super::super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    assert!(matches!(
        view.guard_delete_action(InventoryAction::Delete(Action::Request)),
        InventoryAction::None
    ));
}
