use egui::{Event, Key, Modifiers};
use horizon_core::{
    AgentSessionBinding, Config, PanelKind, PanelResume, PanelState, ResolvedSession, RuntimeState, SessionMeta,
    SessionOpenDisposition, StartupDecision,
};

use super::*;

fn agent_runtime_state() -> RuntimeState {
    RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "agents".to_string(),
            name: "Agents".to_string(),
            position: Some([100.0, 80.0]),
            panels: vec![PanelState {
                local_id: "pi-panel".to_string(),
                kind: PanelKind::Pi,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}

fn bound_agent_runtime_state() -> RuntimeState {
    let mut runtime_state = agent_runtime_state();
    let panel = &mut runtime_state.workspaces[0].panels[0];
    panel.resume = PanelResume::Last;
    panel.session_binding = Some(AgentSessionBinding::new(
        PanelKind::Pi,
        "pi-1".to_string(),
        None,
        None,
        None,
    ));
    runtime_state
}

fn resolved_session(runtime_state: RuntimeState) -> ResolvedSession {
    let session_id = "test-resume-all-session".to_string();
    ResolvedSession {
        meta: SessionMeta {
            session_id: session_id.clone(),
            ..SessionMeta::default()
        },
        session_id,
        runtime_state,
        runtime_state_path: std::path::PathBuf::from("/tmp/resume-all-test/runtime.yaml"),
        transcript_root: std::path::PathBuf::from("/tmp/resume-all-test/transcripts"),
    }
}

fn open_startup(runtime_state: RuntimeState) -> StartupDecision {
    StartupDecision::Open {
        disposition: SessionOpenDisposition::Resume,
        session: Box::new(resolved_session(runtime_state)),
    }
}

fn run_key_frame(ctx: &Context, app: &mut HorizonApp, key: Key) {
    let size = [app.window_config.width, app.window_config.height];
    let mut input = raw_input(size, None);
    input.events.push(Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    run_app_frame_with_input(ctx, app, input);
}

#[test]
fn restore_with_agent_panels_queues_resume_all_prompt() {
    let (temp, _ctx, app) = test_app_with_config_and_startup(&Config::default(), open_startup(agent_runtime_state()));

    assert!(app.pending_resume_all.is_some());
    assert!(app.board.workspaces.is_empty());
    assert!(app.active_session.is_none());
    let lease = temp.path().join(".horizon/sessions/test-resume-all-session/lease.json");
    assert!(lease.exists(), "prompt must reserve the session lease while pending");
}

#[test]
fn new_session_disposition_skips_resume_all_prompt() {
    let decision = StartupDecision::Open {
        disposition: SessionOpenDisposition::New,
        session: Box::new(resolved_session(agent_runtime_state())),
    };
    let (_temp, _ctx, app) = test_app_with_config_and_startup(&Config::default(), decision);

    assert!(app.pending_resume_all.is_none());
    assert_eq!(app.board.workspaces.len(), 1);
    assert!(app.active_session.is_some());
}

#[test]
fn bound_agent_panels_skip_resume_all_prompt() {
    let (_temp, _ctx, app) =
        test_app_with_config_and_startup(&Config::default(), open_startup(bound_agent_runtime_state()));

    assert!(app.pending_resume_all.is_none());
    assert!(app.active_session.is_some());
}

#[test]
fn restore_without_agent_panels_skips_resume_all_prompt() {
    let runtime_state = RuntimeState {
        workspaces: vec![editor_workspace_state("notes", [100.0, 80.0])],
        ..RuntimeState::default()
    };
    let (_temp, _ctx, app) = test_app_with_config_and_startup(&Config::default(), open_startup(runtime_state));

    assert!(app.pending_resume_all.is_none());
    assert_eq!(app.board.workspaces.len(), 1);
    assert!(app.active_session.is_some());
}

#[test]
fn resume_all_choice_activates_session_and_rearms_bootstrap() {
    let (_temp, ctx, mut app) =
        test_app_with_config_and_startup(&Config::default(), open_startup(agent_runtime_state()));
    app.theme_applied = true;

    run_key_frame(&ctx, &mut app, Key::Enter);

    assert!(app.pending_resume_all.is_none());
    assert!(app.active_session.is_some());

    // The real bootstrap worker may have delivered its own outcome before we
    // get here; a Ready outcome always rebuilds the board, so driving the
    // final frame from a known catalog outcome stays deterministic either way.
    let mut ready_state = agent_runtime_state();
    ready_state.apply_resume_all_agent_panels();
    ready_state.workspaces[0].panels[0].session_binding = Some(AgentSessionBinding::new(
        PanelKind::Pi,
        "pi-session-1".to_string(),
        None,
        None,
        None,
    ));
    let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
    app.startup_receiver = Some(bootstrap_rx);
    bootstrap_tx
        .send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
            runtime_state: ready_state,
            session_catalog: AgentSessionCatalog::default(),
            runtime_state_changed: false,
        })))
        .expect("bootstrap outcome");
    run_frame_at_configured_size(&ctx, &mut app);

    let panel = app.board.panels.first().expect("restored panel");
    assert!(matches!(panel.resume, PanelResume::Last));
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("pi-session-1")
    );
}

#[test]
fn start_fresh_choice_launches_agents_without_sessions() {
    let (_temp, ctx, mut app) =
        test_app_with_config_and_startup(&Config::default(), open_startup(agent_runtime_state()));
    app.theme_applied = true;

    run_key_frame(&ctx, &mut app, Key::Escape);

    assert!(app.pending_resume_all.is_none());
    assert!(app.active_session.is_some());
    assert!(app.startup_receiver.is_none());
    let panel = app.board.panels.first().expect("restored panel");
    assert!(matches!(panel.resume, PanelResume::Fresh));
    assert!(panel.session_binding.is_none());
}
