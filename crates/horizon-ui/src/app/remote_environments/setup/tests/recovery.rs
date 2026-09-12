use super::super::*;
use crate::test_egui::DiscardTextures;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn scope(home: &HorizonHome) -> Scope {
    Scope {
        home: home.root().to_path_buf(),
        owner: OWNER.into(),
        config: serde_json::from_value(serde_json::json!({"local_docker":[{
            "name":"local", "docker_host":"unix:///synthetic/must-not-connect.sock"
        }]}))
        .expect("synthetic profile"),
    }
}

fn pending(
    state: &mut SetupState,
    scope: &Scope,
    locator: Option<RemoteWorkspaceSetupLocator>,
) -> mpsc::SyncSender<Result<Completion, String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    state.pending = Some(Pending {
        receiver,
        scope: scope.clone(),
        creation_locator: locator,
        discard: false,
        started: std::time::Instant::now(),
    });
    sender
}

fn locator(home: &HorizonHome, name: &str) -> RemoteWorkspaceSetupLocator {
    RemoteWorkspaceSetupLocator::new(home, OWNER, name).expect("coordinates")
}

#[test]
fn old_attempt_response_cannot_be_accepted_for_a_new_attempt() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("not-opened"));
    let scope = scope(&home);
    let first = locator(&home, "first");
    let second = locator(&home, "second");
    let mut state = SetupState {
        attempts: vec![first.clone(), second.clone()],
        ..Default::default()
    };
    let sender = pending(&mut state, &scope, Some(second.clone()));
    assert!(
        sender
            .send(Ok(Completion::Submitted(Box::new(
                api::ConfiguredWorkspaceSetupAttempt {
                    locator: first.clone(),
                    result: Err(api::ConfiguredWorkspaceSetupError::InvalidRequest),
                }
            ))))
            .is_ok()
    );
    state.sync(Some((&home, OWNER, &scope.config)));
    assert!(state.unknown && state.pending.is_none());
    assert!(
        state
            .notice
            .as_deref()
            .is_some_and(|text| text.contains("did not match"))
    );
    assert!(state.attempts == vec![first, second]);
    assert!(!home.root().exists());
}

#[test]
fn loss_and_context_drift_keep_original_coordinates_without_retry() {
    for discard in [false, true] {
        let temp = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(temp.path().join("not-opened"));
        let scope = scope(&home);
        let original = locator(&home, "original");
        let mut state = SetupState {
            attempts: vec![original.clone()],
            ..Default::default()
        };
        let sender = pending(&mut state, &scope, Some(original.clone()));
        if discard {
            state.invalidate();
        }
        drop(sender);
        state.sync(if discard {
            None
        } else {
            Some((&home, OWNER, &scope.config))
        });
        assert!(state.unknown && state.pending.is_none());
        for _ in 0..3 {
            state.sync(Some((&home, OWNER, &scope.config)));
            state.invalidate();
        }
        assert!(state.attempts == vec![original] && state.pending.is_none());
        assert!(!home.root().exists());
    }
}

#[test]
fn checking_and_editing_never_replace_uncertain_creation_coordinates() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("not-opened"));
    let scope = scope(&home);
    let original = locator(&home, "original");
    let mut state = SetupState {
        attempts: vec![original.clone()],
        unknown: true,
        ..Default::default()
    };
    let ctx = Context::default();
    state.action(Action::New, &home, OWNER, &scope.config, &ctx);
    state.action(Action::Cancel, &home, OWNER, &scope.config, &ctx);
    state.action(Action::CheckAttempt(99), &home, OWNER, &scope.config, &ctx);
    assert!(state.unknown && state.attempts == vec![original.clone()] && state.pending.is_none());
    state.selected = Some(locator(&home, "different"));
    state.action(Action::CheckSelected, &home, OWNER, &scope.config, &ctx);
    settle(&mut state, &scope);
    assert!(state.unknown && state.attempts == vec![original]);
    assert!(!home.root().exists());
}

#[test]
fn settled_history_preserves_inventory_and_check_rejects_wrong_home_or_owner() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let home = HorizonHome::from_root(std::path::PathBuf::from("/synthetic-original"));
    let original = locator(&home, "original");
    let state = &mut app.remote_environments.setup;
    state.attempts.push(original.clone());
    state.unknown = true;
    state.notice = Some("Earlier result".into());
    assert!(!state.is_active());
    let other = HorizonHome::from_root(std::path::PathBuf::from("/synthetic-other"));
    for (current, owner) in [(&other, OWNER), (&home, "00000000-0000-4000-8000-000000000002")] {
        state.action(
            Action::CheckAttempt(0),
            current,
            owner,
            &RemoteProviderConfig::default(),
            &Context::default(),
        );
        assert!(state.pending.is_none() && state.attempts == vec![original.clone()] && state.unknown);
    }
    assert!(app.remote_environments.setup_idle());
    app.remote_environments.page = Some(super::super::super::InventoryPage {
        rows: vec![],
        next_cursor: None,
    });
    let ctx = Context::default();
    let paint = |ui: &mut egui::Ui| {
        super::super::super::paint::show(ui.ctx(), &mut app.remote_environments);
    };
    let _ = ctx
        .run_ui(crate::app::test_support::raw_input([1000.0, 900.0], None), paint)
        .discard_textures();
    let output = ctx
        .run_ui(crate::app::test_support::raw_input([1000.0, 900.0], None), |ui| {
            super::super::super::paint::show(ui.ctx(), &mut app.remote_environments);
        })
        .discard_textures();
    let painted = format!("{:?}", output.shapes);
    assert!(
        painted.contains("Refresh saved page") && painted.contains("No saved"),
        "{painted}"
    );
}

fn settle(state: &mut SetupState, scope: &Scope) {
    let home = HorizonHome::from_root(scope.home.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.pending.is_some() && std::time::Instant::now() < deadline {
        state.sync(Some((&home, OWNER, &scope.config)));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(state.pending.is_none());
}

#[test]
fn pending_warnings_have_explicit_ninety_and_one_eighty_second_boundaries() {
    use crate::app::test_support::raw_input;
    for elapsed in [89, 90, 179, 180] {
        let ctx = Context::default();
        let home = HorizonHome::from_root(std::path::PathBuf::from("/synthetic-unused"));
        let scope = scope(&home);
        let mut state = SetupState::default();
        let _sender = pending(&mut state, &scope, None);
        state.pending.as_mut().expect("pending").started = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(elapsed))
            .expect("synthetic elapsed time");
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input([1000.0, 600.0], None), |ui| state.show(ui, &mut action));
        let painted = format!("{:?}", output.shapes);
        assert_eq!(
            painted.contains("taking longer than expected"),
            (90..180).contains(&elapsed)
        );
        assert_eq!(painted.contains("Still waiting."), elapsed >= 180);
        assert!(painted.contains("does not cancel creation"));
        assert!(matches!(action, InventoryAction::None) && state.pending.is_some() && state.attempts.is_empty());
        let _ = output.discard_textures();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn consent_is_required_and_one_confirmation_is_consumed_once() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("original"));
    let other = HorizonHome::from_root(temp.path().join("refused-before-writes"));
    let scope = scope(&home);
    let prepared = api::preview_configured_remote_workspace(
        &home,
        &scope.config,
        OWNER,
        form::tests::populated(false).draft(i64::MAX).expect("draft"),
    )
    .expect("inert preview");
    let original = prepared.locator().clone();
    let mut state = SetupState {
        scope: Some(scope.clone()),
        review: Some(paint::Review::new(prepared, &scope.config)),
        ..Default::default()
    };
    let ctx = Context::default();
    state.action(Action::Confirm, &home, OWNER, &scope.config, &ctx);
    assert!(state.pending.is_none() && state.review.is_some() && state.attempts.is_empty());
    state.consent = true;
    let scope = Scope {
        home: other.root().to_path_buf(),
        ..scope
    };
    state.scope = Some(scope.clone());
    // Real public submit is deliberately home-mismatched, so it cannot reach storage/provider I/O.
    state.action(Action::Confirm, &other, OWNER, &scope.config, &ctx);
    state.action(Action::Confirm, &other, OWNER, &scope.config, &ctx);
    assert!(state.review.is_none() && state.attempts == vec![original.clone()]);
    settle(&mut state, &scope);
    state.action(Action::Confirm, &other, OWNER, &scope.config, &ctx);
    assert!(state.pending.is_none() && state.attempts == vec![original]);
    assert!(!home.root().exists() && !other.root().exists());
}
