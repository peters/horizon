use super::*;

#[test]
fn no_owning_session_or_other_pending_work_cannot_open_form() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.remote_environments.open = true;
    app.remote_workspace_request_action(InventoryAction::RequestPreview(Action::New), &Context::default());
    assert!(!app.remote_environments.request_preview.is_active());
    let (_sender, rx) = mpsc::sync_channel(1);
    app.remote_environments.pending = Some(super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    assert!(!app.remote_environments.request_preview_idle());
}

#[test]
fn entry_action_requires_persistent_linux_owner_and_invalidates_on_session_change() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    app.remote_environments.open = true;
    app.template_config.remote = form::tests::config();
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "00000000-0000-4000-8000-000000000001".into(),
        lease: None,
        last_lease_refresh: None,
        persistent: false,
    });
    app.remote_workspace_request_action(InventoryAction::RequestPreview(Action::New), &ctx);
    assert!(!app.remote_environments.request_preview.is_active());
    app.active_session.as_mut().expect("session").persistent = true;
    app.remote_workspace_request_action(InventoryAction::RequestPreview(Action::New), &ctx);
    assert_eq!(
        app.remote_environments.request_preview.is_active(),
        cfg!(target_os = "linux")
    );
    app.active_session.as_mut().expect("session").session_id = "00000000-0000-4000-8000-000000000002".into();
    app.remote_workspace_request_action(InventoryAction::None, &ctx);
    assert!(!app.remote_environments.request_preview.is_active());
    let (_sender, rx) = mpsc::sync_channel(1);
    app.remote_environments.pending = Some(super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    app.remote_workspace_request_action(InventoryAction::RequestPreview(Action::New), &ctx);
    assert!(!app.remote_environments.request_preview.is_active());
    assert!(!app.session_store.home().cloud_workflow_store_path().exists());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{app::test_support::raw_input, test_egui::DiscardTextures};
    const OWNER: &str = "00000000-0000-4000-8000-000000000001";

    fn fixture(cloud: bool) -> (tempfile::TempDir, HorizonHome, Scope, PreviewState) {
        let temp = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(temp.path().join("must-not-be-created"));
        let scope = Scope {
            home: home.root().to_path_buf(),
            owner: OWNER.into(),
            config: form::tests::config(),
        };
        let state = PreviewState {
            form: Some(form::tests::populated(cloud)),
            scope: Some(scope.clone()),
            ..Default::default()
        };
        (temp, home, scope, state)
    }
    fn prepared(home: &HorizonHome, scope: &Scope, cloud: bool) -> PreparedRemoteWorkspaceSetup {
        preview_configured_remote_workspace(
            home,
            &scope.config,
            OWNER,
            form::tests::populated(cloud).draft(i64::MAX).expect("draft"),
        )
        .expect("preview")
    }
    fn pending(
        state: &mut PreviewState,
        scope: &Scope,
    ) -> mpsc::SyncSender<Result<PreparedRemoteWorkspaceSetup, String>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        state.pending = Some(Pending {
            receiver,
            scope: scope.clone(),
            discard: false,
        });
        sender
    }
    fn render(state: &mut PreviewState, size: [f32; 2]) -> String {
        let ctx = Context::default();
        let output = ctx
            .run_ui(raw_input(size, None), |ui| {
                egui::CentralPanel::default().show(ui, |ui| state.show(ui, &mut InventoryAction::None));
            })
            .discard_textures();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn actual_preview_worker_is_single_flight_and_never_creates_home() {
        for cloud in [false, true] {
            let (_temp, home, scope, mut state) = fixture(cloud);
            let ctx = Context::default();
            state.action(Action::Review, &home, OWNER, &scope.config, &ctx);
            assert!(state.pending.is_some());
            let retained_scope = state.scope.clone();
            state.action(Action::New, &home, OWNER, &scope.config, &ctx);
            assert!(state.scope == retained_scope && state.pending.is_some());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while state.pending.is_some() && std::time::Instant::now() < deadline {
                state.sync(Some((&home, OWNER, &scope.config)));
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(state.pending.is_none() && state.review.is_some(), "{:?}", state.notice);
            assert!(!home.root().exists());
            state.action(Action::Edit, &home, OWNER, &scope.config, &ctx);
            assert!(state.form.is_some() && state.review.is_none());
            state.action(Action::Cancel, &home, OWNER, &scope.config, &ctx);
            assert!(!state.is_active() && !home.root().exists());
        }
    }
    #[test]
    fn canceled_or_changed_context_discards_late_review_without_restart() {
        for mode in 0..5 {
            let (_temp, home, scope, mut state) = fixture(false);
            let tx = pending(&mut state, &scope);
            let mut config = scope.config.clone();
            let mut owner = OWNER;
            let other_home = HorizonHome::from_root(home.root().join("other"));
            let mut current_home = &home;
            match mode {
                0 => state.action(Action::Cancel, &home, OWNER, &scope.config, &Context::default()),
                1 => config.local_docker[0].docker_host = "unix:///changed.sock".into(),
                2 => owner = "00000000-0000-4000-8000-000000000002",
                3 => current_home = &other_home,
                _ => state.invalidate(),
            }
            assert!(tx.send(Ok(prepared(&home, &scope, false))).is_ok());
            state.sync(Some((current_home, owner, &config)));
            assert!(state.review.is_none() && !state.is_active());
            state.sync(Some((&home, OWNER, &scope.config)));
            assert!(!state.is_active() && !home.root().exists());
        }
    }
    #[test]
    fn disconnected_preview_has_definite_noncreating_notice_and_cancel_clears_it() {
        let (_temp, home, scope, mut state) = fixture(false);
        drop(pending(&mut state, &scope));
        state.sync(Some((&home, OWNER, &scope.config)));
        assert_eq!(
            state.notice.as_deref(),
            Some("Request review could not finish. Nothing has been created.")
        );
        assert!(state.review.is_none() && state.pending.is_none());
        state.invalidate();
        assert!(!state.is_active() && !home.root().exists());
    }
    #[test]
    fn exact_summary_and_narrow_form_have_no_creation_or_consent_controls() {
        for cloud in [false, true] {
            let (_temp, home, scope, mut state) = fixture(cloud);
            let form = render(&mut state, [390.0, 1000.0]);
            assert!(form.contains("Nothing has been created") && form.contains("Review request"));
            let tx = pending(&mut state, &scope);
            assert!(tx.send(Ok(prepared(&home, &scope, cloud))).is_ok());
            state.sync(Some((&home, OWNER, &scope.config)));
            let text = render(&mut state, [900.0, 1600.0]);
            for required in [
                "Nothing has been created",
                "example/project",
                "work/synthetic",
                "Edit request",
                "Cancel",
                "not executed",
                "Proposed workspace ID (not saved)",
            ] {
                assert!(text.contains(required), "missing {required}");
            }
            if cloud {
                assert!(text.contains("synthetic-volume") && text.contains("459 US cents/hour"));
            }
            for forbidden in [
                "Create task-free worker",
                "Check this setup",
                "I trust this",
                "Authorize creation",
                "Connect",
            ] {
                assert!(!text.contains(forbidden), "unexpected {forbidden}");
            }
            assert!(!home.root().exists());
        }
    }
    #[test]
    fn overview_close_clears_form_and_suppresses_its_late_result() {
        let (_temp, home, scope, state) = fixture(false);
        let mut inventory = super::super::super::RemoteEnvironments {
            open: true,
            request_preview: state,
            ..Default::default()
        };
        let tx = pending(&mut inventory.request_preview, &scope);
        inventory.close();
        assert!(tx.send(Ok(prepared(&home, &scope, false))).is_ok());
        inventory.request_preview.sync(None);
        assert!(!inventory.open && !inventory.request_preview.is_active() && !home.root().exists());
    }
}
