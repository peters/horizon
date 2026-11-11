//! A machine terminal that reaches its tmux session again after a lost link.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{
    Machine, MachineRegistry, Machines, PanelId, PanelKind, PanelOptions, RuntimeState, SshConnection,
    SshConnectionStatus, StartupDecision, WorkspaceId, WorkspacePlace, WorkspaceState,
};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::test_app_with_startup;

/// The workspace local id of the test, as Horizon makes one: its first eight
/// hexadecimal characters name the tmux sessions of its panels.
const WORKSPACE: &str = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a";

fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        ..Machine::default()
    }
}

/// A Horizon whose one workspace is on `build-1`, with the machine list that
/// keeps that machine.
fn app_on_a_machine() -> (tempfile::TempDir, Context, HorizonApp, WorkspaceId) {
    let runtime = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: WORKSPACE.to_owned(),
            name: "work".to_owned(),
            place: WorkspacePlace::Machine("build-1".to_owned()),
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    settle_machine_load(&mut app);
    app.machine_list = Machines::with_records(
        MachineRegistry::from_home(app.session_store.home()),
        vec![record("build-1")],
    )
    .with_key("build-1", "build-1");
    let workspace = app.board.workspaces[0].id;
    (temp, ctx, app, workspace)
}

/// The options of a command that ends at once with `code`, in place of the
/// program of the panel, so the test starts no ssh.
fn ends_with(code: u8) -> PanelOptions {
    PanelOptions {
        launch_wrapper: vec!["/bin/sh".to_owned(), "-c".to_owned(), format!("exit {code}")],
        ..PanelOptions::default()
    }
}

fn terminal_of_the_machine(app: &mut HorizonApp, workspace: WorkspaceId, code: u8) -> PanelId {
    let panel = app
        .create_panel_with_options(ends_with(code), workspace)
        .expect("a terminal of the machine");
    // The frame that creates a panel asks a worker for its preparation.
    app.settle_panel_starts();
    panel
}

/// Reads the panels until the stand-in command of `panel_id` ended, so the test
/// waits for no frame.
fn wait_for_the_end(app: &mut HorizonApp, panel_id: PanelId) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        app.board.process_output();
        let ended = app
            .board
            .panel(panel_id)
            .is_some_and(|panel| panel.ssh_status() == Some(SshConnectionStatus::Disconnected));
        if ended {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the stand-in command of the panel did not end");
}

fn status(app: &HorizonApp, panel_id: PanelId) -> Option<SshConnectionStatus> {
    app.board.panel(panel_id).and_then(horizon_core::Panel::ssh_status)
}

#[test]
fn a_lost_link_of_a_machine_terminal_reconnects_after_the_first_delay() {
    let (_temp, ctx, mut app, workspace) = app_on_a_machine();
    let panel_id = terminal_of_the_machine(&mut app, workspace, 255);
    wait_for_the_end(&mut app, panel_id);
    let now = Instant::now();
    let mut restarted = Vec::new();

    app.poll_reconnects_with(&ctx, now, |_, panel| restarted.push(panel));

    assert!(restarted.is_empty(), "the first attempt waits");
    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Reconnecting),
        "the panel says that it works on the link again"
    );
    assert_eq!(
        app.reconnects.next_due(),
        Some(now + Duration::from_secs(3)),
        "the first attempt is due after 3 s"
    );

    app.board.process_output();
    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Reconnecting),
        "a frame that reads the ended process again keeps the waiting state"
    );

    app.poll_reconnects_with(&ctx, now + Duration::from_secs(2), |_, panel| restarted.push(panel));
    assert!(restarted.is_empty(), "nothing starts before the due time");

    app.poll_reconnects_with(&ctx, now + Duration::from_secs(3), |_, panel| restarted.push(panel));
    assert_eq!(
        restarted,
        [panel_id],
        "the panel runs its own command again at the due time"
    );
    assert!(!app.reconnects.is_waiting(panel_id), "one attempt is due once");
}

#[test]
fn the_attempts_of_one_panel_grow_the_delay_of_its_next_loss() {
    use horizon_core::place::{AfterConnectionLoss, ConnectionLoss, after_connection_loss};

    use super::ReconnectWaits;

    let mut waits = ReconnectWaits::default();
    let panel_id = PanelId(1);
    let now = Instant::now();
    let mut delays = Vec::new();
    for _ in 0..5 {
        let AfterConnectionLoss::Reconnect { attempt, after } = after_connection_loss(ConnectionLoss {
            exit_code: Some(horizon_core::place::SSH_CONNECTION_FAILURE),
            connected_for: None,
            machine_tied: true,
            closed_by_the_person: false,
            attempts: waits.attempts(panel_id),
        }) else {
            panic!("a lost link of a machine terminal waits for an attempt");
        };
        delays.push(after.as_secs());
        waits.arm(panel_id, attempt, now + after);
        waits.due(now + after);
    }
    assert_eq!(delays, [3, 6, 12, 24, 30], "3 s, 6 s, 12 s, 24 s, then every 30 s");
}

#[test]
fn a_command_that_ended_does_not_reconnect_and_no_key_starts_it() {
    let (_temp, ctx, mut app, workspace) = app_on_a_machine();
    let panel_id = terminal_of_the_machine(&mut app, workspace, 0);
    wait_for_the_end(&mut app, panel_id);

    app.poll_reconnects_with(&ctx, Instant::now(), |_, _| {});

    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Disconnected),
        "a tmux session that ended stays ended"
    );
    assert_eq!(app.reconnects.next_due(), None, "nothing waits");
}

#[test]
fn a_terminal_of_a_plain_ssh_host_does_not_reconnect() {
    let (_temp, ctx, mut app, _workspace) = app_on_a_machine();
    let workspace = app.board.create_workspace("plain");
    let mut options = ends_with(255);
    options.kind = PanelKind::Ssh;
    options.ssh_connection = Some(SshConnection {
        host: "203.0.113.9".to_owned(),
        ..SshConnection::default()
    });
    let panel_id = app
        .create_panel_with_options(options, workspace)
        .expect("a terminal of a host of the remote hosts chooser");
    wait_for_the_end(&mut app, panel_id);

    app.poll_reconnects_with(&ctx, Instant::now(), |_, _| {});

    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Disconnected),
        "a plain SSH terminal has no tmux session to reach again"
    );
    assert_eq!(app.reconnects.next_due(), None, "nothing waits");
}

#[test]
fn a_terminal_with_a_key_and_a_tmux_command_reconnects_without_a_record() {
    let (_temp, ctx, mut app, _workspace) = app_on_a_machine();
    let workspace = app.board.create_workspace("agent");
    let mut options = ends_with(255);
    options.kind = PanelKind::Ssh;
    options.ssh_connection = Some(SshConnection {
        host: "203.0.113.10".to_owned(),
        remote_command: Some("tmux new -A -s build".to_owned()),
        ..SshConnection::default()
    });
    let panel_id = app
        .create_panel_with_options(options, workspace)
        .expect("the terminal that an agent opened with a machine key");
    app.settle_panel_starts();
    wait_for_the_end(&mut app, panel_id);
    let now = Instant::now();
    let mut restarted = Vec::new();

    app.poll_reconnects_with(&ctx, now, |_, panel| restarted.push(panel));
    app.poll_reconnects_with(&ctx, now + Duration::from_secs(3), |_, panel| restarted.push(panel));

    assert_eq!(
        restarted,
        [panel_id],
        "the command attaches its own session, so it reaches the same work again"
    );
}

#[test]
fn closing_a_panel_that_waits_stops_its_attempts() {
    let (_temp, ctx, mut app, workspace) = app_on_a_machine();
    let panel_id = terminal_of_the_machine(&mut app, workspace, 255);
    wait_for_the_end(&mut app, panel_id);
    let now = Instant::now();
    app.poll_reconnects_with(&ctx, now, |_, _| {});
    assert!(app.reconnects.is_waiting(panel_id), "the panel waits");

    app.close_panel(panel_id);

    assert!(!app.reconnects.is_waiting(panel_id), "a closed panel waits for nothing");
    let mut restarted = Vec::new();
    app.poll_reconnects_with(&ctx, now + Duration::from_secs(30), |_, panel| restarted.push(panel));
    assert!(restarted.is_empty(), "a closed panel starts nothing");
}

#[test]
fn a_machine_that_was_forgotten_ends_the_attempts_of_its_terminal() {
    let (_temp, ctx, mut app, workspace) = app_on_a_machine();
    let panel_id = terminal_of_the_machine(&mut app, workspace, 255);
    wait_for_the_end(&mut app, panel_id);
    app.machine_list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), Vec::new());

    app.poll_reconnects_with(&ctx, Instant::now(), |_, _| {});

    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Disconnected),
        "the last words of ssh stay, and Horizon stops trying"
    );
    assert_eq!(
        app.reconnects.next_due(),
        None,
        "nothing waits for a machine that is gone"
    );
}

#[test]
fn the_mark_of_a_waiting_terminal_reads_reconnecting() {
    assert_eq!(SshConnectionStatus::Reconnecting.label(), "Reconnecting");
    assert_eq!(SshConnectionStatus::Connecting.label(), "Connecting...");
}

#[test]
fn a_panel_that_waits_names_the_attempt_that_it_reached() {
    let (_temp, ctx, mut app, workspace) = app_on_a_machine();
    let panel_id = terminal_of_the_machine(&mut app, workspace, 255);
    wait_for_the_end(&mut app, panel_id);

    app.poll_reconnects_with(&ctx, Instant::now(), |_, _| {});
    let attempt = app
        .board
        .panel(panel_id)
        .and_then(horizon_core::Panel::ssh_status_attempt);
    assert_eq!(attempt, Some(1), "the first attempt of a lost link");

    // A person's key press connects at once, and the attempts start again, so
    // a third attempt is reached only by a link that keeps failing.
    app.poll_reconnects_with(&ctx, Instant::now() + Duration::from_secs(60), |app, panel_id| {
        if let Some(panel) = app.board.panel_mut(panel_id) {
            panel.show_reconnecting(3);
        }
    });
    assert_eq!(
        app.board
            .panel(panel_id)
            .and_then(horizon_core::Panel::ssh_status_attempt),
        Some(3),
        "the mark of a panel that keeps trying says which attempt it reached"
    );
}

/// The arguments of a stand-in that hangs in its connect, as a network that drops
/// packets does: it writes its process id and then waits for ever. It is the
/// command of the panel, so no test starts an ssh.
fn a_hanging_attempt(marker: &Path) -> Vec<String> {
    vec![
        "-c".to_owned(),
        format!("printf '%s\\n' \"$$\" >> '{}'; exec sleep 300", marker.display()),
    ]
}

/// The process ids that the stand-in of `marker` wrote, in order.
fn attempts(marker: &Path) -> Vec<i32> {
    std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect()
}

/// Waits for the stand-in to write `count` process ids.
fn wait_for_attempts(marker: &Path, count: usize) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let written = attempts(marker);
        if written.len() >= count {
            return written;
        }
        assert!(
            Instant::now() < deadline,
            "the stand-in wrote {} process ids and not {count}",
            written.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the process `pid` is gone, including a child that waits as a zombie
/// for a reap. Linux reports the state of a process in its `/proc` entry, so the
/// test reads it there; macOS has no such file, and the test then proves the new
/// attempt alone.
#[cfg(target_os = "linux")]
fn is_gone(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    stat.split_whitespace().nth(2).is_none_or(|state| state == "Z")
}

#[test]
fn a_key_press_ends_an_attempt_that_is_in_flight_and_starts_a_new_one() {
    let (temp, _ctx, mut app, _workspace) = app_on_a_machine();
    let marker = temp.path().join("attempts.txt");
    let workspace = app.board.create_workspace("machine");
    // An SSH panel whose child hangs, as the `ssh` of a machine terminal hangs in
    // a network that drops its packets. The command of the panel is the stand-in,
    // so the restart of the same panel runs the stand-in again.
    let options = PanelOptions {
        kind: PanelKind::Ssh,
        name: Some("build-1".to_owned()),
        command: Some("/bin/sh".to_owned()),
        args: a_hanging_attempt(&marker),
        ..PanelOptions::default()
    };
    let panel_id = app.create_panel_with_options(options, workspace).expect("the terminal");
    app.settle_panel_starts();
    let first = wait_for_attempts(&marker, 1);
    assert_eq!(first.len(), 1, "one attempt runs");
    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Connecting),
        "the attempt hangs in its connect"
    );

    // The key press of the person ends the attempt and starts the next one now,
    // and not after the delay of the reconnect.
    app.reconnect_panel_now(panel_id);
    app.apply_panel_transitions();
    let both = wait_for_attempts(&marker, 2);

    assert_ne!(both[0], both[1], "a new process runs the attempt");
    #[cfg(target_os = "linux")]
    assert!(
        is_gone(first[0]),
        "the child of the attempt that hung was killed and reaped"
    );
    assert_eq!(
        status(&app, panel_id),
        Some(SshConnectionStatus::Connecting),
        "the new attempt is in flight"
    );
    assert!(!app.reconnects.is_waiting(panel_id), "the panel waits for no delay");
    app.close_panel(panel_id);
}
