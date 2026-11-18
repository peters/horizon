//! The counts of what runs at a close, and the lines that speak of them.

use std::time::{Duration, Instant};

use crate::agents::AgentStatus;
use crate::panel::{PanelKind, PanelOptions};
use crate::workspace::WorkspaceId;
use crate::{Board, RunningPrograms};

use super::WindowCloseSetting;

/// The options of a panel of this computer that runs a program of its own. The
/// program ends by itself after a few seconds, so a test that fails leaves no
/// process behind.
fn program_that_runs(kind: PanelKind) -> PanelOptions {
    PanelOptions {
        kind,
        command: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 10".into()],
        ..PanelOptions::default()
    }
}

/// The options of a panel that runs `command` and ends.
fn program_that_ends(command: &str) -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Command,
        command: Some("/bin/sh".into()),
        args: vec!["-c".into(), command.into()],
        ..PanelOptions::default()
    }
}

/// The line of every count, in both numbers of the words: one program and two.
#[test]
fn the_lines_of_the_ask_count_what_runs() {
    let nothing = RunningPrograms::default();
    assert!(nothing.is_empty(), "nothing runs");
    assert_eq!(nothing.programs_line(), None);
    assert_eq!(nothing.machines_line(), None);
    assert_eq!(nothing.kept_agents_line(), None);
    assert_eq!(nothing.quit_question(), "Quit Horizon?");

    let one = RunningPrograms {
        here: 1,
        ..RunningPrograms::default()
    };
    assert!(!one.is_empty());
    assert_eq!(one.programs_line().as_deref(), Some("1 program is running."));
    assert_eq!(one.quit_question(), "End 1 program and quit?");

    let busy = RunningPrograms {
        here: 5,
        working_agents: 3,
        ..RunningPrograms::default()
    };
    assert_eq!(
        busy.programs_line().as_deref(),
        Some("5 programs are running. 3 agents are working."),
        "the agents that work are the second sentence"
    );
    assert_eq!(busy.quit_question(), "End 5 programs and quit?");

    let one_agent = RunningPrograms {
        here: 2,
        working_agents: 1,
        ..RunningPrograms::default()
    };
    assert_eq!(
        one_agent.programs_line().as_deref(),
        Some("2 programs are running. 1 agent is working.")
    );

    let machines = RunningPrograms {
        on_machines: 2,
        ..RunningPrograms::default()
    };
    assert_eq!(
        machines.machines_line().as_deref(),
        Some("2 panels run on a machine and keep running there.")
    );
    assert_eq!(
        RunningPrograms {
            on_machines: 1,
            ..RunningPrograms::default()
        }
        .machines_line()
        .as_deref(),
        Some("1 panel runs on a machine and keeps running there.")
    );

    let kept = RunningPrograms {
        here: 2,
        kept_agents: 1,
        ..RunningPrograms::default()
    };
    assert_eq!(
        kept.kept_agents_line().as_deref(),
        Some("Agent programs that are kept through a restart come back at the next start.")
    );
    assert_eq!(one_agent.kept_agents_line(), None, "no program is kept");
}

/// The terminal panels of this computer count, with the agents among them; an
/// editor, a card and the panels of a machine do not count as programs here.
#[test]
fn only_the_terminal_panels_of_this_computer_count_as_programs() {
    let mut board = Board::new();
    let workspace = board.create_workspace("work");
    let agent = board
        .create_panel(program_that_runs(PanelKind::Codex), workspace)
        .expect("an agent panel");
    board
        .create_panel(program_that_runs(PanelKind::Shell), workspace)
        .expect("a shell panel");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("an editor panel");
    board
        .create_panel(
            PanelOptions {
                machine: Some("build-1".into()),
                machine_session: Some("hz-11111111-1".into()),
                ..program_that_runs(PanelKind::Ssh)
            },
            workspace,
        )
        .expect("a panel of a machine");
    board.panel_mut(agent).expect("the agent panel").agent_status = AgentStatus::Working;
    // A follow panel holds the terminal of another computer, and it shows nothing
    // of this one: its program is no program here.
    board
        .create_follow_panel(
            workspace,
            crate::terminal::FollowSpec {
                device: "device-1".to_owned(),
                label: "other computer".to_owned(),
                panel: "panel-1".to_owned(),
                title: "review".to_owned(),
                kind: PanelKind::Shell,
                cols: 80,
                rows: 24,
                typing: true,
            },
        )
        .expect("a follow panel");

    let running = board.running_programs(true);
    assert_eq!(running.here, 2, "the agent and the shell run here, and nothing else");
    assert_eq!(running.working_agents, 1, "the agent works");
    assert_eq!(running.kept_agents, 1, "a restart keeps the agent program");
    assert_eq!(running.on_machines, 1, "the panel of the machine runs there");
    assert_eq!(
        board.running_programs(false).kept_agents,
        0,
        "no holder keeps a program when the setting is off"
    );

    board.shutdown_terminal_panels();
}

/// A program that ended is no program that runs: a close then ends at once.
#[test]
fn a_program_that_ended_does_not_count() {
    let mut board = Board::new();
    let workspace: WorkspaceId = board.create_workspace("work");
    let ended = board
        .create_panel(program_that_ends("exit 0"), workspace)
        .expect("a program that ends");
    let deadline = Instant::now() + Duration::from_secs(30);
    while board.panel(ended).expect("the panel").program_end().is_none() {
        assert!(Instant::now() < deadline, "the program of the panel ends");
        let _ = board.process_unread_output();
        std::thread::sleep(Duration::from_millis(5));
    }

    let running = board.running_programs(false);
    assert!(running.is_empty(), "the program of the panel ended: {running:?}");

    board.shutdown_terminal_panels();
}

/// A store of an older build has no row for the choice, so it asks, and every
/// value of the setting reads as itself.
#[test]
fn the_choice_of_the_close_reads_and_round_trips() {
    let older = crate::Config::from_json(r#"{"features": {"sidebar_accordion": true}}"#).expect("a store");
    assert_eq!(older.features.when_window_closes, WindowCloseSetting::Ask);

    for setting in WindowCloseSetting::VALUES {
        let mut config = crate::Config::default();
        config.features.when_window_closes = setting;
        let document = config.to_json().expect("config should serialize");
        let stored = crate::Config::from_json(&document).expect("the store reads");
        assert_eq!(stored.features.when_window_closes, setting, "{setting:?}");
    }
    assert_eq!(
        crate::Config::from_json(r#"{"features": {"when_window_closes": "keep_running"}}"#)
            .expect("config should deserialize")
            .features
            .when_window_closes,
        WindowCloseSetting::KeepRunning
    );
}
