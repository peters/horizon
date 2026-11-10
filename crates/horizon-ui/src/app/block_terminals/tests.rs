//! The SSH terminal an agent opens, judged on a test board.

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::browser::manifest;
use horizon_core::{RuntimeState, StartupDecision};

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    // No live agent process is needed to exercise the caller's identity.
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    (temp, ctx, app)
}

fn route(host: &str, user: Option<&str>, port: Option<i64>) -> TerminalRoute {
    TerminalRoute {
        host: host.into(),
        user: user.map(str::to_string),
        port,
    }
}

fn request_from(app: &HorizonApp, panel: usize, operation: block::Operation) -> block::Request {
    block::Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[panel].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

fn request(app: &HorizonApp, operation: block::Operation) -> block::Request {
    request_from(app, 0, operation)
}

fn open_terminal(title: Option<&str>, command: Option<&str>) -> block::Operation {
    block::Operation::OpenTerminal {
        ssh: Some(route("lab-1.invalid", Some("deploy"), Some(2222))),
        remote_command: command.map(str::to_string),
        title: title.map(str::to_string),
        key: None,
        machine: None,
    }
}

fn opened(outcome: block::Outcome) -> String {
    match outcome {
        block::Outcome::Opened { panel_id } => panel_id,
        other => panic!("expected an opened terminal: {other:?}"),
    }
}

fn code(outcome: &block::Outcome) -> &str {
    match outcome {
        block::Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn an_opened_terminal_lands_in_the_callers_workspace_without_taking_focus() {
    let (_temp, ctx, mut app) = app();
    let workspace = app.board.panels[0].workspace_id;
    let other_workspace = app.board.panels[1].workspace_id;
    app.board.focus(app.board.panels[1].id);
    let before = app.board.panels.len();

    let panel_id =
        opened(app.apply_block_request(&request(&app, open_terminal(None, Some("tmux new -A -s build"))), &ctx));

    assert_eq!(app.board.panels.len(), before + 1);
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("terminal"))
        .expect("terminal");
    assert_eq!(panel.workspace_id, workspace);
    assert_ne!(panel.workspace_id, other_workspace);
    assert_eq!(panel.kind, PanelKind::Ssh);
    assert!(panel.visible, "a new terminal is visible");
    assert_eq!(
        panel.opened_by(),
        Some("Claude"),
        "the titlebar marks the terminal with the agent that opened it"
    );
    assert_eq!(
        panel.title, "deploy@lab-1.invalid",
        "a terminal without a title is named after its route"
    );
    assert_eq!(
        panel.ssh_connection.as_ref().map(SshConnection::to_command_args),
        Some(vec![
            "-p".to_string(),
            "2222".to_string(),
            "-o".to_string(),
            "ServerAliveInterval=15".to_string(),
            "-o".to_string(),
            "ServerAliveCountMax=1".to_string(),
            "-t".to_string(),
            "deploy@lab-1.invalid".to_string(),
            "tmux new -A -s build".to_string(),
        ]),
        "Horizon builds the ssh command line itself, and the command is last"
    );
    assert_eq!(
        app.board.focused,
        Some(app.board.panels[1].id),
        "opening a terminal for the person must not steal keyboard input"
    );
}

#[test]
fn a_title_is_one_trimmed_line_and_a_plain_shell_needs_no_tty() {
    let (_temp, ctx, mut app) = app();
    let long = format!("  {}  ", "build\nbox".repeat(20));
    let panel_id = opened(app.apply_block_request(
        &request(
            &app,
            block::Operation::OpenTerminal {
                ssh: Some(route("lab-1.invalid", None, None)),
                remote_command: None,
                title: Some(long),
                key: None,
                machine: None,
            },
        ),
        &ctx,
    ));
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("terminal"))
        .expect("terminal");
    assert_eq!(
        panel.title, "build",
        "the title keeps one line, trimmed to 80 characters"
    );
    let args = panel
        .ssh_connection
        .as_ref()
        .map(SshConnection::to_command_args)
        .expect("the terminal keeps its connection");
    assert!(
        !args.iter().any(|argument| argument == "-t"),
        "a plain shell allocates its own terminal: {args:?}"
    );
    assert_eq!(args.last().map(String::as_str), Some("lab-1.invalid"));
}

#[test]
fn an_agent_opens_at_most_eight_terminals_in_one_run() {
    let (_temp, ctx, mut app) = app();
    for _ in 0..block::MAX_TERMINALS_PER_OWNER {
        opened(app.apply_block_request(&request(&app, open_terminal(None, None)), &ctx));
    }
    let before = app.board.panels.len();
    let refusal = app.apply_block_request(&request(&app, open_terminal(None, None)), &ctx);
    assert_eq!(code(&refusal), "too_many_terminals");
    assert_eq!(app.board.panels.len(), before, "a refused terminal opens no panel");

    // Another agent of the same workspace has its own allowance.
    let other = app.apply_block_request(&request_from(&app, 1, open_terminal(None, None)), &ctx);
    assert!(matches!(other, block::Outcome::Opened { .. }), "{other:?}");
}

#[test]
fn the_terminal_lands_in_the_callers_own_workspace() {
    let (_temp, ctx, mut app) = app();
    let second_workspace = app.board.create_workspace("elsewhere");
    app.board.panels[1].workspace_id = second_workspace;
    let first = app.board.panels[0].workspace_id;

    let panel_id = opened(app.apply_block_request(&request_from(&app, 1, open_terminal(None, None)), &ctx));
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("terminal"))
        .expect("terminal");
    assert_eq!(panel.workspace_id, second_workspace);
    assert_ne!(panel.workspace_id, first);
}

#[test]
fn a_route_or_a_command_that_is_not_plain_is_refused_without_a_panel() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    for (operation, expected) in [
        (
            block::Operation::OpenTerminal {
                ssh: Some(route("-oProxyCommand=touch /tmp/horizon-c5", None, None)),
                remote_command: None,
                title: None,
                key: None,
                machine: None,
            },
            "invalid_ssh_route",
        ),
        (
            block::Operation::OpenTerminal {
                ssh: Some(route("lab-1.invalid", Some("-L2222"), None)),
                remote_command: None,
                title: None,
                key: None,
                machine: None,
            },
            "invalid_ssh_route",
        ),
        (
            block::Operation::OpenTerminal {
                ssh: Some(route("lab-1.invalid", None, Some(0))),
                remote_command: None,
                title: None,
                key: None,
                machine: None,
            },
            "invalid_ssh_route",
        ),
        (
            block::Operation::OpenTerminal {
                ssh: Some(route("lab-1.invalid", None, None)),
                remote_command: Some("tmux new; id".into()),
                title: None,
                key: None,
                machine: None,
            },
            "invalid_remote_command",
        ),
        (
            block::Operation::OpenTerminal {
                ssh: Some(route("lab-1.invalid", None, None)),
                remote_command: Some("-oProxyCommand=id".into()),
                title: None,
                key: None,
                machine: None,
            },
            "invalid_remote_command",
        ),
    ] {
        assert_eq!(
            code(&app.apply_block_request(&request(&app, operation), &ctx)),
            expected
        );
    }
    assert_eq!(app.board.panels.len(), before, "a refused route opens no panel");
}

#[test]
fn the_mark_of_an_opened_terminal_is_saved_with_the_panel_and_survives_a_restore() {
    let (_temp, ctx, mut app) = app();
    let panel_id = opened(app.apply_block_request(&request(&app, open_terminal(None, None)), &ctx));
    let id = app.board.panel_id_by_local_id(&panel_id).expect("terminal");

    let saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    let state = saved.workspaces[0]
        .panels
        .iter()
        .find(|panel| panel.local_id == panel_id)
        .expect("the saved terminal");
    assert_eq!(state.opened_by.as_deref(), Some("Claude"));
    assert_eq!(
        state
            .to_panel_options(&horizon_core::browser::BrowserConfig::default())
            .opened_by,
        Some("Claude".to_string()),
        "a restored terminal keeps its mark"
    );

    // A snapshot that a previous Horizon wrote holds no such field.
    let mut older = serde_json::to_value(state).expect("the saved panel");
    older.as_object_mut().expect("an object").remove("opened_by");
    let loaded: horizon_core::PanelState = serde_json::from_value(older).expect("an older panel loads");
    assert_eq!(loaded.opened_by, None);
    assert_eq!(app.board.panels.iter().filter(|panel| panel.id == id).count(), 1);
}
