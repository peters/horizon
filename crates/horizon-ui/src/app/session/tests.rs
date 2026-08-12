use std::collections::HashSet;
use std::time::Duration;

use super::{
    DynamicPanelBindingState, HorizonApp, STARTUP_BOOTSTRAP_FAILURE_REPAINT_INTERVAL, StartupBootstrap,
    StartupBootstrapFailure, StartupBootstrapFailureAction, StartupBootstrapOutcome, StartupBootstrapValidationFailure,
    collect_dynamic_binding_updates,
};
use egui::Context;
use horizon_core::{
    AgentSessionBinding, Config, HorizonHome, PanelId, PanelKind, PanelOptions, PanelResume, PanelState, RuntimeState,
    SessionLease, SessionStore, StartupDecision, WorkspaceState,
};
use tempfile::TempDir;

use crate::input;

mod provider_scoping;

fn test_app() -> (TempDir, HorizonApp) {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.yaml");
    let session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        config_path.clone(),
    );
    let config = Config::default();
    let ctx = Context::default();
    let app = HorizonApp::new_with_egui_context(
        &ctx,
        &config,
        config_path,
        session_store,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
        input::ObservedKeyboardInputs::default(),
    );
    (temp, app)
}

fn test_persistent_recovery_app(runtime_state: RuntimeState) -> (TempDir, HorizonApp, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.yaml");
    let session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        config_path.clone(),
    );
    let pending_runtime_state = runtime_state.clone();
    let mut session = session_store
        .create_session_from_runtime(runtime_state)
        .expect("create persistent session");
    let runtime_path = session.runtime_state_path.clone();
    // Construct the app without starting the real provider-catalog worker;
    // these tests inject the failed pre-open state below.
    session.runtime_state = RuntimeState::default();
    let ctx = Context::default();
    let mut app = HorizonApp::new_with_egui_context(
        &ctx,
        &Config::default(),
        config_path,
        session_store,
        StartupDecision::Open {
            disposition: horizon_core::SessionOpenDisposition::Resume,
            session: Box::new(session),
        },
        input::ObservedKeyboardInputs::default(),
    );
    assert!(app.startup_receiver.is_none());
    app.pending_startup_runtime_state = Some(pending_runtime_state);
    (temp, app, runtime_path)
}

#[cfg(windows)]
fn exiting_command() -> (String, Vec<String>) {
    (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), "exit".to_string(), "0".to_string()],
    )
}

#[cfg(not(windows))]
fn exiting_command() -> (String, Vec<String>) {
    ("/bin/sh".to_string(), vec!["-c".to_string(), "exit 0".to_string()])
}

#[test]
fn runtime_state_needs_bootstrap_for_unbound_last_agent_panel() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            cwd: None,
            position: None,
            template: None,
            layout: None,
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Claude".to_string(),
                kind: PanelKind::Claude,
                resume: PanelResume::Last,
                ..PanelState::default()
            }],
        }],
        ..RuntimeState::default()
    };

    assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn runtime_state_needs_bootstrap_for_unbound_last_opencode_panel() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            cwd: None,
            position: None,
            template: None,
            layout: None,
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "OpenCode".to_string(),
                kind: PanelKind::OpenCode,
                resume: PanelResume::Last,
                ..PanelState::default()
            }],
        }],
        ..RuntimeState::default()
    };

    assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn runtime_state_needs_bootstrap_for_unbound_last_pi_panel() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            cwd: None,
            position: None,
            template: None,
            layout: None,
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Pi".to_string(),
                kind: PanelKind::Pi,
                resume: PanelResume::Last,
                ..PanelState::default()
            }],
        }],
        ..RuntimeState::default()
    };

    assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn runtime_state_needs_bootstrap_for_a_persisted_codex_binding() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            cwd: None,
            position: None,
            template: None,
            layout: None,
            panels: vec![
                PanelState {
                    local_id: "fresh".to_string(),
                    name: "Shell".to_string(),
                    kind: PanelKind::Shell,
                    resume: PanelResume::Fresh,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "bound".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Fresh,
                    session_binding: Some(horizon_core::AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-9".to_string(),
                        None,
                        None,
                        None,
                    )),
                    ..PanelState::default()
                },
            ],
        }],
        ..RuntimeState::default()
    };

    assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn runtime_state_needs_bootstrap_for_an_explicit_codex_session() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                resume: PanelResume::Session {
                    session_id: "session-9".to_string(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn runtime_state_skips_bootstrap_for_a_bound_non_codex_panel() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "OpenCode".to_string(),
                kind: PanelKind::OpenCode,
                resume: PanelResume::Last,
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::OpenCode,
                    "session-9".to_string(),
                    None,
                    None,
                    None,
                )),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(!HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn disconnected_bootstrap_enters_a_stable_failed_state() {
    let (_temp, mut app) = test_app();
    let (tx, rx) = std::sync::mpsc::channel();
    app.startup_receiver = Some(rx);
    app.pending_startup_runtime_state = Some(RuntimeState::default());
    drop(tx);

    assert!(!app.poll_startup_bootstrap());
    assert!(matches!(
        app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::WorkerDisconnected)
    ));
    assert!(app.startup_receiver.is_none());
    assert!(app.pending_startup_runtime_state.is_some());
    assert!(!app.poll_startup_bootstrap());
}

#[test]
fn failed_startup_bootstrap_keeps_a_slow_repaint() {
    let ctx = Context::default();
    let (_temp, mut app) = test_app();
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

    let mut repaint_delay = Duration::ZERO;
    for _ in 0..8 {
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(!app.prepare_startup_bootstrap(ctx));
        });
        repaint_delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .repaint_delay;
        if !repaint_delay.is_zero() {
            break;
        }
    }

    assert!(!repaint_delay.is_zero());
    assert!(repaint_delay <= STARTUP_BOOTSTRAP_FAILURE_REPAINT_INTERVAL);
}

#[test]
fn failed_startup_bootstrap_refreshes_a_persistent_session_lease() {
    let ctx = Context::default();
    let (temp, mut app, _runtime_path) = test_persistent_recovery_app(RuntimeState::default());
    let session_id = app
        .active_session
        .as_ref()
        .expect("active persistent session")
        .session_id
        .clone();
    let lease_path = HorizonHome::from_root(temp.path().join(".horizon")).session_lease_path(&session_id);
    let before: SessionLease =
        serde_yaml::from_str(&std::fs::read_to_string(&lease_path).expect("read acquired lease"))
            .expect("parse acquired lease");
    std::thread::sleep(Duration::from_millis(2));
    app.active_session
        .as_mut()
        .expect("active persistent session")
        .last_lease_refresh = None;
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(!app.prepare_startup_bootstrap(ctx));
    });

    let after: SessionLease = serde_yaml::from_str(&std::fs::read_to_string(lease_path).expect("read refreshed lease"))
        .expect("parse refreshed lease");
    assert!(after.last_heartbeat_at > before.last_heartbeat_at);
    assert!(
        app.active_session
            .as_ref()
            .and_then(|session| session.last_lease_refresh)
            .is_some()
    );
}

#[test]
fn failed_bootstrap_can_continue_without_saved_codex_resumes() {
    let (_temp, mut app) = test_app();
    let (command, args) = exiting_command();
    app.pending_startup_runtime_state = Some(RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    });
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

    assert!(app.startup_bootstrap_failure.is_none());
    assert!(app.startup_receiver.is_none());
    assert!(app.pending_startup_runtime_state.is_none());
    let panel = &app.board.panels[0];
    assert!(panel.session_binding.is_none());
    assert!(matches!(panel.resume, PanelResume::Fresh));
}

#[test]
fn partial_validation_failure_keeps_repaired_and_unverified_pending_ids() {
    let (_temp, mut app) = test_app();
    let repaired_runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![
                PanelState {
                    local_id: "repaired".to_string(),
                    name: "Repaired".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Session {
                        session_id: "session-root".to_string(),
                    },
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "unverified".to_string(),
                    name: "Unverified".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Session {
                        session_id: "session-missing".to_string(),
                    },
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(StartupBootstrapOutcome::ExactValidationFailed(Box::new(
        StartupBootstrapValidationFailure {
            runtime_state: repaired_runtime_state,
            message: "one resume could not be verified".to_string(),
            unavailable_exact_session_ids: HashSet::from(["session-missing".to_string()]),
            all_exact_session_ids: false,
            runtime_state_changed: true,
        },
    )))
    .expect("send validation failure");
    app.startup_receiver = Some(rx);
    app.pending_startup_runtime_state = Some(RuntimeState::default());

    assert!(!app.poll_startup_bootstrap());

    let pending = app
        .pending_startup_runtime_state
        .as_ref()
        .expect("pending repaired state");
    assert_eq!(
        pending.workspaces[0].panels[0].stored_session_id(),
        Some("session-root")
    );
    assert_eq!(
        pending.workspaces[0].panels[1].stored_session_id(),
        Some("session-missing")
    );
    assert!(app.pending_startup_runtime_state_changed);
    assert!(matches!(
        &app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::ExactValidationFailed {
            unavailable_exact_session_ids,
            ..
        }) if unavailable_exact_session_ids == &HashSet::from(["session-missing".to_string()])
    ));
}

#[test]
fn persistent_recovery_is_saved_before_the_board_opens() {
    let (command, args) = exiting_command();
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![
                PanelState {
                    local_id: "unverified".to_string(),
                    name: "Unverified".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command.clone()),
                    args: args.clone(),
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "verified".to_string(),
                    name: "Verified".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    resume: PanelResume::Last,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-root".to_string(),
                        None,
                        None,
                        None,
                    )),
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (_temp, mut app, runtime_path) = test_persistent_recovery_app(runtime_state);
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::ExactValidationFailed {
        message: "missing".to_string(),
        unavailable_exact_session_ids: HashSet::from(["session-child".to_string()]),
        all_exact_session_ids: false,
    });

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

    let saved = RuntimeState::load(&runtime_path)
        .expect("load repaired runtime")
        .expect("repaired runtime exists");
    assert!(saved.workspaces[0].panels[0].stored_session_id().is_none());
    assert_eq!(saved.workspaces[0].panels[1].stored_session_id(), Some("session-root"));
    assert!(app.pending_startup_runtime_state.is_none());
    assert!(app.startup_bootstrap_failure.is_none());
    assert!(app.board.panels[0].session_binding.is_none());
}

#[test]
fn failed_recovery_save_keeps_pending_state_until_retry_succeeds() {
    let (command, args) = exiting_command();
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (temp, mut app, runtime_path) = test_persistent_recovery_app(runtime_state);
    let blocked_home = temp.path().join("blocked-home");
    std::fs::write(&blocked_home, "not a directory").expect("create blocked home path");
    app.session_store = SessionStore::new(HorizonHome::from_root(blocked_home), temp.path().join("config.yaml"));
    app.startup_receiver = None;
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::ExactValidationFailed {
        message: "missing".to_string(),
        unavailable_exact_session_ids: HashSet::from(["session-child".to_string()]),
        all_exact_session_ids: false,
    });

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

    assert!(matches!(
        app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::RecoverySaveFailed { .. })
    ));
    assert!(app.pending_startup_runtime_state.is_some());
    assert!(app.board.panels.is_empty());

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::Retry);

    assert!(matches!(
        app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::RecoverySaveFailed { .. })
    ));
    assert!(app.startup_receiver.is_none());
    assert!(app.pending_startup_runtime_state.is_some());
    assert!(app.board.panels.is_empty());

    app.session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        temp.path().join("config.yaml"),
    );
    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::Retry);

    let saved = RuntimeState::load(&runtime_path)
        .expect("load repaired runtime")
        .expect("repaired runtime exists");
    assert!(saved.workspaces[0].panels[0].stored_session_id().is_none());
    assert!(matches!(saved.workspaces[0].panels[0].resume, PanelResume::Fresh));
    assert!(app.pending_startup_runtime_state.is_none());
    assert!(app.startup_bootstrap_failure.is_none());
    assert!(app.board.panels[0].session_binding.is_none());
}

#[test]
fn failed_recovery_save_can_open_the_repaired_state_without_persisting() {
    let (command, args) = exiting_command();
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (temp, mut app, _runtime_path) = test_persistent_recovery_app(runtime_state);
    let blocked_home = temp.path().join("blocked-open-home");
    std::fs::write(&blocked_home, "not a directory").expect("create blocked home path");
    app.session_store = SessionStore::new(HorizonHome::from_root(blocked_home), temp.path().join("config.yaml"));
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::ExactValidationFailed {
        message: "missing".to_string(),
        unavailable_exact_session_ids: HashSet::from(["session-child".to_string()]),
        all_exact_session_ids: false,
    });

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);
    assert!(matches!(
        app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::RecoverySaveFailed { .. })
    ));

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::OpenWithoutSaving);

    assert!(app.startup_bootstrap_failure.is_none());
    assert!(app.pending_startup_runtime_state.is_none());
    assert_eq!(app.board.panels.len(), 1);
    assert!(app.board.panels[0].session_binding.is_none());
    assert!(matches!(app.board.panels[0].resume, PanelResume::Fresh));
    assert!(app.last_session_catalog_refresh.is_none());
    assert!(app.session_catalog_refresh.is_some());
}

#[test]
fn automatic_repair_is_saved_before_the_board_opens() {
    let (command, args) = exiting_command();
    let original_runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                command: Some(command.clone()),
                args: args.clone(),
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (temp, mut app, runtime_path) = test_persistent_recovery_app(original_runtime_state);
    let repaired_runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                resume: PanelResume::Session {
                    session_id: "session-root".to_string(),
                },
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-root".to_string(),
                    None,
                    None,
                    None,
                )),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let blocked_home = temp.path().join("blocked-ready-home");
    std::fs::write(&blocked_home, "not a directory").expect("create blocked home path");
    app.session_store = SessionStore::new(HorizonHome::from_root(blocked_home), temp.path().join("config.yaml"));
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
        runtime_state: repaired_runtime_state,
        session_catalog: horizon_core::AgentSessionCatalog::default(),
        runtime_state_changed: true,
    })))
    .expect("send repaired runtime");
    app.startup_receiver = Some(rx);

    assert!(!app.poll_startup_bootstrap());
    assert!(matches!(
        app.startup_bootstrap_failure,
        Some(StartupBootstrapFailure::RecoverySaveFailed { .. })
    ));
    assert!(app.board.panels.is_empty());
    assert_eq!(
        app.pending_startup_runtime_state
            .as_ref()
            .and_then(|state| state.workspaces[0].panels[0].stored_session_id()),
        Some("session-root")
    );

    app.session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        temp.path().join("config.yaml"),
    );
    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::Retry);

    let saved = RuntimeState::load(&runtime_path)
        .expect("load repaired runtime")
        .expect("repaired runtime exists");
    assert_eq!(saved.workspaces[0].panels[0].stored_session_id(), Some("session-root"));
    assert_eq!(
        app.board.panels[0]
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-root")
    );
    assert!(app.pending_startup_runtime_state.is_none());
    assert!(app.startup_bootstrap_failure.is_none());
}

#[test]
fn runtime_state_skips_bootstrap_for_agents_without_exact_session_catalogs() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            cwd: None,
            position: None,
            template: None,
            layout: None,
            panels: vec![
                PanelState {
                    local_id: "gemini".to_string(),
                    name: "Gemini".to_string(),
                    kind: PanelKind::Gemini,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "kilo".to_string(),
                    name: "KiloCode".to_string(),
                    kind: PanelKind::KiloCode,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
            ],
        }],
        ..RuntimeState::default()
    };

    assert!(!HorizonApp::runtime_state_needs_session_bootstrap(&state));
}

#[test]
fn collect_dynamic_binding_updates_assigns_unbound_panels() {
    let panels = vec![DynamicPanelBindingState {
        panel_id: PanelId(7),
        kind: PanelKind::Codex,
        cwd: "/repo".to_string(),
        launched_at_millis: 10,
        session_binding: None,
        recent_output: false,
    }];
    let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
        assert_eq!(kind, PanelKind::Codex);
        assert_eq!(cwd, Some("/repo"));
        vec![horizon_core::AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-1".to_string(),
            cwd: Some("/repo".to_string()),
            label: None,
            updated_at: 12,
            interactive: true,
        }]
    });

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, PanelId(7));
    assert_eq!(updates[0].1.session_id, "session-1");
}

#[test]
fn collect_dynamic_binding_updates_refreshes_single_recently_active_panel() {
    let panels = vec![DynamicPanelBindingState {
        panel_id: PanelId(7),
        kind: PanelKind::Codex,
        cwd: "/repo".to_string(),
        launched_at_millis: 10,
        session_binding: Some(horizon_core::AgentSessionBinding::new(
            PanelKind::Codex,
            "session-old".to_string(),
            Some("/repo".to_string()),
            None,
            Some(12),
        )),
        recent_output: true,
    }];
    let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
        assert_eq!(kind, PanelKind::Codex);
        assert_eq!(cwd, Some("/repo"));
        vec![
            horizon_core::AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: "session-new".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 20,
                interactive: true,
            },
            horizon_core::AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: "session-old".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 12,
                interactive: true,
            },
        ]
    });

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, PanelId(7));
    assert_eq!(updates[0].1.session_id, "session-new");
}

#[test]
fn collect_dynamic_binding_updates_does_not_reassign_ambiguous_recent_group() {
    let panels = vec![
        DynamicPanelBindingState {
            panel_id: PanelId(7),
            kind: PanelKind::Codex,
            cwd: "/repo".to_string(),
            launched_at_millis: 10,
            session_binding: Some(horizon_core::AgentSessionBinding::new(
                PanelKind::Codex,
                "session-a".to_string(),
                Some("/repo".to_string()),
                None,
                Some(12),
            )),
            recent_output: true,
        },
        DynamicPanelBindingState {
            panel_id: PanelId(8),
            kind: PanelKind::Codex,
            cwd: "/repo".to_string(),
            launched_at_millis: 11,
            session_binding: Some(horizon_core::AgentSessionBinding::new(
                PanelKind::Codex,
                "session-b".to_string(),
                Some("/repo".to_string()),
                None,
                Some(13),
            )),
            recent_output: true,
        },
    ];
    let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
        assert_eq!(kind, PanelKind::Codex);
        assert_eq!(cwd, Some("/repo"));
        vec![horizon_core::AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-c".to_string(),
            cwd: Some("/repo".to_string()),
            label: None,
            updated_at: 20,
            interactive: true,
        }]
    });

    assert!(updates.is_empty());
}

#[test]
fn collect_dynamic_binding_updates_does_not_reassign_claude_bindings() {
    let panels = vec![DynamicPanelBindingState {
        panel_id: PanelId(7),
        kind: PanelKind::Claude,
        cwd: "/repo".to_string(),
        launched_at_millis: 10,
        session_binding: Some(horizon_core::AgentSessionBinding::new(
            PanelKind::Claude,
            "preassigned-session".to_string(),
            Some("/repo".to_string()),
            None,
            Some(12),
        )),
        recent_output: true,
    }];
    let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
        assert_eq!(kind, PanelKind::Claude);
        assert_eq!(cwd, Some("/repo"));
        vec![horizon_core::AgentSessionRecord {
            kind: PanelKind::Claude,
            session_id: "external-newer-session".to_string(),
            cwd: Some("/repo".to_string()),
            label: None,
            updated_at: 20,
            interactive: true,
        }]
    });

    assert!(updates.is_empty());
}

#[test]
fn rebind_and_restart_updates_the_binding_and_queues_the_panel() {
    let (_temp, mut app) = test_app();
    let workspace_id = app.board.create_workspace("test");
    let (command, args) = exiting_command();
    let panel_id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("create agent panel");
    let binding = AgentSessionBinding::new(
        PanelKind::Codex,
        "session-2".to_string(),
        Some("/repo".to_string()),
        Some("Recovered session".to_string()),
        Some(42),
    );

    assert!(app.rebind_and_restart_panel_session(panel_id, binding.clone()));
    assert!(app.rebind_and_restart_panel_session(panel_id, binding.clone()));

    let panel = app.board.panel(panel_id).expect("rebound panel");
    assert_eq!(
        panel.resume,
        PanelResume::Session {
            session_id: "session-2".to_string(),
        }
    );
    assert_eq!(panel.session_binding.as_ref(), Some(&binding));
    assert_eq!(app.panels_to_restart, vec![panel_id]);
}

#[test]
fn rebind_rejects_a_session_used_by_another_panel() {
    let (_temp, mut app) = test_app();
    let workspace_id = app.board.create_workspace("test");
    let (command, args) = exiting_command();
    let occupied = AgentSessionBinding::new(
        PanelKind::Codex,
        "session-2".to_string(),
        Some("/repo".to_string()),
        None,
        Some(42),
    );
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                command: Some(command.clone()),
                args: args.clone(),
                resume: PanelResume::Last,
                session_binding: Some(occupied.clone()),
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("create occupied panel");
    let target_id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("create target panel");

    assert!(!app.rebind_and_restart_panel_session(target_id, occupied));
    assert!(
        app.board
            .panel(target_id)
            .is_some_and(|panel| panel.session_binding.is_none())
    );
    assert!(app.panels_to_restart.is_empty());
}
