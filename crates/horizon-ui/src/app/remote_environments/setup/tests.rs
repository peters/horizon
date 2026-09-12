use super::*;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_OWNER: &str = "00000000-0000-4000-8000-000000000002";

fn respond(sender: &mpsc::SyncSender<Result<Completion, String>>, completion: Completion) {
    assert!(sender.send(Ok(completion)).is_ok());
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

#[test]
fn no_owning_session_or_other_pending_work_cannot_open_form() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.remote_environments.open = true;
    app.remote_workspace_setup_action(InventoryAction::WorkspaceSetup(Action::New), &Context::default());
    assert!(!app.remote_environments.setup.is_active());
    let (_sender, rx) = mpsc::sync_channel(1);
    app.remote_environments.pending = Some(super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    assert!(!app.remote_environments.setup_idle());
}

#[test]
fn entry_action_requires_persistent_linux_owner_and_invalidates_on_session_change() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    app.remote_environments.open = true;
    app.template_config.remote = form::tests::config();
    app.active_session = Some(crate::app::ActiveSession {
        session_id: OWNER.into(),
        lease: None,
        last_lease_refresh: None,
        persistent: false,
    });
    app.remote_workspace_setup_action(InventoryAction::WorkspaceSetup(Action::New), &ctx);
    assert!(!app.remote_environments.setup.is_active());
    app.active_session.as_mut().expect("session").persistent = true;
    app.remote_workspace_setup_action(InventoryAction::WorkspaceSetup(Action::New), &ctx);
    assert_eq!(app.remote_environments.setup.is_active(), cfg!(target_os = "linux"));
    app.active_session.as_mut().expect("session").session_id = OTHER_OWNER.into();
    app.remote_workspace_setup_action(InventoryAction::None, &ctx);
    assert!(!app.remote_environments.setup.is_active());
    let (_sender, rx) = mpsc::sync_channel(1);
    app.remote_environments.pending = Some(super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    app.remote_workspace_setup_action(InventoryAction::WorkspaceSetup(Action::New), &ctx);
    assert!(!app.remote_environments.setup.is_active());
    assert!(!app.session_store.home().cloud_workflow_store_path().exists());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{app::test_support::raw_input, test_egui::DiscardTextures};

    fn fixture(cloud: bool) -> (tempfile::TempDir, HorizonHome, Scope, SetupState) {
        let temp = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(temp.path().join("must-not-be-created"));
        let scope = super::recovery::scope(&home);
        let state = SetupState {
            form: Some(form::tests::populated(cloud)),
            scope: Some(scope.clone()),
            ..Default::default()
        };
        (temp, home, scope, state)
    }
    pub(super) fn prepared(home: &HorizonHome, scope: &Scope, cloud: bool) -> PreparedRemoteWorkspaceSetup {
        api::preview_configured_remote_workspace(
            home,
            &scope.config,
            OWNER,
            form::tests::populated(cloud).draft(i64::MAX).expect("draft"),
        )
        .expect("preview")
    }
    fn texts(output: &egui::FullOutput) -> impl Iterator<Item = &egui::epaint::TextShape> {
        output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text),
            _ => None,
        })
    }
    fn render(state: &mut SetupState, size: [f32; 2]) -> String {
        let ctx = Context::default();
        let output = ctx
            .run_ui(raw_input(size, None), |ui| {
                egui::CentralPanel::default().show(ui, |ui| state.show(ui, &mut InventoryAction::None));
            })
            .discard_textures();
        texts(&output)
            .map(|text| text.galley.job.text.as_str())
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
            super::recovery::settle(&mut state, &scope);
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
            let tx = pending(&mut state, &scope, None);
            let mut config = scope.config.clone();
            let mut owner = OWNER;
            let other_home = HorizonHome::from_root(home.root().join("other"));
            let mut current_home = &home;
            match mode {
                0 => state.action(Action::Cancel, &home, OWNER, &scope.config, &Context::default()),
                1 => config.local_docker[0].docker_host = "unix:///changed.sock".into(),
                2 => owner = OTHER_OWNER,
                3 => current_home = &other_home,
                _ => state.invalidate(),
            }
            respond(&tx, Completion::Preview(Box::new(prepared(&home, &scope, false))));
            state.sync(Some((current_home, owner, &config)));
            assert!(state.review.is_none() && !state.is_active());
            state.sync(Some((&home, OWNER, &scope.config)));
            assert!(!state.is_active() && !home.root().exists());
        }
    }
    #[test]
    fn disconnected_preview_has_definite_noncreating_notice_and_cancel_clears_it() {
        let (_temp, home, scope, mut state) = fixture(false);
        drop(pending(&mut state, &scope, None));
        state.sync(Some((&home, OWNER, &scope.config)));
        assert_eq!(
            state.notice.as_deref(),
            Some("Request review or check could not finish. No creation was requested.")
        );
        assert!(state.review.is_none() && state.pending.is_none());
        state.invalidate();
        assert!(!state.is_active() && !home.root().exists());
    }
    #[test]
    fn exact_summary_and_narrow_form_offer_consent_only_after_review() {
        for cloud in [false, true] {
            let (_temp, home, scope, mut state) = fixture(cloud);
            let form = render(&mut state, [390.0, 1000.0]);
            assert!(form.contains("Nothing has been created") && form.contains("Review request"));
            assert!(!form.contains("Create task-free worker") && !form.contains("I trust this"));
            let tx = pending(&mut state, &scope, None);
            respond(&tx, Completion::Preview(Box::new(prepared(&home, &scope, cloud))));
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
                for required in [
                    "12.8",
                    "CUDA allowlist",
                    "synthetic-dc",
                    "overridden by HPS",
                    "8080/http",
                    "37 GB",
                    "unused with HPS",
                    "123",
                    "234",
                    "345",
                    "minDisk (raw)",
                    "synthetic-registration",
                ] {
                    assert!(text.contains(required), "missing RunPod value {required}");
                }
            }
            for forbidden in ["Check this setup", "Authorize creation", "Connect"] {
                assert!(!text.contains(forbidden), "unexpected {forbidden}");
            }
            assert!(text.contains("Create task-free worker") && text.contains("I trust this"));
            assert!(!state.consent && !home.root().exists());
        }
    }
    #[test]
    fn overview_close_clears_form_and_suppresses_its_late_result() {
        let (_temp, home, scope, state) = fixture(false);
        let mut inventory = super::super::super::RemoteEnvironments {
            open: true,
            setup: state,
            ..Default::default()
        };
        let tx = pending(&mut inventory.setup, &scope, None);
        inventory.close();
        respond(&tx, Completion::Preview(Box::new(prepared(&home, &scope, false))));
        inventory.setup.sync(None);
        assert!(!inventory.open && !inventory.setup.is_active() && !home.root().exists());
    }

    #[test]
    fn profile_selection_works_inside_actual_modal_without_false_loading() {
        let (_temp, home, _, state) = fixture(true);
        let mut inventory = super::super::super::RemoteEnvironments {
            setup: state,
            ..Default::default()
        };
        let ctx = Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut frame = |events| {
            let mut input = raw_input([900.0, 900.0], None);
            input.events = events;
            ctx.run_ui(input, |ui| {
                super::super::super::paint::show(ui.ctx(), &mut inventory);
            })
            .discard_textures()
        };
        for label in ["RunPod / gpu", "Local Docker / local"] {
            frame(Vec::new());
            let output = frame(Vec::new());
            assert!(!texts(&output).any(|text| text.galley.job.text.contains("Loading saved inventory")));
            let text = texts(&output)
                .find(|text| text.galley.job.text == label)
                .expect("visible profile control");
            let position = text.pos + text.galley.size() / 2.0;
            for pressed in [true, false] {
                frame(vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }
        let form = inventory.setup.form.as_ref().expect("form");
        let provider = form.draft(i64::MAX).expect("draft").target.provider;
        assert_eq!(provider, horizon_core::cloud_run::CloudProvider::LocalDocker);
        assert!(!home.root().exists() && inventory.pending.is_none() && inventory.setup.pending.is_none());
    }

    #[test]
    fn confirmation_rejects_home_owner_or_config_drift_before_dispatch() {
        for mode in 0..3 {
            let (_temp, home, scope, mut state) = fixture(false);
            state.review = Some(paint::Review::new(prepared(&home, &scope, false), &scope.config));
            state.consent = true;
            let other = HorizonHome::from_root(home.root().join("other"));
            let mut config = scope.config.clone();
            if mode == 2 {
                config.local_docker[0].docker_host = "unix:///changed.sock".into();
            }
            state.action(
                Action::Confirm,
                if mode == 0 { &other } else { &home },
                if mode == 1 { OTHER_OWNER } else { OWNER },
                &config,
                &Context::default(),
            );
            assert!(state.pending.is_none() && state.attempts.is_empty() && !home.root().exists());
        }
    }

    #[test]
    fn submitted_result_matrix_preserves_only_real_uncertainty_and_prior_history() {
        use api::ConfiguredWorkspaceSetupError as Error;
        let (_temp, home, scope, _) = fixture(false);
        let prepared = prepared(&home, &scope, false);
        let locator = prepared.locator().clone();
        // A local saved allocation supplies the success channel type, not a provider result.
        let store = horizon_core::cloud_run::CloudWorkflowStore::open(&home).expect("store");
        let spec = horizon_core::remote_workspace::RemoteWorkspaceState::new(prepared.spec().clone()).expect("state");
        let saved = store.create_remote_workspace(OWNER, &spec).expect("saved");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        for error in [
            None,
            Some(Error::UnsupportedPlatform),
            Some(Error::UnsupportedProvider),
            Some(Error::InvalidRequest),
            Some(Error::InvalidProfile),
            Some(Error::ContextChanged),
            Some(Error::ConsentMismatch),
            Some(Error::SaveConflict),
            Some(Error::CredentialUnavailable),
            Some(Error::StorageUnavailable),
            Some(Error::SetupUnconfirmed),
        ] {
            for discard in [false, true] {
                for prior in [false, true] {
                    let mut state = SetupState {
                        attempts: vec![locator.clone()],
                        unknown: prior,
                        ..Default::default()
                    };
                    let sender = pending(&mut state, &scope, Some(locator.clone()));
                    respond(
                        &sender,
                        Completion::Submitted(Box::new(api::ConfiguredWorkspaceSetupAttempt {
                            locator: locator.clone(),
                            result: error.map_or_else(|| Ok(allocation.clone()), Err),
                        })),
                    );
                    if discard {
                        state.invalidate();
                    }
                    state.sync(Some((&home, OWNER, &scope.config)));
                    let uncertain = matches!(error, Some(Error::StorageUnavailable | Error::SetupUnconfirmed));
                    assert_eq!(state.unknown, prior || uncertain || (discard && error.is_none()));
                    assert!(state.pending.is_none() && state.attempts == vec![locator.clone()]);
                }
            }
        }
    }

    #[test]
    fn actual_expired_preview_refuses_before_store_and_never_claims_billing_uncertainty() {
        let (_temp, home, scope, _) = fixture(false);
        let now = || {
            i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_millis(),
            )
            .expect("millis")
        };
        let expiry = now().checked_add(2000).expect("expiry");
        let prepared = api::preview_configured_remote_workspace(
            &home,
            &scope.config,
            OWNER,
            form::tests::populated(false).draft(expiry).expect("draft"),
        )
        .expect("preview");
        let consent = api::RemoteWorkspaceSetupConsent::LocalDocker {
            image: prepared.spec().target.image.clone(),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while now() <= expiry {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let attempt = api::submit_configured_remote_workspace(&home, &scope.config, OWNER, prepared, consent);
        assert!(
            matches!(attempt.result, Err(api::ConfiguredWorkspaceSetupError::InvalidRequest)) && !home.root().exists()
        );
        let mut state = SetupState {
            attempts: vec![attempt.locator.clone()],
            ..Default::default()
        };
        let sender = pending(&mut state, &scope, Some(attempt.locator.clone()));
        state.invalidate();
        respond(&sender, Completion::Submitted(Box::new(attempt)));
        state.sync(None);
        assert!(!state.unknown && state.pending.is_none() && !home.root().exists());
    }
}

mod recovery;
