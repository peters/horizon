//! The join of shared terminals in one window, with a stand-in of the live link
//! in the place of a server: an owner terminal that runs a small program and the
//! panel that follows it.

mod relay;
mod window;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision};
use horizon_sync::{LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveWatcher};

use super::LiveLink;
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
struct StandIn(Arc<Mutex<Relay>>);

#[derive(Default)]
struct Relay {
    events: Vec<LiveEvent>,
    panels: Vec<LivePanel>,
    followed: Vec<String>,
    /// How many screens the owner side sent, and how many the follower asked for.
    screens: usize,
    refreshes: usize,
    /// Refuses screens and output, as a link whose queue is full does.
    full: bool,
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
}

impl StandIn {
    fn relay(&self) -> MutexGuard<'_, Relay> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
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
            relay.watchers(panel, true);
        }
    }

    fn screen(&self, panel: &str, cols: u16, rows: u16) -> bool {
        let mut relay = self.relay();
        if relay.full {
            return false;
        }
        relay.screens += 1;
        if relay.follows(panel) {
            relay.events.push(LiveEvent::Screen {
                owner: THERE.to_owned(),
                panel: panel.to_owned(),
                cols,
                rows,
            });
        }
        true
    }

    fn output(&self, panel: &str, bytes: &[u8]) -> bool {
        let mut relay = self.relay();
        if relay.full {
            return false;
        }
        if relay.follows(panel) {
            relay.events.push(LiveEvent::Output {
                owner: THERE.to_owned(),
                panel: panel.to_owned(),
                data: LiveBytes(bytes.into()),
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
        std::mem::take(&mut self.relay().events)
    }
}

/// One window of a test with the stand-in as its live link.
struct Window {
    _temp: tempfile::TempDir,
    ctx: Context,
    app: HorizonApp,
    link: StandIn,
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
        app.shared_terminals.link = Some(Box::new(link.clone()));
        Self {
            _temp: temp,
            ctx,
            app,
            link,
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

    /// Runs `count` frames, so what one frame queued reaches the other side.
    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            run_app_frame(&self.ctx, &mut self.app);
        }
    }

    /// The id of `panel` in the list of this window.
    fn id(&self, panel: PanelId) -> String {
        self.app.board.panel(panel).expect("the panel").local_id.clone()
    }

    /// Opens the panel that follows `owner`, and answers it.
    fn follow(&mut self, owner: PanelId) -> PanelId {
        let id = self.id(owner);
        self.until("the list names the owner panel", |app| {
            let computers = &app.shared_terminals.follower.computers;
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

    // The list names the owner panel and never the panel that follows it.
    window.shows(follow, "READY");
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

    // More output than the outlet holds, while no frame takes it, gives a new screen.
    let screens = window.link.relay().screens;
    window.write(owner, b"big\n");
    let deadline = Instant::now() + PATIENCE;
    while !text(&window.app, owner).contains("BIG-DONE") {
        window.app.board.process_output();
        assert!(Instant::now() < deadline, "the owner program prints its long output");
        std::thread::sleep(Duration::from_millis(5));
    }
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
        app.shared_terminals
            .chip(follow)
            .is_some_and(|chip| chip.name == "on There \u{00b7} view only")
    });
    window.write(follow, b"dropped\n");
    window.frames(4);
    window.app.template_config.server.remote_typing = true;
    window.until("the chip of the follow panel", |app| {
        app.shared_terminals
            .chip(follow)
            .is_some_and(|chip| chip.name == "on There")
    });
    window.write(follow, b"kept\n");
    window.shows(owner, "GOT:kept");
    assert!(!text(&window.app, owner).contains("GOT:dropped"), "typing was off");

    // Without sharing the list is empty and the stream of the follower ends.
    window.app.template_config.server.share_terminals = false;
    let link = window.link.clone();
    window.until("the empty list", move |_| link.relay().panels.is_empty());
    window.until("the end of the stream", |app| ended(app).is_some());
    assert!(
        window.app.shared_terminals.chip(owner).is_none(),
        "nobody watches a panel that is not shared"
    );
    let sentence = ended(&window.app).expect("the line of the ended stream");
    assert!(sentence.contains("shares it no more"), "{sentence}");

    // The follow panel stays, and follows again when the panel is listed again.
    window.app.template_config.server.share_terminals = true;
    window.until("the stream is back", |app| ended(app).is_none());
    window.write(owner, b"back\n");
    window.shows(follow, "GOT:back");
    assert_eq!(
        window.app.shared_terminals.chip(owner).map(|chip| chip.name),
        Some("seen by Here")
    );

    // The person closes the follow panel: nobody watches the owner any more.
    window.app.close_panel(follow);
    let link = window.link.clone();
    window.until("the unfollow", move |_| link.relay().followed.is_empty());
    window.until("the chip of the owner goes", |app| {
        app.shared_terminals.chip(owner).is_none()
    });
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
