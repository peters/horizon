//! The share with no frame of the window. A window that the desktop does not
//! show can get no frame, so after its setup a test here never runs one: the
//! pump moves every byte. The other tests are about the pump itself: when it
//! sleeps, how it stops, and that the window and the pump never hold each other.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, mpsc};
use std::time::{Duration, Instant};

use horizon_core::{PanelKind, PanelOptions};
use horizon_sync::{LiveEvent, LiveWatcher, ServerInfo, Store, run_server_on_unix_socket};

use super::super::pump;
use super::relay::{computer, owner, stand_in};
use super::{HERE, OFFSCREEN, PATIENCE, Window, follower_of, text};
use crate::app::HorizonApp;
use crate::app::test_support::run_app_frame;
use crate::app::undrawn::tests::passes_until;

/// The longest time that one stop of a pump takes in a test. A pass does no
/// I/O, so the real time is the rest of one pass.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// Waits until `done` holds and runs no frame. Answers whether it held in time.
fn waits(done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

/// What the two panels of a window show, read from their grids with no frame.
fn shows(app: &HorizonApp, panels: &[horizon_core::PanelId], words: &str) -> bool {
    panels.iter().all(|panel| text(app, *panel).contains(words))
}

#[test]
fn a_window_that_gets_no_frame_shares_and_follows_through_the_stand_in() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);

    // The setup is over: from here no frame of the window runs.
    let app = &mut window.app;
    let wrote = app.board.panel_mut(owner).expect("the owner").write_input(b"one\n");
    assert!(
        wrote && waits(|| shows(app, &[follow], "GOT:one")),
        "the output of the owner"
    );

    // What the person types in the follow panel reaches the program, and the
    // echo of the program comes back.
    let wrote = app.board.panel_mut(follow).expect("the follower").write_input(b"two\n");
    assert!(
        wrote && waits(|| shows(app, &[owner, follow], "GOT:two")),
        "the typed bytes and their echo"
    );

    // A new follower gets a screen at once and a second one a moment later.
    let screens = window.link.relay().screens;
    let watcher = |device: &str| LiveWatcher {
        device: device.to_owned(),
        label: device.to_owned(),
    };
    window.link.relay().events.push(LiveEvent::Watchers {
        panel: id,
        watchers: vec![watcher(HERE), watcher("desk-third")],
        refresh: true,
    });
    let link = window.link.clone();
    assert!(
        waits(|| link.relay().screens >= screens + 2),
        "the screens of a new follower"
    );
    let wrote = app.board.panel_mut(owner).expect("the owner").write_input(b"three\n");
    assert!(
        wrote && waits(|| shows(app, &[follow], "GOT:three")),
        "the output after the new screens"
    );
}

#[test]
fn two_windows_that_get_no_frame_share_and_follow_through_the_real_relay() {
    let home = tempfile::tempdir().expect("the home of the server");
    let socket = home.path().join("server.sock");
    let store = Store::open(&home.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&socket, store.clone(), &ServerInfo::default()).expect("the socket");
    let program = stand_in(home.path(), &socket);
    let (_temp_a, ctx_a, mut a) = computer(&program, "desk-a", "Desk A");
    let (_temp_b, ctx_b, mut b) = computer(&program, "desk-b", "Desk B");
    let panel = owner(&mut a);
    let id = a.board.panel(panel).expect("the owner panel").local_id.clone();
    super::frames_until(
        "B lists the panel of A",
        &mut [(&ctx_a, &mut a), (&ctx_b, &mut b)],
        |apps| {
            let computers = &apps[1].shared_terminals.found.computers;
            let listed = |computer: &horizon_sync::LiveComputer| computer.panels.iter().any(|listed| listed.id == id);
            text(apps[0], panel).contains("READY") && computers.iter().any(listed)
        },
    );

    // The setup is over: from here no frame of either window runs. The press on
    // the row is no frame, and the screen for the new follower arrives.
    b.open_shared_terminal("desk-a", &id, &ctx_b);
    let follow = follower_of(&b).expect("the follow panel of B");
    assert!(waits(|| shows(&b, &[follow], "READY")), "the screen of A at B");

    // A program of A prints, and B shows it.
    let wrote = a.board.panel_mut(panel).expect("the owner").write_input(b"one\n");
    assert!(
        wrote && waits(|| shows(&b, &[follow], "GOT:one")),
        "the output of A at B"
    );

    // B types, the program of A reads it, and its echo comes back to B.
    let wrote = b.board.panel_mut(follow).expect("the follower").write_input(b"two\n");
    assert!(wrote && waits(|| shows(&a, &[panel], "GOT:two")), "the bytes of B at A");
    assert!(waits(|| shows(&b, &[follow], "GOT:two")), "the echo of A at B");

    drop((a, b));
    server.stop();
    store.close();
}

#[test]
fn a_panel_that_opens_and_closes_in_a_window_that_draws_nothing_enters_and_leaves_the_list() {
    let mut window = Window::new();
    let first = window.owner(OFFSCREEN);
    let first_id = window.id(first);
    let listed = |link: &super::StandIn, id: &str| link.relay().panels.iter().any(|panel| panel.id == id);
    let (link, id) = (window.link.clone(), first_id.clone());
    window.until("the list names the first panel", move |_| listed(&link, &id));

    // From here no frame runs: the request of an agent opens a panel in a
    // window that the desktop does not show.
    let workspace = window.app.board.active_workspace.expect("the workspace of the test");
    let options = PanelOptions {
        kind: PanelKind::Command,
        name: Some("second".to_owned()),
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), super::PROGRAM.to_owned()],
        position: Some(OFFSCREEN),
        size: Some([600.0, 300.0]),
        ..PanelOptions::default()
    };
    // The program of such a panel waits for the preparation of a worker.
    let board = &mut window.app.board;
    let second = board
        .create_panel_waiting(options, workspace)
        .expect("the second terminal");
    let second_id = window.id(second);
    let (link, id) = (window.link.clone(), second_id.clone());
    assert!(
        passes_until(&window.ctx, &mut window.app, move |app| {
            listed(&link, &id) && text(app, second).contains("READY")
        }),
        "the other computers get the panel that opened, and its program runs"
    );
    assert!(listed(&window.link, &first_id), "the first panel stays in the list");

    // The panel closes, as a request of an agent closes it.
    window.app.board.close_panel(second);
    let (link, id) = (window.link.clone(), second_id.clone());
    assert!(
        passes_until(&window.ctx, &mut window.app, move |_| !listed(&link, &id)),
        "the panel that closed leaves the list of the other computers"
    );
    assert!(listed(&window.link, &first_id), "the first panel stays in the list");
}

#[test]
fn the_pump_sleeps_while_nothing_is_watched_and_nothing_is_followed() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let link = window.link.clone();
    window.until("the list names the owner panel", move |_| {
        link.relay().panels.iter().any(|panel| panel.id == id)
    });
    window.frames(2);

    // No panel is watched and no follow panel is open, so no timer runs: the
    // pump makes no pass in a second without a frame, and none for a frame
    // that changes nothing.
    let passes = window.passes();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(window.passes(), passes, "a pump with no stream sleeps");
    for _ in 0..5 {
        run_app_frame(&window.ctx, &mut window.app);
    }
    assert_eq!(window.passes(), passes, "a frame that changes nothing wakes no pump");

    // A notice wakes it for one pass, and it sleeps again.
    window.pump.notify();
    assert!(waits(|| window.passes() > passes), "a notice wakes the pump");
    let passes = window.passes();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(window.passes(), passes, "one notice is one pass");

    // The stop wakes a pump that sleeps with no timer, and its thread ends.
    let pump = Arc::downgrade(&window.pump);
    let stopped = Instant::now();
    window.app.stop_shared_terminals();
    window.link.relay().pump = None;
    window.pump = Arc::default();
    assert!(
        waits(|| pump.strong_count() == 0),
        "the thread of a sleeping pump ends at the stop"
    );
    assert!(stopped.elapsed() < STOP_BOUND, "the stop took {:?}", stopped.elapsed());
}

#[test]
fn a_pump_asks_for_a_frame_only_when_what_it_found_changed() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    // A new pump of the same link, whose requests for a frame the test counts.
    let asked = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&asked);
    let shared = Arc::new(pump::Shared::default());
    window.link.relay().pump = Some(Arc::clone(&shared));
    window.pump = Arc::clone(&shared);
    let join = &mut window.app.shared_terminals;
    shared.want(&join.wanted);
    let repaint = move || {
        count.fetch_add(1, Ordering::Relaxed);
    };
    let link = Box::new(window.link.clone());
    join.pump = Some(pump::Pump::start(shared, link, repaint, join.pump.take()).expect("the pump of the test"));
    window.write(owner, b"one\n");
    window.shows(follow, "GOT:one");
    assert!(
        waits(|| asked.load(Ordering::Relaxed) > 0),
        "the pump asked for the frame of its chips"
    );

    // The second screen of the join went out, and the stream is quiet: the pump
    // runs on its timer, finds nothing new and asks for no frame. A loaded
    // computer delays a pass past any short window, so the test waits for the
    // passes and does not time them.
    std::thread::sleep(Duration::from_millis(600));
    let (before, passes) = (asked.load(Ordering::Relaxed), window.passes());
    assert!(
        waits(|| window.passes() >= passes + 4),
        "the pump of a stream runs on its timer"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        before,
        "a pass that found nothing new asks for no frame"
    );

    // One more computer watches the panel, so its chip changes.
    let watcher = |device: &str| LiveWatcher {
        device: device.to_owned(),
        label: device.to_owned(),
    };
    window.link.relay().events.push(LiveEvent::Watchers {
        panel: id,
        watchers: vec![watcher(HERE), watcher("desk-third")],
        refresh: false,
    });
    assert!(
        waits(|| asked.load(Ordering::Relaxed) > before),
        "the pump asks for the frame of a new chip"
    );
}

#[test]
fn every_stop_ends_the_pump_within_its_bound_and_leaves_no_thread() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    // Ten more starts: each new pump ends the pump before it, as the start of a
    // new live link does, and the last stop is the end of the link.
    let mut pumps = vec![Arc::downgrade(&window.pump)];
    let mut fastest = Duration::MAX;
    for _ in 0..10 {
        let pump = Arc::new(pump::Shared::default());
        pumps.push(Arc::downgrade(&pump));
        window.link.relay().pump = Some(Arc::clone(&pump));
        window.pump = Arc::clone(&pump);
        let started = Instant::now();
        window
            .app
            .shared_terminals
            .start_pump(pump, Box::new(window.link.clone()), &window.ctx);
        let before = &pumps[pumps.len() - 2];
        assert!(
            waits(|| before.strong_count() == 0),
            "the pump before the new one ended"
        );
        fastest = fastest.min(started.elapsed());
    }
    // The fastest start is the least that the machine adds to the product's own
    // time: eleven rounds gave 2.71 s for the bound of 2 s at load 30.
    assert!(fastest < STOP_BOUND, "a new start ends the old pump in {fastest:?}");
    window.write(owner, b"last\n");
    window.shows(follow, "GOT:last");

    let stopped = Instant::now();
    window.app.stop_shared_terminals();
    window.link.relay().pump = None;
    window.pump = Arc::default();
    assert!(
        waits(|| pumps.iter().all(|pump| pump.strong_count() == 0)),
        "no thread of a pump is left after eleven starts and the stop"
    );
    assert!(stopped.elapsed() < STOP_BOUND, "the stop took {:?}", stopped.elapsed());
    // The pump ends the streams of its follow panels on its own thread, so the
    // test waits for the line of the panel and does not read it once.
    assert!(
        waits(|| window
            .app
            .board
            .panel(follow)
            .and_then(horizon_core::Panel::follow_ended_line)
            .is_some_and(|line| line.contains("no link"))),
        "the follow panel says that its stream ended"
    );
}

#[test]
fn a_window_that_stands_still_in_a_copy_does_not_hold_the_pump() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    // The two locks that the window and the pump share, held as a window holds
    // them that stopped in the middle of its copy. No frame runs.
    let pump = Arc::clone(&window.pump);
    let held = (
        pump.wanted.lock().unwrap_or_else(PoisonError::into_inner),
        pump.found.lock().unwrap_or_else(PoisonError::into_inner),
    );
    let app = &mut window.app;
    let wrote = app.board.panel_mut(owner).expect("the owner").write_input(b"one\n");
    let printed = wrote && waits(|| shows(app, &[follow], "GOT:one"));
    let wrote = app.board.panel_mut(follow).expect("the follower").write_input(b"two\n");
    let typed = wrote && waits(|| shows(app, &[owner, follow], "GOT:two"));
    drop(held);
    assert!(
        printed,
        "the output of the owner reaches the follower while the window holds its locks"
    );
    assert!(
        typed,
        "the typed bytes and their echo travel while the window holds its locks"
    );
}

#[test]
fn a_pump_that_waits_in_a_call_of_the_link_does_not_hold_a_frame() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    // The link takes no output any more, so the pump waits in the middle of a
    // pass with the output of the owner. A helper lets the link go after ten
    // seconds, so a frame that waited for the pump ends the test with a
    // failure and not with a hang.
    window.link.relay().held_sends = true;
    window.write(owner, b"one\n");
    let link = window.link.clone();
    assert!(waits(|| link.relay().waiting), "the pump waits in a call of the link");
    let (done, wait) = mpsc::channel::<()>();
    let release = std::thread::spawn(move || {
        let _ = wait.recv_timeout(Duration::from_secs(10));
        link.release();
    });
    window.app.template_config.server.remote_typing = false;
    for _ in 0..5 {
        run_app_frame(&window.ctx, &mut window.app);
    }
    let waiting = window.link.relay().waiting;
    let wanted = window.pump.wanted.lock().unwrap_or_else(PoisonError::into_inner);
    let told = !wanted.typing;
    drop(wanted);
    let passes = window.pump.passes.load(Ordering::Acquire);
    let _ = done.send(());
    release.join().expect("the helper that lets the link go");
    assert!(
        waiting,
        "five frames ran while the pump still waited in its call of the link"
    );
    assert!(told, "a frame told the pump the new switch while the pump waited");
    assert!(
        waits(|| window.passes() > passes),
        "the pump goes on when the link takes a call again"
    );
    window.shows(follow, "GOT:one");
}
