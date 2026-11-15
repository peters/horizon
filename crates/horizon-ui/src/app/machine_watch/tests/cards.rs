//! The card sockets of a machine follow the `tmux` sessions of that machine, and not
//! the rows of its agents: a session whose agent program ends and starts again keeps
//! its socket and its child.

use std::path::PathBuf;
use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::MachineWatch;

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{run_app_frame, test_app};

use super::snapshot;
use super::support::{Server, Ssh, record, wait_for, with_machines};

/// The socket here of `session` of `machine`, which the supervisor binds.
fn card_socket(app: &HorizonApp, machine: &str, session: &str) -> PathBuf {
    horizon_core::node_cards::local_dir(&app.node_cards.base)
        .join(machine)
        .join(format!("{session}.sock"))
}

/// One machine whose cards are set up, over a private `tmux` server and the stand-in
/// `ssh` of the test.
fn watched_cards(server: &Server, ssh: &Ssh) -> (tempfile::TempDir, HorizonApp, Context) {
    let (temp, mut app) = test_app();
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    app.node_cards.program = ssh.program();
    let _ = server;
    with_machines(
        &mut app,
        vec![horizon_core::Machine {
            cards: true,
            ..record("build-1")
        }],
    );
    (temp, app, Context::default())
}

#[test]
fn the_card_sockets_follow_the_sessions_of_a_machine_and_not_its_agents() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(ssh) = Ssh::new(&server) else {
        return;
    };
    let (_temp, mut app, ctx) = watched_cards(&server, &ssh);
    // A session whose program is no agent, with no state in its pane: it holds no row,
    // and it gets a card socket all the same.
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "hz-shell",
        "-x",
        "100",
        "-y",
        "20",
        "sleep",
        "300",
    ]);
    let socket = card_socket(&app, "build-1", "hz-shell");
    wait_for(&ctx, &mut app, "the card child of the machine", |_| ssh.forwards() == 1);
    wait_for(&ctx, &mut app, "the socket of a session with no agent", |_| {
        socket.exists()
    });
    assert!(
        snapshot(&app)
            .machine("build-1")
            .is_some_and(|held| held.sessions.contains(&"hz-shell".to_owned())),
        "the snapshot names the session of the machine"
    );
    assert!(
        snapshot(&app).agents("build-1").is_empty(),
        "the report of the machine holds no row"
    );

    // The same session reports a state, as a job of a person or a hook of an agent
    // does: a row appears, and no socket and no child changes with it.
    server.report("hz-shell", Some("working"));
    wait_for(&ctx, &mut app, "the row of the session", |app| {
        snapshot(app).agents("build-1").len() == 1
    });
    run_app_frame(&ctx, &mut app);
    assert_eq!(ssh.forwards(), 1, "a new row of the same session starts no child");
    assert!(socket.exists(), "the socket of the session stays");

    // The row goes again while the session stays.
    server.report("hz-shell", None);
    wait_for(&ctx, &mut app, "the row that left", |app| {
        snapshot(app).agents("build-1").is_empty()
    });
    run_app_frame(&ctx, &mut app);
    assert_eq!(ssh.forwards(), 1, "a row that left ends no child");
    assert!(socket.exists(), "the socket of a session with no agent stays");
    drop(app);
}
