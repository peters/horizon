//! The join of shared terminals in one window, with a stand-in of the live link
//! in the place of a server: an owner terminal that runs a small program and the
//! panel that follows it.

mod name;
mod no_frame;
mod relay;
mod window;

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision};
use horizon_sync::{LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveWatcher};

use super::{LiveLink, pump};
use crate::app::HorizonApp;
use crate::app::test_support::{run_app_frame, test_app_with_startup};

/// The time that a test gives one step of the join.
const PATIENCE: Duration = Duration::from_secs(60);

/// The other computer of the stand-in, and this window as that computer names it.
const THERE: &str = "desk-there";
const HERE: &str = "desk-here";

/// A place that no frame draws, so the terminal widget never resizes the panel.
const OFFSCREEN: [f32; 2] = [100_000.0, 100_000.0];

/// The program of an owner terminal: it says that it is ready, and answers each
/// line that it reads. `big` prints more than the outlet of a terminal holds,
/// `alt` enters the alternate screen and `main` leaves it.
const PROGRAM: &str = r#"stty -echo; printf 'READY\n'; while read -r line; do case "$line" in
big) head -c 1300000 /dev/zero | tr '\0' x; printf '\nBIG-DONE\n';;
alt) printf '\033[?1049hALTERNATE';;
main) printf '\033[?1049l';;
*) printf 'GOT:%s\n' "$line";;
esac; done"#;

/// A stand-in of the live link that plays the server for one window. The list
/// of this window comes back as the list of the computer [`THERE`], a follow of
/// that computer is a watcher of this window, and the screens, the output and
/// the typed bytes travel between the two sides of the one window.
#[derive(Clone, Default)]
struct StandIn(Arc<(Mutex<Relay>, Condvar)>);

#[derive(Default)]
struct Relay {
    /// The pump of the window, which a new event wakes as the link of a server does.
    pump: Option<Arc<pump::Shared>>,
    /// Holds the pump in its next call of the link, as a call that does not
    /// return: every call, or only one that sends a screen or output. And
    /// whether the pump waits in such a call now.
    held: bool,
    held_sends: bool,
    waiting: bool,
    events: Vec<LiveEvent>,
    panels: Vec<LivePanel>,
    followed: Vec<String>,
    /// The panels that the follower of this window waits for a screen of, and
    /// the panels whose screen it is taking now: the relay of the server.
    awaiting: Vec<String>,
    replay: Vec<String>,
    /// How many screens the owner side sent, how many the follower of this
    /// window took, and how many it asked for.
    screens: usize,
    taken: usize,
    refreshes: usize,
    /// Refuses screens and output, as a link whose queue is full does, and
    /// counts what it refused.
    full: bool,
    /// The room of the link for one screen or one call of output, and zero for a
    /// link with room for every frame.
    room: usize,
    refused: usize,
}

impl Relay {
    fn list(&mut self) {
        let computers = vec![
            LiveComputer {
                device: HERE.to_owned(),
                label: "Here".to_owned(),
                panels: Vec::new(),
                own: true,
            },
            LiveComputer {
                device: THERE.to_owned(),
                label: "There".to_owned(),
                panels: self.panels.clone(),
                own: false,
            },
        ];
        self.events.push(LiveEvent::Computers { computers });
    }

    fn watchers(&mut self, panel: &str, refresh: bool) {
        let watchers = self
            .followed
            .iter()
            .filter(|followed| *followed == panel)
            .map(|_| LiveWatcher {
                device: HERE.to_owned(),
                label: "Here".to_owned(),
            })
            .collect();
        self.events.push(LiveEvent::Watchers {
            panel: panel.to_owned(),
            watchers,
            refresh,
        });
    }

    /// Ends the follows of the panels that left the list.
    fn end_unlisted(&mut self) {
        let listed: Vec<&str> = self.panels.iter().map(|panel| panel.id.as_str()).collect();
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.followed)
            .into_iter()
            .partition(|followed| listed.contains(&followed.as_str()));
        self.followed = kept;
        for panel in gone {
            self.events.push(LiveEvent::Ended {
                owner: THERE.to_owned(),
                panel,
                reason: LiveEndReason::Closed,
            });
        }
    }

    fn follows(&self, panel: &str) -> bool {
        self.followed.iter().any(|followed| followed == panel)
    }

    /// Whether the follower of this window takes a frame of `panel` now: a screen
    /// that is for everyone or for a follower that waits, the replay of a screen
    /// that it took, and the live output after that replay.
    fn takes(&self, panel: &str, screen: bool, replay: bool, everyone: bool) -> bool {
        if !self.follows(panel) {
            return false;
        }
        if screen {
            return everyone || self.awaiting.iter().any(|waiting| waiting == panel);
        }
        let of = |list: &Vec<String>| list.iter().any(|listed| listed == panel);
        if replay { of(&self.replay) } else { !of(&self.awaiting) }
    }
}

/// The relay of a stand-in under its lock. An event that was added under the
/// lock wakes the pump when the lock goes back, as the link of a server does.
struct Waking<'a> {
    relay: MutexGuard<'a, Relay>,
    /// The count of the events when the lock was taken.
    before: usize,
}

impl Deref for Waking<'_> {
    type Target = Relay;

    fn deref(&self) -> &Relay {
        &self.relay
    }
}

impl DerefMut for Waking<'_> {
    fn deref_mut(&mut self) -> &mut Relay {
        &mut self.relay
    }
}

impl Drop for Waking<'_> {
    fn drop(&mut self) {
        if self.relay.events.len() > self.before
            && let Some(pump) = &self.relay.pump
        {
            pump.notify();
        }
    }
}

impl StandIn {
    fn relay(&self) -> Waking<'_> {
        let relay = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        let before = relay.events.len();
        Waking { relay, before }
    }

    /// The relay for a call of the pump that a test can hold: it waits while
    /// the test holds the link. `sends` is a call with a screen or output.
    fn gated(&self, sends: bool) -> Waking<'_> {
        let mut relay = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        while relay.held || (sends && relay.held_sends) {
            relay.waiting = true;
            relay = self.0.1.wait(relay).unwrap_or_else(PoisonError::into_inner);
        }
        relay.waiting = false;
        let before = relay.events.len();
        Waking { relay, before }
    }

    /// Lets the calls of the pump return again.
    fn release(&self) {
        let mut relay = self.relay();
        (relay.held, relay.held_sends) = (false, false);
        drop(relay);
        self.0.1.notify_all();
    }
}

impl LiveLink for StandIn {
    fn set_panels(&self, panels: Vec<LivePanel>) {
        let mut relay = self.relay();
        relay.panels = panels;
        relay.end_unlisted();
        relay.list();
    }

    fn follow(&self, owner: &str, panel: &str) {
        let mut relay = self.relay();
        if owner == THERE && relay.panels.iter().any(|listed| listed.id == panel) {
            if !relay.follows(panel) {
                relay.followed.push(panel.to_owned());
            }
            relay.awaiting.push(panel.to_owned());
            relay.watchers(panel, true);
        } else {
            relay.events.push(LiveEvent::Ended {
                owner: owner.to_owned(),
                panel: panel.to_owned(),
                reason: LiveEndReason::NotFound,
            });
        }
    }

    fn unfollow(&self, _owner: &str, panel: &str) {
        let mut relay = self.relay();
        relay.followed.retain(|followed| followed != panel);
        relay.watchers(panel, false);
    }

    fn refresh(&self, _owner: &str, panel: &str) {
        let mut relay = self.relay();
        relay.refreshes += 1;
        if relay.follows(panel) {
            relay.awaiting.push(panel.to_owned());
            relay.watchers(panel, true);
        }
    }

    fn screen(&self, panel: &str, cols: u16, rows: u16, fresh: bool, everyone: bool) -> bool {
        let mut relay = self.gated(true);
        if relay.full || (relay.room > 0 && 64 > relay.room) {
            relay.refused += 1;
            return false;
        }
        relay.screens += 1;
        if relay.takes(panel, true, false, everyone) {
            relay.taken += 1;
            relay.awaiting.retain(|waiting| waiting != panel);
            relay.replay.push(panel.to_owned());
            relay.events.push(LiveEvent::Screen {
                owner: THERE.to_owned(),
                panel: panel.to_owned(),
                cols,
                rows,
                fresh,
                everyone,
            });
        }
        true
    }

    fn output(&self, panel: &str, bytes: &[u8], replay: bool) -> bool {
        let mut link = self.gated(true);
        if link.full || (link.room > 0 && bytes.len() > link.room) {
            link.refused += 1;
            return false;
        }
        if link.takes(panel, false, replay, false) {
            if !replay {
                link.replay.retain(|drawing| drawing != panel);
            }
            link.events.push(LiveEvent::Output {
                owner: THERE.to_owned(),
                panel: panel.to_owned(),
                data: LiveBytes(bytes.into()),
                replay,
            });
        }
        true
    }

    fn ended(&self, panel: &str) {
        let mut relay = self.relay();
        relay.panels.retain(|listed| listed.id != panel);
        relay.end_unlisted();
        relay.list();
    }

    fn typed(&self, _owner: &str, panel: &str, bytes: &[u8]) {
        let mut relay = self.relay();
        if relay.follows(panel) {
            relay.events.push(LiveEvent::Typed {
                panel: panel.to_owned(),
                from: HERE.to_owned(),
                label: "Here".to_owned(),
                data: LiveBytes(bytes.into()),
            });
        }
    }

    fn take_events(&self) -> Vec<LiveEvent> {
        std::mem::take(&mut self.gated(false).events)
    }
}

/// One window of a test with the stand-in as its live link.
struct Window {
    _temp: tempfile::TempDir,
    ctx: Context,
    app: HorizonApp,
    link: StandIn,
    /// The pump of the window, for a test that counts its passes.
    pump: Arc<pump::Shared>,
}

impl Window {
    fn new() -> Self {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.root_viewport_stabilizer = None;
        let workspace = app.board.create_workspace("Work");
        app.board.focus_workspace(workspace);
        let link = StandIn::default();
        let pump = Arc::new(pump::Shared::default());
        link.relay().pump = Some(Arc::clone(&pump));
        app.shared_terminals
            .start_pump(Arc::clone(&pump), Box::new(link.clone()), &ctx);
        Self {
            _temp: temp,
            ctx,
            app,
            link,
            pump,
        }
    }

    /// A terminal panel of this window that runs [`PROGRAM`] at `position`.
    fn owner(&mut self, position: [f32; 2]) -> PanelId {
        let workspace = self.app.board.active_workspace.expect("the workspace of the test");
        let options = PanelOptions {
            kind: PanelKind::Command,
            name: Some("build".to_owned()),
            command: Some("/bin/sh".to_owned()),
            args: vec!["-c".to_owned(), PROGRAM.to_owned()],
            position: Some(position),
            size: Some([600.0, 300.0]),
            ..PanelOptions::default()
        };
        let panel = self
            .app
            .board
            .create_panel(options, workspace)
            .expect("the owner terminal");
        self.until("the owner program is ready", |app| text(app, panel).contains("READY"));
        panel
    }

    /// Runs frames until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&HorizonApp) -> bool) {
        frames_until(what, &mut [(&self.ctx, &mut self.app)], |apps| done(apps[0]));
    }

    /// Runs `count` frames, so what one frame queued reaches the other side:
    /// the pump finishes what each frame and the link queued, and one more
    /// frame takes what the pump found.
    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            run_app_frame(&self.ctx, &mut self.app);
            self.settle();
        }
        run_app_frame(&self.ctx, &mut self.app);
    }

    /// Holds the pump in a call of the link, as a link call that does not
    /// return, until [`StandIn::release`].
    fn hold_pump(&self) {
        self.link.relay().held = true;
        let deadline = Instant::now() + PATIENCE;
        while !self.link.relay().waiting {
            self.pump.notify();
            assert!(Instant::now() < deadline, "the pump reaches no call of the link");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Waits until the pump finished what the window and the link queued: two
    /// passes, of which the second one started after this call, with no event
    /// left for a third one.
    fn settle(&self) {
        let deadline = Instant::now() + PATIENCE;
        let mut passes = 0;
        while passes < 2 || !self.link.relay().events.is_empty() {
            let before = self.passes();
            self.pump.notify();
            while self.passes() == before {
                assert!(Instant::now() < deadline, "the pump makes no pass");
                std::thread::sleep(Duration::from_millis(1));
            }
            passes += 1;
        }
    }

    /// The count of the passes of the pump.
    fn passes(&self) -> u64 {
        self.pump.passes.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The id of `panel` in the list of this window.
    fn id(&self, panel: PanelId) -> String {
        self.app.board.panel(panel).expect("the panel").local_id.clone()
    }

    /// Opens the panel that follows `owner`, and answers it.
    fn follow(&mut self, owner: PanelId) -> PanelId {
        let id = self.id(owner);
        self.until("the list names the owner panel", |app| {
            let computers = &app.shared_terminals.found.computers;
            computers
                .iter()
                .any(|computer| computer.panels.iter().any(|panel| panel.id == id))
        });
        self.app.open_shared_terminal(THERE, &id, &self.ctx);
        follower_of(&self.app).expect("the follow panel")
    }

    fn write(&mut self, panel: PanelId, bytes: &[u8]) {
        let written = self.app.board.panel_mut(panel).expect("the panel").write_input(bytes);
        assert!(written, "the panel takes the bytes");
    }

    /// Waits until `panel` shows `words`.
    fn shows(&mut self, panel: PanelId, words: &str) {
        self.until(&format!("the panel shows {words}"), |app| {
            text(app, panel).contains(words)
        });
    }
}

/// Runs one frame of every window, again and again, until `done` holds.
fn frames_until(what: &str, apps: &mut [(&Context, &mut HorizonApp)], done: impl Fn(&[&HorizonApp]) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        for (ctx, app) in apps.iter_mut() {
            run_app_frame(ctx, app);
        }
        let seen: Vec<&HorizonApp> = apps.iter().map(|(_, app)| &**app).collect();
        if done(&seen) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}: it did not happen");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The last lines of the terminal of `panel`.
fn text(app: &HorizonApp, panel: PanelId) -> String {
    let terminal = app.board.panel(panel).and_then(Panel::terminal);
    terminal
        .map(|terminal| terminal.last_lines_text(60))
        .unwrap_or_default()
}

/// The text of the chip in the title bar of `panel`.
fn chip_of(app: &HorizonApp, panel: PanelId) -> Option<String> {
    let local = &app.board.panel(panel)?.local_id;
    app.shared_terminals.chip(local).map(|chip| chip.name.to_owned())
}

/// The first follow panel of a window.
fn follower_of(app: &HorizonApp) -> Option<PanelId> {
    let follower = app.board.panels.iter().find(|panel| panel.is_follower());
    follower.map(|panel| panel.id)
}

fn grid(app: &HorizonApp, panel: PanelId) -> (u16, u16) {
    let terminal = app.board.panel(panel).and_then(Panel::terminal).expect("a terminal");
    (terminal.cols(), terminal.rows())
}

#[test]
fn a_follow_panel_shows_the_owner_terminal_and_types_in_it() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let follow = window.follow(owner);

    // The list names the owner panel and never the panel that follows it,
    // also after the build that the new count of panels asked for.
    window.shows(follow, "READY");
    let opened = Instant::now();
    window.until("the build of the list after the new panel", move |_| {
        opened.elapsed() >= Duration::from_millis(600)
    });
    let listed = window.link.relay().panels.clone();
    assert_eq!(listed.len(), 1, "a follow panel is not shared: {listed:?}");
    let panel = &listed[0];
    assert_eq!(
        (
            panel.id.as_str(),
            panel.title.as_str(),
            panel.kind.as_str(),
            panel.workspace.as_str()
        ),
        (id.as_str(), "build", "command", "Work")
    );
    assert_eq!(
        (panel.cols, panel.rows, panel.typing, panel.state.as_str()),
        (80, 24, true, "")
    );
    assert_eq!(
        grid(&window.app, follow),
        (80, 24),
        "the follower has the grid of the owner"
    );

    // Live output, and what the person types in the follow panel.
    window.write(owner, b"one\n");
    window.shows(follow, "GOT:one");
    window.write(follow, b"two\n");
    window.shows(owner, "GOT:two");
    window.shows(follow, "GOT:two");

    // A new follower gets one screen at once and one a moment later.
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);
    assert_eq!(
        window.link.relay().screens,
        2,
        "a join asks for two screens and no more"
    );

    // A resize of the owner gives the follower the new grid.
    let owner_panel = window.app.board.panel_mut(owner).expect("the owner");
    owner_panel.resize_immediately(30, 100, 8, 17);
    window.until("the grid of the resize", |app| grid(app, follow) == (100, 30));
    window.write(owner, b"three\n");
    window.shows(follow, "GOT:three");

    // More output than the outlet holds, while the pump takes none of it, gives
    // a new screen. A link call that does not return holds the pump.
    let screens = window.link.relay().screens;
    window.hold_pump();
    window.write(owner, b"big\n");
    let deadline = Instant::now() + PATIENCE;
    while !text(&window.app, owner).contains("BIG-DONE") {
        window.app.board.process_output();
        assert!(Instant::now() < deadline, "the owner program prints its long output");
        std::thread::sleep(Duration::from_millis(5));
    }
    window.link.release();
    window.shows(follow, "BIG-DONE");
    assert!(
        window.link.relay().screens > screens,
        "an overflow is answered with a screen"
    );

    // A link that refuses output is answered with a new screen when it takes one.
    let screens = window.link.relay().screens;
    window.link.relay().full = true;
    window.write(owner, b"four\n");
    window.shows(owner, "GOT:four");
    // The pump takes the output on its own time, so the link stays full until
    // it refused that output.
    let link = window.link.clone();
    window.until("the link refuses the output", move |_| link.relay().refused > 0);
    window.link.relay().full = false;
    window.shows(follow, "GOT:four");
    assert!(
        window.link.relay().screens > screens,
        "a refused output is answered with a screen"
    );

    // A second press on the row goes to the open panel.
    let panels = window.app.board.panels.len();
    window.app.open_shared_terminal(THERE, &id, &window.ctx);
    assert_eq!(window.app.board.panels.len(), panels, "one panel follows one terminal");
    assert_eq!(window.app.board.focused, Some(follow));
}

/// The lines of `panel` above its viewport, oldest first.
fn above_the_viewport(app: &HorizonApp, panel: PanelId) -> Vec<String> {
    let terminal = app.board.panel(panel).and_then(Panel::terminal).expect("a terminal");
    let (lines, _) = terminal.full_text_lines(2000);
    let rows = usize::from(terminal.rows());
    lines[..lines.len().saturating_sub(rows)].to_vec()
}

#[test]
fn a_follower_reads_the_lines_that_the_program_wrote_before_it_came() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // Three hundred lines are written before anybody follows the panel.
    for line in 1..=300 {
        window.write(owner, format!("line {line}\n").as_bytes());
    }
    window.shows(owner, "GOT:line 300");
    let follow = window.follow(owner);
    window.shows(follow, "GOT:line 300");

    // The person scrolls back in a panel that follows: the lines of the program
    // that ran before the join are there.
    let above = above_the_viewport(&window.app, follow);
    assert!(
        above.iter().any(|line| line == "GOT:line 1"),
        "the oldest line is above the viewport of the follower: {} lines",
        above.len()
    );

    // A second computer follows the same panel. The owner makes the screen of
    // that computer, and it reaches that computer alone: the follower of this
    // window takes none of it, so its history and its screen do not change.
    let sent = window.link.relay().screens;
    let taken = window.link.relay().taken;
    let lines = above_the_viewport(&window.app, follow);
    let other = LiveWatcher {
        device: "desk-other".to_owned(),
        label: "Other".to_owned(),
    };
    let here = LiveWatcher {
        device: HERE.to_owned(),
        label: "Here".to_owned(),
    };
    let id = window.id(owner);
    window.link.relay().events.push(LiveEvent::Watchers {
        panel: id,
        watchers: vec![here, other],
        refresh: true,
    });
    window.frames(3);
    assert_eq!(
        window.link.relay().screens,
        sent + 1,
        "the owner makes a screen for the computer that joined"
    );
    assert_eq!(
        window.link.relay().taken,
        taken,
        "a follower that relays takes no screen of another follower"
    );
    assert_eq!(
        above_the_viewport(&window.app, follow),
        lines,
        "a follower that relays keeps its history when another follower joins"
    );
}

#[test]
fn the_two_switches_empty_the_list_and_drop_what_is_typed() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let ended = |app: &HorizonApp| app.board.panel(follow).and_then(Panel::follow_ended_line);

    // Without typing the owner drops the bytes, and the chip of the follower says so.
    window.app.template_config.server.remote_typing = false;
    let link = window.link.clone();
    window.until("the list says that nobody types", move |_| {
        link.relay().panels.first().is_some_and(|panel| !panel.typing)
    });
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some("on There \u{00b7} view only")
    });
    window.write(follow, b"dropped\n");
    window.frames(4);
    window.app.template_config.server.remote_typing = true;
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some("on There")
    });
    window.write(follow, b"kept\n");
    window.shows(owner, "GOT:kept");
    assert!(!text(&window.app, owner).contains("GOT:dropped"), "typing was off");

    // Without sharing the list is empty and the stream of the follower ends.
    window.app.template_config.server.share_terminals = false;
    let link = window.link.clone();
    window.until("the empty list", move |_| link.relay().panels.is_empty());
    window.until("the end of the stream", |app| ended(app).is_some());
    window.frames(1);
    assert!(
        chip_of(&window.app, owner).is_none(),
        "nobody watches a panel that is not shared"
    );
    let sentence = ended(&window.app).expect("the line of the ended stream");
    assert!(sentence.contains("shares it no more"), "{sentence}");

    // The follow panel stays, and follows again when the panel is listed again.
    window.app.template_config.server.share_terminals = true;
    window.until("the stream is back", |app| ended(app).is_none());
    window.write(owner, b"back\n");
    window.shows(follow, "GOT:back");
    window.frames(1);
    assert_eq!(chip_of(&window.app, owner).as_deref(), Some("seen by Here"));

    // The person closes the follow panel: nobody watches the owner any more.
    window.app.close_panel(follow);
    let link = window.link.clone();
    window.until("the unfollow", move |_| link.relay().followed.is_empty());
    window.until("the chip of the owner goes", |app| chip_of(app, owner).is_none());
}

#[test]
fn a_follow_panel_takes_the_typing_of_the_newest_list_in_both_directions() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // The owner lets nobody type when the person opens the panel.
    window.app.template_config.server.remote_typing = false;
    let link = window.link.clone();
    window.until("the list says that nobody types", move |_| {
        link.relay().panels.first().is_some_and(|panel| !panel.typing)
    });
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let view_only = "on There \u{00b7} view only";
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some(view_only)
    });

    // The owner turns typing on: the open panel takes it, with its chip.
    window.app.template_config.server.remote_typing = true;
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some("on There")
    });
    window.write(follow, b"typed now\n");
    window.shows(owner, "GOT:typed now");

    // And it loses it again when the owner turns typing off.
    window.app.template_config.server.remote_typing = false;
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some(view_only)
    });
    window.write(follow, b"no longer\n");
    window.frames(4);
    assert!(
        !text(&window.app, owner).contains("GOT:no longer"),
        "the panel drops the bytes after the owner turned typing off"
    );
}

/// Queues a `Typed` event as a server gives it that checks nothing: the link
/// hands it on as it is, so the owner side is the last guard.
fn typed_event(link: &StandIn, panel: &str, bytes: &[u8]) {
    link.relay().events.push(LiveEvent::Typed {
        panel: panel.to_owned(),
        from: "desk-unknown".to_owned(),
        label: "Unknown".to_owned(),
        data: LiveBytes(bytes.into()),
    });
}

#[test]
fn typed_bytes_reach_a_program_only_through_the_four_checks_of_the_owner() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let listed = |link: &StandIn, count: usize| {
        let relay = link.relay();
        relay.panels.len() == count && relay.panels.iter().all(|panel| panel.typing)
    };

    // 1. This computer lets nobody type. The panel is shared and in the list.
    // A person clears a switch in a frame, so a frame tells the pump.
    window.app.template_config.server.remote_typing = false;
    window.frames(1);
    typed_event(&window.link, &id, b"typing-off\n");
    window.frames(1);
    window.app.template_config.server.remote_typing = true;
    window.frames(1);

    // 2. This computer shares no terminal. The pump takes the event with the
    // new switch before it takes the panel out of the list of the link, so only
    // the switch refuses the bytes.
    window.hold_pump();
    window.app.template_config.server.share_terminals = false;
    run_app_frame(&window.ctx, &mut window.app);
    typed_event(&window.link, &id, b"sharing-off\n");
    window.link.release();
    window.frames(1);
    assert!(window.link.relay().panels.is_empty(), "the list of the link is empty");

    // 3. The list of the link does not name the panel yet. The pump takes the
    // event with the switch on before it gives the link the list again, so only
    // the list refuses the bytes.
    window.hold_pump();
    window.app.template_config.server.share_terminals = true;
    run_app_frame(&window.ctx, &mut window.app);
    typed_event(&window.link, &id, b"not-listed\n");
    window.link.release();
    let link = window.link.clone();
    window.until("the list is back", move |_| listed(&link, 1));

    // A panel that this window did not tell the pump yet. Its program would
    // read the bytes when it starts.
    let workspace = window.app.board.active_workspace.expect("the workspace of the test");
    let options = PanelOptions {
        kind: PanelKind::Command,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), PROGRAM.to_owned()],
        position: Some([OFFSCREEN[0], OFFSCREEN[1] + 1_000.0]),
        size: Some([600.0, 300.0]),
        ..PanelOptions::default()
    };
    let late = window
        .app
        .board
        .create_panel(options, workspace)
        .expect("a second terminal");
    let late_id = window.id(late);
    typed_event(&window.link, &late_id, b"not-known\n");
    window.settle();

    // 4. A follow panel. Its bytes would travel on to the terminal that it follows.
    let follow_id = window.id(follow);
    typed_event(&window.link, &follow_id, b"follow-panel\n");
    typed_event(&window.link, "no-such-panel", b"no-panel\n");
    window.frames(4);

    // The same events reach both programs when every check passes.
    window.shows(late, "READY");
    let link = window.link.clone();
    window.until("the list names both panels", move |_| listed(&link, 2));
    typed_event(&window.link, &id, b"kept\n");
    typed_event(&window.link, &late_id, b"kept\n");
    window.shows(owner, "GOT:kept");
    window.shows(late, "GOT:kept");
    let seen = format!("{}{}", text(&window.app, owner), text(&window.app, late));
    let refused = [
        "typing-off",
        "sharing-off",
        "not-listed",
        "not-known",
        "follow-panel",
        "no-panel",
    ];
    let reached: Vec<&str> = refused.into_iter().filter(|words| seen.contains(words)).collect();
    assert!(reached.is_empty(), "{reached:?} reached a program: {seen}");
}

#[test]
fn the_texts_of_another_computer_are_cut_and_cleaned_where_they_enter_the_window() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let id = window.id(owner);
    let link = window.link.clone();
    window.until("the list names the owner panel", move |_| {
        link.relay().panels.len() == 1
    });

    // A server that cuts nothing: every text is long and holds control characters.
    let dirty = |text: &str| format!("{text}\u{1b}[31m\n{}", "x".repeat(500));
    let panel = |index: usize| LivePanel {
        id: dirty(&format!("p{index}")),
        title: dirty("title"),
        kind: dirty("shell"),
        state: dirty("working"),
        workspace: dirty("work"),
        cols: 80,
        rows: 24,
        typing: true,
    };
    let computer = |index: usize| LiveComputer {
        device: dirty(&format!("desk-{index}")),
        label: dirty("There"),
        panels: (0..200).map(panel).collect(),
        own: false,
    };
    let computers = (0..100).map(computer).collect();
    let watcher = LiveWatcher {
        device: dirty("desk-0"),
        label: dirty("There"),
    };
    let events = [
        LiveEvent::Computers { computers },
        LiveEvent::Watchers {
            panel: id,
            watchers: vec![watcher],
            refresh: false,
        },
        LiveEvent::State(horizon_sync::LiveState::Down(dirty("down"))),
    ];
    // The end of the link takes the list and the watch away again, so the
    // window is read after each event alone.
    let clean = |text: &str| text.chars().count() <= 80 && !text.chars().any(char::is_control);
    let (mut listed, mut chipped) = (false, false);
    for event in events {
        window.link.relay().events.push(event);
        window.frames(1);
        let found = &window.app.shared_terminals.found;
        let computers = &found.computers;
        assert!(computers.len() <= 64, "the list keeps a bounded count of computers");
        for computer in computers {
            assert!(clean(&computer.device) && clean(&computer.label), "{computer:?}");
            assert_eq!(computer.panels.len(), 64, "one computer shares 64 panels at most");
            for panel in &computer.panels {
                let texts = [&panel.id, &panel.title, &panel.kind, &panel.state, &panel.workspace];
                assert!(texts.iter().all(|text| clean(text)), "{panel:?}");
            }
        }
        // The chip holds its own words in front of the name that it cut.
        let chip = chip_of(&window.app, owner);
        let chip = chip.as_deref().and_then(|chip| chip.strip_prefix("seen by "));
        assert!(chip.is_none_or(clean), "the chip of a watched panel: {chip:?}");
        assert!(
            found.down.as_deref().is_none_or(clean),
            "the line of a link that is down"
        );
        listed |= computers.len() == 64;
        chipped |= chip.is_some();
    }
    assert!(listed && chipped, "one frame kept the list and one showed the watcher");
    let down = window.app.shared_terminals.found.down.as_deref();
    assert!(
        down.is_some_and(|line| line.starts_with("down")),
        "the last event ended the link"
    );
}

#[test]
fn a_follower_that_joined_on_the_alternate_screen_asks_for_a_screen_when_it_leaves() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    window.write(owner, b"alt\n");
    window.shows(owner, "ALTERNATE");
    let follow = window.follow(owner);
    window.shows(follow, "ALTERNATE");
    // Both screens of the join are out, so no later one repairs the follower.
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);
    window.frames(4);
    assert_eq!(
        window.link.relay().refreshes,
        0,
        "a follower on its first screen asks for none"
    );

    // The primary screen under the alternate one did not travel, so the
    // follower asks for it when the program leaves the alternate screen.
    window.write(owner, b"main\n");
    let link = window.link.clone();
    window.until("the request for a new screen", move |_| link.relay().refreshes >= 1);
    window.shows(follow, "READY");
    assert!(!text(&window.app, follow).contains("ALTERNATE"));
}

#[test]
fn the_share_of_a_panel_goes_on_after_a_restart_of_its_terminal() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.write(owner, b"before\n");
    window.shows(follow, "GOT:before");
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);

    // The new terminal has a new outlet, which nobody asked for output yet.
    let screens = window.link.relay().screens;
    window.app.queue_panel_restart(owner);
    let link = window.link.clone();
    window.until("the screen of the new terminal", move |app| {
        link.relay().screens > screens && !text(app, follow).contains("GOT:before")
    });
    window.shows(follow, "READY");
    window.write(follow, b"after\n");
    window.shows(owner, "GOT:after");
    window.shows(follow, "GOT:after");
    assert_eq!(
        chip_of(&window.app, owner).as_deref(),
        Some("seen by Here"),
        "the panel is still watched"
    );
}

#[test]
fn a_closed_owner_panel_leaves_the_list_and_ends_the_stream() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    window.app.close_panel(owner);
    let link = window.link.clone();
    window.until("the panel leaves the list", move |_| link.relay().panels.is_empty());
    window.until("the end of the stream", |app| {
        app.board.panel(follow).and_then(Panel::follow_ended_line).is_some()
    });
    assert!(
        window.app.board.panel(follow).is_some(),
        "the follow panel keeps its last screen"
    );
}

/// A screen that does not fit the room of the link goes in parts, in the order of
/// its bytes, and the output of the program follows the whole screen.
#[test]
fn a_screen_that_does_not_fit_the_room_of_the_link_goes_in_parts_before_its_output() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // The program writes enough rows that its screen is much larger than the room
    // of the link below.
    let lines: String = (1..=40).fold(String::new(), |text, line| format!("{text}line {line}\n"));
    assert!(
        window
            .app
            .board
            .panel_mut(owner)
            .expect("the owner")
            .write_input(lines.as_bytes()),
        "the panel takes the lines"
    );
    window.shows(owner, "GOT:line 40");

    // A new follower joins with a small room: the parts of its screen are refused
    // until the pump offers a part that fits.
    window.link.relay().room = 512;
    let follow = window.follow(owner);
    let link = window.link.clone();
    window.until("the screen of the join in parts", move |_| link.relay().refused > 0);

    // The program writes after the screen was taken, so its echo is the live
    // output that may not pass the parts of that screen.
    assert!(
        window
            .app
            .board
            .panel_mut(owner)
            .expect("the owner")
            .write_input(b"after\n"),
        "the panel takes the line"
    );
    window.shows(follow, "GOT:after");
    let shows = text(&window.app, follow);
    assert!(shows.contains("GOT:line 40"), "the screen of the join arrived in parts");

    // The second screen of the join goes in parts too, and the output after the
    // first one stays on the screen of the follower.
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);
    let shows = text(&window.app, follow);
    assert!(
        shows.contains("GOT:after"),
        "the output stays after the screen: {shows}"
    );
}
