//! What the panels of this computer get of a named secret, and what they never get.
//!
//! A panel reads its environment once, when its program starts, so these tests
//! read the text that a program printed of its own environment and the state that a
//! restored panel keeps.

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::exclusive;
use crate::panel::spawn::agent_env;
use crate::place::SessionKill;
use crate::ssh::SshConnection;
use crate::{
    Board, CanvasViewState, Config, PanelId, PanelKind, PanelOptions, PanelState, RuntimeState, StartRequest,
    WindowConfig,
    machines::session_command,
    named_secrets::{PUBLISH_BOUND, SecretName, expect_a_publish, publish},
    prepare_panel_start,
};

const MARKER: &str = "MARKER-4d7e-only-in-an-agent-panel";

fn token() -> SecretName {
    SecretName::parse("HETZNER_TOKEN").expect("a name of a named secret")
}

/// Publishes the marker as one named secret of this computer.
fn publish_marker() {
    publish(BTreeMap::from([(token(), Zeroizing::new(MARKER.to_owned()))]));
}

/// A command that prints the value of the named secret of the test, as a tool that
/// an agent starts reads it. Its own command line holds no value.
fn prints_the_token() -> (Option<String>, Vec<String>) {
    (
        Some("/bin/sh".to_string()),
        vec!["-c".to_string(), "printf 'token:%s\\n' \"$HETZNER_TOKEN\"".to_string()],
    )
}

/// The text of one panel, read until `wanted` is on screen or the wait ends.
fn screen_with(board: &mut Board, panel: PanelId, wanted: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        board.process_output();
        let text = board
            .panel(panel)
            .and_then(crate::Panel::terminal)
            .map_or(String::new(), |terminal| terminal.last_lines_text(24));
        if text.contains(wanted) || Instant::now() >= deadline {
            return text;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_agent_panel_gets_every_named_secret_and_a_terminal_panel_gets_none() {
    let _guard = exclusive();
    publish_marker();

    for kind in [PanelKind::Claude, PanelKind::Codex, PanelKind::Pi, PanelKind::Grok] {
        let env = agent_env(kind, "agent-panel", true);
        assert_eq!(
            env.get("HETZNER_TOKEN").map(String::as_str),
            Some(MARKER),
            "{kind:?} is an agent, so it reads a named secret"
        );
    }
    for kind in [
        PanelKind::Shell,
        PanelKind::Ssh,
        PanelKind::Command,
        PanelKind::Editor,
        PanelKind::Browser,
    ] {
        let env = agent_env(kind, "other-panel", true);
        assert!(
            !env.values().any(|value| value == MARKER),
            "{kind:?} is not an agent, so it reads no named secret"
        );
        assert!(!env.contains_key("HETZNER_TOKEN"));
    }
}

#[test]
fn no_command_line_of_a_machine_and_no_machine_environment_carries_a_named_secret() {
    let _guard = exclusive();
    publish_marker();
    let connection = SshConnection {
        host: "build-vm".to_string(),
        user: Some("deploy".to_string()),
        port: Some(2222),
        remote_command: Some(session_command("hz-1a2b3c4d-1", None)),
        ..SshConnection::default()
    };

    let mut command_lines = vec![
        connection.to_command_args(),
        connection.ssh_transport_args(),
        connection.ssh_probe_transport_args(),
        SessionKill::over(&connection, "hz-1a2b3c4d-1").args,
    ];
    // The interactive shell of a panel keeps the environment of its own start, and
    // that environment holds no named secret of an agent.
    let environment = agent_env(PanelKind::Ssh, "ssh-panel", true);
    command_lines.push(environment.values().cloned().collect());
    for argv in command_lines {
        for argument in argv {
            assert!(!argument.contains(MARKER), "an ssh command line holds no value");
            assert!(
                !argument.contains("HETZNER_TOKEN"),
                "an ssh command line names no secret"
            );
        }
    }
}

#[test]
fn an_agent_panel_starts_with_the_value_and_a_terminal_panel_does_not() {
    let _guard = exclusive();
    publish_marker();
    let mut board = Board::new();
    let workspace = board.create_workspace("secrets");
    let (command, args) = prints_the_token();
    let agent = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Pi,
                command,
                args,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("an agent panel");
    assert!(
        screen_with(&mut board, agent, MARKER).contains(&format!("token:{MARKER}")),
        "the agent panel reads the named secret"
    );

    let terminal = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a terminal panel");
    let text = screen_with(&mut board, terminal, "$");
    assert!(!text.contains(MARKER), "a terminal panel reads no named secret");
    board.shutdown_terminal_panels();
}

#[test]
fn a_saved_and_restored_session_holds_no_value_and_the_restored_panel_gets_it_again() {
    let _guard = exclusive();
    publish_marker();
    let mut board = Board::new();
    let workspace = board.create_workspace("secrets");
    let (command, args) = prints_the_token();
    let agent = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Pi,
                command,
                args,
                name: Some("secrets agent".to_string()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("an agent panel");
    assert!(screen_with(&mut board, agent, MARKER).contains(&format!("token:{MARKER}")));

    // The session that Horizon saves, as the store of this home writes it.
    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let document = state.to_yaml().expect("the session");
    assert!(
        document.contains("secrets agent"),
        "the panel is in the session: {}",
        document.len()
    );
    assert!(!document.contains(MARKER), "a saved session holds no value");

    // A second start reads the session and starts the panel again. The value comes
    // from the store of this computer, never from the file.
    let saved = RuntimeState::from_yaml(&document).expect("the saved session");
    let panel_state: PanelState = saved.workspaces[0]
        .panels
        .iter()
        .find(|panel| panel.local_id == board.panel(agent).expect("the panel").local_id)
        .cloned()
        .expect("the panel of the session");
    assert!(
        !serde_json::to_string(&panel_state).expect("the panel").contains(MARKER),
        "no field of a saved panel holds a value"
    );

    // The restored panel prints the value again, because its start reads the
    // published environment of this process.
    let mut restored = Board::new();
    let workspace = restored.create_workspace("secrets");
    let options = PanelOptions {
        kind: PanelKind::Pi,
        ..panel_state.to_panel_options(&Config::default().browser)
    };
    let panel = restored.create_panel(options, workspace).expect("the restored panel");
    assert!(
        screen_with(&mut restored, panel, MARKER).contains(&format!("token:{MARKER}")),
        "a restored agent panel reads the named secret again"
    );
    board.shutdown_terminal_panels();
    restored.shutdown_terminal_panels();
}

#[test]
fn a_name_whose_value_this_computer_does_not_hold_is_not_injected() {
    let _guard = exclusive();
    // The name is a named secret of the settings, and no value of it is here: the
    // store refused it, or the value of the server has not arrived.
    publish(BTreeMap::new());
    let env = agent_env(PanelKind::Claude, "agent-panel", true);
    assert!(
        !env.contains_key("HETZNER_TOKEN"),
        "a name without a value is no variable"
    );
}

/// The saved board of one agent panel that prints the value of its named secret,
/// as a start restores it: the panel waits for the preparation of a worker.
fn restored_board() -> (RuntimeState, PanelState) {
    let (command, args) = prints_the_token();
    let panel = PanelState {
        local_id: "restored-agent".to_owned(),
        name: "restored agent".to_owned(),
        kind: PanelKind::Pi,
        command,
        args,
        rows: 6,
        cols: 60,
        ..PanelState::default()
    };
    let mut saved = panel.clone();
    saved.local_id = "restored-agent".to_owned();
    (
        RuntimeState {
            workspaces: vec![crate::WorkspaceState {
                local_id: "workspace".to_owned(),
                name: "workspace".to_owned(),
                panels: vec![saved],
                ..crate::WorkspaceState::default()
            }],
            ..RuntimeState::default()
        },
        panel,
    )
}

/// Starts the panels of a restored board the way the coordination worker starts
/// them: the frame asks for the preparation and the answer starts the process.
fn start_waiting_panels(board: &mut Board) {
    for pending in board.take_pending_starts() {
        let request = StartRequest {
            id: pending.id,
            local_id: pending.options.local_id.clone().unwrap_or_default(),
            opts: pending.options.clone(),
            home: crate::HorizonHome::resolve(),
            keep_agents_through_restart: false,
        };
        let start = prepare_panel_start(request);
        board
            .start_panel(pending.id, &pending.options, start)
            .expect("a panel of the restored board starts");
    }
}

#[test]
fn an_agent_panel_of_a_restored_board_gets_the_value_that_the_read_publishes_later() {
    let _guard = exclusive();
    // The worker of this start reads the store, and its answer comes after the
    // board was restored, as it does when the disk is slow: the panel starts
    // before the values of this computer are here.
    expect_a_publish();
    let read = thread::spawn(|| {
        thread::sleep(Duration::from_millis(250));
        publish_marker();
    });

    let (state, panel_state) = restored_board();
    let mut board = Board::from_runtime_state_waiting(&state, None).expect("the restored board");
    start_waiting_panels(&mut board);
    let panel = board
        .panel_id_by_local_id(&panel_state.local_id)
        .expect("the panel of the restored board");
    let text = screen_with(&mut board, panel, MARKER);
    read.join().expect("the read of the store ends");
    assert!(
        text.contains(&format!("token:{MARKER}")),
        "the panel of the restored board waits for the values of this start: {text}"
    );
    board.shutdown_terminal_panels();
}

#[test]
fn an_agent_panel_of_a_start_that_never_answers_starts_after_the_bound_and_says_one_line() {
    let _guard = exclusive();
    // The store does not answer in the bound, so the panel starts without the
    // values of this computer instead of waiting for them.
    expect_a_publish();
    let (state, panel_state) = restored_board();
    let mut board = Board::from_runtime_state_waiting(&state, None).expect("the restored board");
    let asked = Instant::now();
    start_waiting_panels(&mut board);
    let waited = asked.elapsed();
    assert!(
        waited >= PUBLISH_BOUND,
        "the panel waits for the bound of the read of this start: {waited:?}"
    );
    let panel = board
        .panel_id_by_local_id(&panel_state.local_id)
        .expect("the panel of the restored board");
    let text = screen_with(&mut board, panel, "named secrets");
    assert!(
        text.contains("The named secrets of this computer were not read in time"),
        "the panel says that it has no named secret: {text}"
    );
    assert!(
        !text.contains(&format!("token:{MARKER}")),
        "the panel of a store that did not answer reads no value: {text}"
    );
    board.shutdown_terminal_panels();
}
