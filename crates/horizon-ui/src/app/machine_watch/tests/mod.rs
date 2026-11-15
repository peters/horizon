//! The watch of the machines of the window: the readers follow the machine list and
//! the setting, and a quit leaves no child behind.

mod support;

use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::{MachineWatch, MachineWatchSnapshot, WatchLink};

use crate::app::HorizonApp;
use crate::app::test_support::{run_app_frame, test_app};

use support::{Server, Ssh, record, settle_machine_load, wait_for, wait_until, with_machines};

/// The snapshot of the watch of `app`.
fn snapshot(app: &HorizonApp) -> Arc<MachineWatchSnapshot> {
    app.machine_watch.watch.snapshot()
}

/// The link of `machine` in the snapshot of `app`.
fn link(app: &mut HorizonApp, machine: &str) -> Option<WatchLink> {
    snapshot(app).machine(machine).map(|state| state.link)
}

#[test]
fn the_readers_follow_the_machine_list_and_start_no_child_twice() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(ssh) = Ssh::new(&server) else {
        return;
    };
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&ctx, &mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1"), record("build-2")]);
    wait_for(&ctx, &mut app, "the link of build-1", |app| {
        link(app, "build-1") == Some(WatchLink::Watching)
    });
    let running = ssh.calls();
    assert_eq!(running, 2, "one reader for each machine of the list");

    for _ in 0..3 {
        run_app_frame(&ctx, &mut app);
    }
    assert_eq!(ssh.calls(), running, "an unchanged list starts no reader again");

    // The route of one machine changed, so that reader starts again and the other
    // one stays.
    let moved = record("build-1");
    let moved = horizon_core::Machine {
        port: Some(2223),
        ..moved
    };
    with_machines(&mut app, vec![moved, record("build-2")]);
    wait_for(&ctx, &mut app, "the second reader of build-1", |_| {
        ssh.calls() > running
    });
    assert_eq!(ssh.calls(), running + 1, "one reader restarted and one stayed");
}

#[test]
fn a_quit_stops_every_reader_and_leaves_no_ssh_child() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(ssh) = Ssh::new(&server) else {
        return;
    };
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&ctx, &mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    wait_for(&ctx, &mut app, "the link of build-1", |app| {
        link(app, "build-1") == Some(WatchLink::Watching)
    });
    assert_eq!(ssh.children(), 1, "one ssh child of one watched machine");

    drop(app);
    wait_until("the child of the watch of the closed window", || ssh.children() == 0);
}

#[test]
fn the_setting_off_stops_every_reader() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(ssh) = Ssh::new(&server) else {
        return;
    };
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&ctx, &mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    wait_for(&ctx, &mut app, "the link of build-1", |app| {
        link(app, "build-1") == Some(WatchLink::Watching)
    });
    assert_eq!(ssh.children(), 1, "one ssh child of the watch");

    app.template_config.machines.watch_agents = false;
    wait_for(&ctx, &mut app, "the watch of the setting that is off", |app| {
        link(app, "build-1") == Some(WatchLink::Off)
    });
    wait_until("the child of the stopped watch", || ssh.children() == 0);
    assert!(
        snapshot(&app).agents("build-1").is_empty(),
        "a stopped watch holds no agent"
    );
}
