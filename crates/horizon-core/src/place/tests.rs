//! A workspace on a machine, judged on the command that its new panels run.

use crate::board::panel_restore_options;
use crate::machines::{Machine, MachineRegistry, Machines};
use crate::panel::{PanelKind, PanelOptions};
use crate::runtime_state::PanelState;
use crate::ssh::MachineKeys;
use crate::util::stand_in_program;

use super::*;

/// A workspace local id, as Horizon makes one.
const WORKSPACE: &str = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a";

fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        ..Machine::default()
    }
}

/// A Horizon home of a test, the keys of that home, and a list that keeps
/// `build-1` with the key `build-1`.
fn home() -> (tempfile::TempDir, MachineKeys, Machines) {
    let temp = tempfile::tempdir().expect("a temporary home");
    let home = crate::horizon_home::HorizonHome::from_root(temp.path().join("horizon"));
    let machines = Machines::with_records(MachineRegistry::from_home(&home), vec![record("build-1")])
        .with_key("build-1", "build-1");
    (temp, MachineKeys::from_home(&home), machines)
}

fn shell() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Shell,
        ..PanelOptions::default()
    }
}

#[test]
fn a_saved_workspace_without_a_place_is_this_computer() {
    let state: crate::runtime_state::WorkspaceState =
        serde_json::from_str(r#"{"local_id":"ws-1","name":"one"}"#).expect("a workspace of an older Horizon");
    assert_eq!(
        state.place,
        WorkspacePlace::Local,
        "a workspace without the field is here"
    );
}

#[test]
fn a_place_is_saved_as_the_machine_name_and_this_computer_saves_nothing() {
    let state = crate::runtime_state::WorkspaceState {
        local_id: "ws-1".to_owned(),
        name: "one".to_owned(),
        place: WorkspacePlace::Machine("build-1".to_owned()),
        ..crate::runtime_state::WorkspaceState::default()
    };
    let saved = serde_json::to_string(&state).expect("the workspace saves");
    assert!(
        saved.contains(r#""place":"build-1""#),
        "the place is the machine name: {saved}"
    );
    let loaded: crate::runtime_state::WorkspaceState = serde_json::from_str(&saved).expect("the workspace loads");
    assert_eq!(loaded.place, WorkspacePlace::Machine("build-1".to_owned()));

    let local = serde_json::to_string(&crate::runtime_state::WorkspaceState {
        place: crate::place::WorkspacePlace::default(),
        local_id: "ws-2".to_owned(),
        name: "two".to_owned(),
        ..crate::runtime_state::WorkspaceState::default()
    })
    .expect("the workspace saves");
    assert!(
        !local.contains("place"),
        "a workspace here writes no place at all: {local}"
    );
}

#[test]
fn a_workspace_on_this_computer_keeps_the_shell_of_the_person() {
    let (_temp, keys, machines) = home();
    let mut options = shell();
    place_command(
        &mut options,
        &WorkspacePlace::Local,
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect("this computer always accepts a new terminal");
    assert_eq!(options.kind, PanelKind::Shell);
    assert!(options.ssh_connection.is_none(), "no ssh is made here");
    assert!(options.machine.is_none());
    assert!(options.machine_session.is_none());
}

#[test]
fn an_agent_or_a_card_is_not_moved_to_the_machine() {
    let (_temp, keys, machines) = home();
    for kind in [
        PanelKind::Claude,
        PanelKind::Browser,
        PanelKind::Device,
        PanelKind::Block,
        PanelKind::Editor,
    ] {
        let mut options = PanelOptions {
            kind,
            ..PanelOptions::default()
        };
        place_command(
            &mut options,
            &WorkspacePlace::Machine("build-1".to_owned()),
            &machines,
            &keys,
            &PlaceWorkspace {
                local_id: WORKSPACE,
                sessions: &[],
            },
        )
        .expect("only a terminal follows the place");
        assert_eq!(options.kind, kind, "{kind:?} keeps its own command");
        assert!(
            options.ssh_connection.is_none(),
            "{kind:?} opens nothing on the machine"
        );
        assert!(options.machine_session.is_none());
    }
}

#[test]
fn a_new_terminal_of_a_placed_workspace_runs_ssh_into_its_own_session() {
    let (_temp, keys, machines) = home();
    let mut options = shell();
    place_command(
        &mut options,
        &WorkspacePlace::Machine("build-1".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect("the machine accepts the terminal");

    assert_eq!(options.kind, PanelKind::Ssh, "the terminal runs on the machine");
    assert_eq!(options.machine.as_deref(), Some("build-1"));
    assert_eq!(options.machine_session.as_deref(), Some("hz-c8dfe3af-1"));
    assert_eq!(options.command, None, "no local command runs");
    assert!(options.args.is_empty());

    let directory = keys.key_directory();
    let remote = "tmux -u new-session -A -s hz-c8dfe3af-1 \\; set-option status off";
    let expected = vec![
        "-i".to_owned(),
        directory.join("build-1").display().to_string(),
        "-o".to_owned(),
        "IdentitiesOnly=yes".to_owned(),
        "-o".to_owned(),
        "IdentityAgent=none".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={}", directory.join("build-1.known_hosts").display()),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=10".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=5".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
        "-t".to_owned(),
        "root@203.0.113.10".to_owned(),
        remote.to_owned(),
    ];
    let args = options
        .ssh_connection
        .as_ref()
        .expect("the terminal connects over ssh")
        .to_command_args();
    assert_eq!(
        args, expected,
        "the argv is the key of the machine alone, one terminal, one tmux session"
    );
}

#[test]
fn a_machine_without_a_key_offers_the_keys_of_the_person() {
    let temp = tempfile::tempdir().expect("a temporary home");
    let home = crate::horizon_home::HorizonHome::from_root(temp.path().join("horizon"));
    let machines = Machines::with_records(MachineRegistry::from_home(&home), vec![record("build-2")]);
    let keys = MachineKeys::from_home(&home);
    let mut options = shell();
    place_command(
        &mut options,
        &WorkspacePlace::Machine("build-2".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect("the machine accepts the terminal");
    let args = options
        .ssh_connection
        .as_ref()
        .expect("the terminal connects over ssh")
        .to_command_args();
    assert!(!args.iter().any(|argument| argument == "-i"), "{args:?}");
    assert!(!args.iter().any(|argument| argument.starts_with("IdentitiesOnly")));
    assert_eq!(
        args.last().map(String::as_str),
        Some("tmux -u new-session -A -s hz-c8dfe3af-1 \\; set-option status off")
    );
}

#[test]
fn the_first_terminal_of_a_workspace_takes_one_and_the_next_takes_the_next_free() {
    let (_temp, keys, machines) = home();
    let place = WorkspacePlace::Machine("build-1".to_owned());
    let session = |sessions: &[&str]| {
        let mut options = shell();
        place_command(
            &mut options,
            &place,
            &machines,
            &keys,
            &PlaceWorkspace {
                local_id: WORKSPACE,
                sessions,
            },
        )
        .expect("the machine accepts the terminal");
        options.machine_session
    };
    assert_eq!(session(&[]).as_deref(), Some("hz-c8dfe3af-1"));
    assert_eq!(session(&["hz-c8dfe3af-1"]).as_deref(), Some("hz-c8dfe3af-2"));
    assert_eq!(
        session(&["hz-c8dfe3af-1", "hz-c8dfe3af-3"]).as_deref(),
        Some("hz-c8dfe3af-2"),
        "the smallest free number, not the last one plus one"
    );
    assert_eq!(
        session(&["hz-0123abcd-1", "hz-c8dfe3af-1"]).as_deref(),
        Some("hz-c8dfe3af-2"),
        "another workspace on the same machine takes no number of this one"
    );
}

#[test]
fn the_session_prefix_is_eight_hex_of_the_workspace() {
    assert_eq!(session_prefix(WORKSPACE), "hz-c8dfe3af");
    assert_eq!(session_prefix("C8DFE3AF-1111"), "hz-c8dfe3af", "in lower case");
    assert_eq!(session_prefix(WORKSPACE), session_prefix(WORKSPACE), "stable");
    let hashed = session_prefix("workspace one");
    assert_eq!(hashed.len(), "hz-".len() + 8, "{hashed}");
    assert!(
        hashed
            .trim_start_matches("hz-")
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "{hashed} is eight lower-case hexadecimal characters"
    );
    assert_eq!(
        hashed,
        session_prefix("workspace one"),
        "a hash does not change between runs"
    );
    assert_ne!(
        hashed,
        session_prefix("workspace two"),
        "two workspaces never share a prefix"
    );
    assert!(crate::machines::session_name(&session_prefix(WORKSPACE)).is_ok());
}

#[test]
fn a_place_whose_machine_is_gone_is_machine_unavailable_and_makes_no_panel() {
    let (_temp, keys, machines) = home();
    let mut options = shell();
    let error = place_command(
        &mut options,
        &WorkspacePlace::Machine("gone-1".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect_err("the machine is not in the list");
    assert_eq!(error.code(), MACHINE_UNAVAILABLE);
    assert_eq!(error.code(), "machine_unavailable");
    assert!(matches!(error, PlaceError::MachineUnavailable { .. }));
    assert!(
        error.to_string().contains("gone-1"),
        "the reason names the machine: {error}"
    );
    assert_eq!(
        options.kind,
        PanelKind::Shell,
        "the options of the caller are untouched"
    );
    assert!(options.ssh_connection.is_none());
    assert!(options.machine.is_none());
}

#[test]
fn a_record_that_fails_its_rule_makes_no_panel() {
    let temp = tempfile::tempdir().expect("a temporary home");
    let home = crate::horizon_home::HorizonHome::from_root(temp.path().join("horizon"));
    let mut broken = record("build-3");
    broken.host = "-oProxyCommand=touch /var/tmp/hz-place".to_owned();
    let machines = Machines::with_records(MachineRegistry::from_home(&home), vec![broken]);
    let keys = MachineKeys::from_home(&home);
    let mut options = shell();
    let error = place_command(
        &mut options,
        &WorkspacePlace::Machine("build-3".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect_err("the route is not one ssh option");
    assert_eq!(error.code(), "invalid_ssh_route");
    assert_eq!(options.kind, PanelKind::Shell);
}

#[test]
fn a_restored_terminal_attaches_with_the_same_argv() {
    let (_temp, keys, machines) = home();
    let mut options = shell();
    place_command(
        &mut options,
        &WorkspacePlace::Machine("build-1".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect("the machine accepts the terminal");
    let running = options
        .ssh_connection
        .as_ref()
        .expect("the terminal connects over ssh")
        .to_command_args();

    // What Horizon saves of the panel, and what it restores from it.
    let saved = PanelState {
        local_id: "panel-1".to_owned(),
        name: "Terminal 2".to_owned(),
        kind: options.kind,
        ssh_connection: options.ssh_connection.clone(),
        machine: options.machine.clone(),
        machine_session: options.machine_session.clone(),
        ..PanelState::default()
    };
    let restored = panel_restore_options(
        &saved,
        Some(std::path::Path::new("/var/tmp/transcripts")),
        &crate::browser::BrowserConfig::default(),
    );
    assert!(
        !restored.restore_as_disconnected_snapshot,
        "a terminal of a machine runs ssh again instead of showing a frozen screen"
    );
    assert_eq!(restored.machine_session.as_deref(), Some("hz-c8dfe3af-1"));
    assert_eq!(restored.machine.as_deref(), Some("build-1"));
    let again = restored
        .ssh_connection
        .as_ref()
        .expect("the saved connection comes back")
        .to_command_args();
    assert_eq!(
        again, running,
        "a restart runs the same command and re-attaches to the same session"
    );
}

#[test]
fn the_kill_of_a_session_is_the_same_argv_with_the_kill_command() {
    let (_temp, keys, machines) = home();
    let mut options = shell();
    place_command(
        &mut options,
        &WorkspacePlace::Machine("build-1".to_owned()),
        &machines,
        &keys,
        &PlaceWorkspace {
            local_id: WORKSPACE,
            sessions: &[],
        },
    )
    .expect("the machine accepts the terminal");
    let connection = options.ssh_connection.clone().expect("the terminal connects over ssh");
    let running = connection.to_command_args();
    let kill = SessionKill::over(&connection, "hz-c8dfe3af-1");

    assert_eq!(kill.program, "ssh");
    assert_eq!(kill.session, "hz-c8dfe3af-1");
    assert_eq!(kill.args.len(), running.len(), "the same command line, one command");
    assert_eq!(
        &kill.args[..kill.args.len() - 1],
        &running[..running.len() - 1],
        "the transport of the panel is the transport of the kill"
    );
    assert_eq!(
        kill.args.last().map(String::as_str),
        Some("tmux kill-session -t hz-c8dfe3af-1")
    );
}

#[test]
fn a_kill_that_does_not_end_in_its_time_limit_is_stopped() {
    let slow = SessionKill {
        session: "hz-c8dfe3af-1".to_owned(),
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), "sleep 30".to_owned()],
    };
    let started = std::time::Instant::now();
    slow.run_within(Duration::from_millis(120));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the kill of a session that does not answer is bounded, not the panel of the person"
    );
}

#[test]
fn a_kill_that_ends_at_once_is_quiet() {
    let quick = SessionKill {
        session: "hz-c8dfe3af-1".to_owned(),
        program: stand_in_program().to_owned(),
        args: Vec::new(),
    };
    quick.run_within(Duration::from_secs(5));
}

#[test]
fn a_place_of_a_machine_named_an_empty_string_is_this_computer() {
    assert_eq!(WorkspacePlace::from(Some(String::new())), WorkspacePlace::Local);
    assert_eq!(WorkspacePlace::from(Some("  ".to_owned())), WorkspacePlace::Local);
    assert_eq!(WorkspacePlace::from(None), WorkspacePlace::Local);
    assert_eq!(
        WorkspacePlace::from(Some("build-1".to_owned())),
        WorkspacePlace::Machine("build-1".to_owned())
    );
}

#[test]
fn the_remote_command_of_a_place_keeps_the_tmux_status_line_out_of_the_panel() {
    assert_eq!(
        session_command("hz-c8dfe3af-1", None),
        "tmux -u new-session -A -s hz-c8dfe3af-1 \\; set-option status off"
    );
    assert_eq!(kill_command("hz-c8dfe3af-1"), "tmux kill-session -t hz-c8dfe3af-1");
}
