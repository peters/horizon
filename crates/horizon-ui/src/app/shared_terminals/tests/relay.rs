//! The join with the real relay: two windows, each with a sync link and a live
//! link to one server, through a stand-in `ssh` that relays to its socket.
//!
//! The stand-in is the one of the supervisor tests of `horizon-sync`: an `sh`
//! program that runs a small Python relay, so no test opens a connection.

use std::path::Path;
use std::sync::Arc;

use egui::Context;
use horizon_core::secret::{CredentialVault, SecretVault};
use horizon_core::secret_store::SecretStore;
use horizon_core::{PanelId, RuntimeState, StartupDecision};
use horizon_sync::{ServerInfo, Store, Supervisor, SupervisorConfig, run_server_on_unix_socket};

use super::{OFFSCREEN, PROGRAM, above_the_viewport, follower_of, frames_until, grid, text};
use crate::app::HorizonApp;
use crate::app::server_sync::ServerSync;
use crate::app::test_support::test_app_with_startup;

/// The relay of one stand-in `ssh`: the bytes of its standard input and output
/// go to the socket of the server in both directions.
const RELAY: &str = "\
import os
import socket
import sys
import threading

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])


def outward():
    try:
        while True:
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            sock.sendall(data)
    except Exception:
        pass
    try:
        sock.shutdown(socket.SHUT_WR)
    except Exception:
        pass


threading.Thread(target=outward, daemon=True).start()
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except Exception:
    pass
";

/// A memory store for the values that a computer of a test never holds.
fn memory() -> Arc<SecretStore> {
    SecretStore::in_memory().expect("a store in memory")
}

/// The stand-in `ssh` of every computer of a test, which relays to `socket`.
///
/// It is written once and before any link starts: a link forks its child, and a
/// fork while another thread writes a script keeps that script busy.
pub(super) fn stand_in(directory: &Path, socket: &Path) -> String {
    std::fs::write(directory.join("relay.py"), RELAY).expect("the relay of the stand-in");
    let script = format!(
        "exec /usr/bin/env python3 '{}' '{}'",
        directory.join("relay.py").display(),
        socket.display()
    );
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// One window with the name `label`, whose computer is `device` on the server
/// that `program` reaches. Its live link starts with its sync link, as in a
/// real start.
pub(super) fn computer(program: &str, device: &str, label: &str) -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let workspace = app.board.create_workspace("Work");
    app.board.focus_workspace(workspace);

    let directory = temp.path().join("link");
    std::fs::create_dir_all(&directory).expect("the directory of the link");
    horizon_sync::write_device_name(&directory, device).expect("the name of this computer");

    let credentials = Arc::new(CredentialVault::new(Box::new(memory())));
    credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
    let named = Arc::new(horizon_core::named_secrets::NamedSecretVault::new(Box::new(memory())));
    let keys = horizon_core::MachineKeys::new(temp.path().join("keys"));
    let secrets = Arc::new(SecretVault::new(keys, Arc::clone(&credentials), named));
    let mut config = SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", &directory, secrets)
        .expect("the settings of the link");
    config.program = program.to_owned();
    let supervisor = Supervisor::start(config.clone(), || {}).expect("the sync link");
    let mut sync = ServerSync::for_test(supervisor, credentials);
    sync.live = Some(config);
    app.machine_load_rx = None;
    app.server_sync = Some(sync);
    app.template_config.server.computer_name = label.to_owned();
    (temp, ctx, app)
}

/// A terminal panel of `app` that runs the program of the join tests.
pub(super) fn owner(app: &mut HorizonApp) -> PanelId {
    let workspace = app.board.active_workspace.expect("the workspace of the test");
    let options = horizon_core::PanelOptions {
        kind: horizon_core::PanelKind::Command,
        name: Some("build".to_owned()),
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), PROGRAM.to_owned()],
        position: Some(OFFSCREEN),
        size: Some([600.0, 300.0]),
        ..horizon_core::PanelOptions::default()
    };
    app.board.create_panel(options, workspace).expect("the owner terminal")
}

#[test]
fn a_follower_reads_the_lines_that_the_owner_wrote_before_it_came() {
    let home = tempfile::tempdir().expect("the home of the server");
    let socket = home.path().join("server.sock");
    let store = Store::open(&home.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket");

    let program = stand_in(home.path(), &socket);
    let (_temp_a, ctx_a, mut a) = computer(&program, "desk-a", "Desk A");
    let (_temp_b, ctx_b, mut b) = computer(&program, "desk-b", "Desk B");
    let panel = owner(&mut a);
    let id = a.board.panel(panel).expect("the owner panel").local_id.clone();
    frames_until(
        "B lists the panel of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| {
            let computers = &apps[1].shared_terminals.found.computers;
            computers.iter().any(|computer| computer.device == "desk-a")
        },
    );

    // A writes sixty lines before any computer follows the panel.
    let lines: String = (1..=60).fold(String::new(), |text, line| format!("{text}line {line}\n"));
    let written = a
        .board
        .panel_mut(panel)
        .expect("the owner panel")
        .write_input(lines.as_bytes());
    assert!(written, "the panel takes the lines");
    frames_until(
        "A shows the last line",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| text(apps[0], panel).contains("GOT:line 60"),
    );

    // B follows it: the lines of the program that ran before the join are above
    // the viewport of the follower, and its viewport shows the newest ones.
    b.open_shared_terminal("desk-a", &id, &ctx_b);
    let follow = follower_of(&b).expect("the follow panel of B");
    frames_until(
        "B shows the screen of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| text(apps[1], follow).contains("GOT:line 60"),
    );
    let above = above_the_viewport(&b, follow);
    assert!(
        above.iter().any(|line| line == "GOT:line 1"),
        "the first line is above the viewport of the follower: {} lines",
        above.len()
    );

    drop((a, b));
    server.stop();
    store.close();
}

#[test]
fn a_panel_of_one_computer_is_listed_followed_shown_and_typed_into_at_the_other() {
    let home = tempfile::tempdir().expect("the home of the server");
    let socket = home.path().join("server.sock");
    let store = Store::open(&home.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket");

    let program = stand_in(home.path(), &socket);
    let (_temp_a, ctx_a, mut a) = computer(&program, "desk-a", "Desk A");
    let (_temp_b, ctx_b, mut b) = computer(&program, "desk-b", "Desk B");
    let panel = owner(&mut a);
    let id = a.board.panel(panel).expect("the owner panel").local_id.clone();

    // The panel of A is in the list of B, under the name of A.
    frames_until(
        "B lists the panel of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| {
            let computers = &apps[1].shared_terminals.found.computers;
            computers.iter().any(|computer| {
                computer.device == "desk-a"
                    && computer.label == "Desk A"
                    && computer.panels.iter().any(|listed| listed.id == id)
            })
        },
    );
    assert!(
        b.shared_terminals.found.computers.iter().all(|computer| !computer.own),
        "B does not list itself"
    );

    // B follows it: the screen arrives, and what B types reaches the program of A.
    b.open_shared_terminal("desk-a", &id, &ctx_b);
    let follow = follower_of(&b).expect("the follow panel of B");
    frames_until(
        "B shows the screen of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| text(apps[1], follow).contains("READY"),
    );
    let typed = b
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .write_input(b"hello\n");
    assert!(typed, "the follow panel takes the keys");
    frames_until(
        "the echo of A at B",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| text(apps[0], panel).contains("GOT:hello") && text(apps[1], follow).contains("GOT:hello"),
    );

    // Each side says in the title bar what the other one does. The chip of the
    // owner names the computer that typed, and the grid that it holds for it.
    let chip = super::chip_of;
    assert_eq!(
        chip(&a, panel).as_deref(),
        Some("sized by Desk B \u{00b7} typed by Desk B")
    );
    frames_until(
        "the chip of the owner names the follower again",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| chip(apps[0], panel).as_deref() == Some("sized by Desk B \u{00b7} seen by Desk B"),
    );
    assert_eq!(chip(&b, follow).as_deref(), Some("on Desk A"));

    // A takes another name. Its live link starts again under that name, and
    // the follow panel of B follows the panel again by itself.
    a.template_config.server.computer_name = "Desk A2".to_owned();
    frames_until("B follows A again", &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)], |apps| {
        let ended = apps[1]
            .board
            .panel(follow)
            .and_then(horizon_core::Panel::follow_ended_line);
        chip(apps[1], follow).as_deref() == Some("on Desk A2")
            && ended.is_none()
            && chip(apps[0], panel).as_deref() == Some("seen by Desk B")
    });
    let typed = b
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .write_input(b"again\n");
    assert!(typed, "the follow panel takes the keys");
    frames_until(
        "the echo of A at B",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| text(apps[1], follow).contains("GOT:again"),
    );

    // A stops sharing: the stream of B ends and the list of B is empty.
    a.template_config.server.share_terminals = false;
    frames_until(
        "the stream of B ends",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| {
            let ended = apps[1]
                .board
                .panel(follow)
                .and_then(horizon_core::Panel::follow_ended_line);
            let computers = &apps[1].shared_terminals.found.computers;
            ended.is_some() && computers.iter().all(|computer| computer.panels.is_empty())
        },
    );

    drop((a, b));
    server.stop();
    store.close();
}

/// The live link starts again when the sync link went away and came back: the
/// server goes away, both sync links read it, and the live links of the windows
/// follow their lists again when it returns.
#[test]
fn the_live_link_starts_again_when_the_sync_link_comes_back() {
    let home = tempfile::tempdir().expect("the home of the server");
    let socket = home.path().join("server.sock");
    let store = Store::open(&home.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket");

    let program = stand_in(home.path(), &socket);
    let (_temp_a, ctx_a, mut a) = computer(&program, "desk-a", "Desk A");
    let (_temp_b, ctx_b, mut b) = computer(&program, "desk-b", "Desk B");
    let panel = owner(&mut a);
    let id = a.board.panel(panel).expect("the owner panel").local_id.clone();
    frames_until(
        "B lists the panel of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| lists(apps[1], &id),
    );

    // The server goes away: no live link reads a frame from it any more.
    server.stop();
    frames_until(
        "both live links are down",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| apps.iter().all(|app| app.shared_terminals.found.down.is_some()),
    );

    // It comes back on the same socket. The sync link of each window comes back
    // by itself, and the live link of each one starts again with it.
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket again");
    frames_until(
        "B lists the panel of A again",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| apps.iter().all(|app| app.shared_terminals.found.down.is_none()) && lists(apps[1], &id),
    );

    drop((a, b));
    server.stop();
    store.close();
}

/// Whether the list of `app` names the panel `id` of the computer `desk-a`.
fn lists(app: &HorizonApp, id: &str) -> bool {
    let computers = &app.shared_terminals.found.computers;
    computers
        .iter()
        .any(|computer| computer.device == "desk-a" && computer.panels.iter().any(|listed| listed.id == id))
}

/// The program of A takes the grid of the window of B, which typed there. C, which
/// only watches, takes the new grid and keeps its history; the person of A then
/// types, and the size of that panel comes back.
/// Mutation: drop the relay of the request, send the screens of a new grid to the
/// follower that asked alone, or keep the hold after the owner typed.
#[test]
fn the_program_of_the_owner_takes_the_grid_of_the_computer_that_typed() {
    let home = tempfile::tempdir().expect("the home of the server");
    let socket = home.path().join("server.sock");
    let store = Store::open(&home.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket");

    let program = stand_in(home.path(), &socket);
    let (_temp_a, ctx_a, mut a) = computer(&program, "desk-a", "Desk A");
    let (_temp_b, ctx_b, mut b) = computer(&program, "desk-b", "Desk B");
    let (_temp_c, ctx_c, mut c) = computer(&program, "desk-c", "Desk C");
    let panel = owner(&mut a);
    let id = a.board.panel(panel).expect("the owner panel").local_id.clone();
    frames_until(
        "B and C list the panel of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| lists(apps[1], &id) && lists(apps[2], &id),
    );

    // The program of A writes the lines that are the history of a follower.
    let lines: String = (1..=60).fold(String::new(), |text, line| format!("{text}line {line}\n"));
    a.board
        .panel_mut(panel)
        .expect("the owner panel")
        .write_input(lines.as_bytes());

    // Two computers follow the panel, and the body of B fits another grid.
    b.open_shared_terminal("desk-a", &id, &ctx_b);
    let follow_b = follower_of(&b).expect("the follow panel of B");
    frames_until(
        "C lists the panel of A again",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| lists(apps[2], &id),
    );
    c.open_shared_terminal("desk-a", &id, &ctx_c);
    let follow_c = follower_of(&c).expect("the follow panel of C");
    frames_until(
        "B and C show the screen of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| text(apps[1], follow_b).contains("GOT:line 60") && text(apps[2], follow_c).contains("GOT:line 60"),
    );
    b.board
        .panel_mut(follow_b)
        .expect("the follow panel")
        .resize_layout([1000.0, 650.0]);
    frames_until(
        "B fits another grid",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| super::size::fit(apps[1], follow_b) != (80, 24),
    );
    let asked = super::size::fit(&b, follow_b);

    // The person of B types, so the program of A takes the grid of the window of
    // B. Both sides show that grid, and the program prints it.
    b.board
        .panel_mut(follow_b)
        .expect("the follow panel")
        .write_input(b"size\n");
    frames_until(
        "A holds the grid of B, and C shows it",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| super::size::shown(apps[0], panel) == (asked.1, asked.0) && grid(apps[2], follow_c) == asked,
    );
    b.board
        .panel_mut(follow_b)
        .expect("the follow panel")
        .write_input(b"size\n");
    frames_until(
        "the program of A reads the grid of B",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| super::size::size(apps[0], panel) == Some((asked.1, asked.0)),
    );
    assert_eq!(grid(&b, follow_b), asked, "B shows the grid of its own body");
    let above = above_the_viewport(&c, follow_c);
    assert!(
        above.iter().any(|line| line == "GOT:line 1"),
        "C keeps its history under a new grid: {} lines",
        above.len()
    );

    // The person of A types: the size of the panel of A comes back, and the
    // program reads it again.
    a.board
        .panel_mut(panel)
        .expect("the owner panel")
        .write_input(b"size\n");
    frames_until(
        "the program of A reads the grid of its own body",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| super::size::size(apps[0], panel) == Some((24, 80)),
    );
    frames_until(
        "B and C show the grid of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b), (&ctx_c, &mut c)],
        |apps| grid(apps[1], follow_b) == (80, 24) && grid(apps[2], follow_c) == (80, 24),
    );
    assert_eq!(
        super::size::shown(&a, panel),
        (24, 80),
        "the owner pushes the grid of its body"
    );

    drop((a, b, c));
    server.stop();
    store.close();
}
