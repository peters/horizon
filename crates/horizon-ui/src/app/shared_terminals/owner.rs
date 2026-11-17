//! The owner side of shared terminals: the list of the terminal panels of this
//! computer, who watches each one, the screens and the output that the watchers
//! get, and what they type.
//!
//! The list is built again only after a trigger: terminal output on the frame
//! (a title and a state of a program arrive as output), another count of panels
//! or workspaces, a new answer of the attention reader, a change of the two
//! sharing switches, and one pass every two seconds, which catches a rename and
//! a resize that printed nothing. It is built at most four times a second and
//! goes to the link only when it changed. Only a watched panel costs more.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Attention, Panel, PanelId, PanelKind, Terminal};
use horizon_sync::{LivePanel, LiveWatcher, MAX_LIVE_PANELS};

use super::super::HorizonApp;
use super::{LiveLink, shown};

/// The shortest time between two builds of the list.
const LIST_EVERY: Duration = Duration::from_millis(250);
/// The longest time without a build of the list.
const LIST_CHECK: Duration = Duration::from_secs(2);
/// How often a watched panel is drained while no frame comes for another reason.
const DRAIN_EVERY: Duration = Duration::from_millis(50);
/// The wait before the second screen of a new follower, which repairs a first
/// one that caught the program in the middle of an escape sequence, and before
/// the next screen after the link refused one.
const NEXT_SCREEN: Duration = Duration::from_millis(250);

/// What a list was built from: the counts of panels and workspaces, the version
/// of the attention states and the two sharing switches.
type ListKey = (usize, usize, u64, bool, bool);

#[derive(Default)]
pub(super) struct Owner {
    /// The list that the link has.
    sent: Vec<LivePanel>,
    key: Option<ListKey>,
    pub(super) dirty: bool,
    built_at: Option<Instant>,
    /// The panels that another computer watches, by their id in the list.
    watched: HashMap<String, Watched>,
}

struct Watched {
    panel: PanelId,
    /// The computers that watch it. An empty list ends the watch.
    devices: Vec<String>,
    /// When the next screen goes out, and the second one of a new follower.
    screen_at: Option<Instant>,
    second_at: Option<Instant>,
}

impl Watched {
    /// Sends what the followers of this panel need now: a new screen when one
    /// is due, or else the output since the last take.
    fn drain(&mut self, link: &dyn LiveLink, id: &str, terminal: &Terminal, now: Instant) {
        let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
        let outlet = terminal.share_outlet();
        // A terminal that a restart started has a new outlet, which nobody
        // wanted yet. Its first take then asks for a screen.
        outlet.set_wanted(true);
        let take = outlet.take();
        if take.overflowed {
            self.screen_at.get_or_insert(now);
        }
        let second = due(self.second_at);
        if due(self.screen_at) || second {
            // The screen holds the output that the take gave, so that is dropped.
            let screen = terminal.share_screen();
            if link.screen(id, screen.cols, screen.rows) && link.output(id, &screen.bytes) {
                self.screen_at = None;
                self.second_at = self.second_at.filter(|_| !second);
            } else {
                self.screen_at = Some(now + NEXT_SCREEN);
            }
        } else if self.screen_at.is_none() && !take.bytes.is_empty() && !link.output(id, &take.bytes) {
            // The link is full, so the followers miss these bytes: only a new
            // screen continues their stream, and no output goes out before it.
            self.screen_at = Some(now + NEXT_SCREEN);
        }
    }
}

/// The kind of a panel as the list names it.
fn kind_code(kind: PanelKind) -> String {
    let code = serde_json::to_value(kind).unwrap_or_default();
    code.as_str().unwrap_or_default().to_owned()
}

/// The states of a panel with the names that the list gives them. An idle
/// panel names none.
pub(super) const STATES: [(Attention, &str); 4] = [
    (Attention::Working, "working"),
    (Attention::Background, "background"),
    (Attention::Done, "done"),
    (Attention::NeedsYou, "blocked"),
];

/// The chip of a watched panel: the computer that watches it, or how many do.
fn watched_chip(watchers: &[LiveWatcher]) -> String {
    match watchers {
        [one] if one.label.is_empty() => format!("seen by {}", shown(&one.device)),
        [one] => format!("seen by {}", shown(&one.label)),
        many => format!("seen by {} computers", many.len()),
    }
}

impl HorizonApp {
    /// The terminal panels of this computer that the other computers see: every
    /// panel with a terminal of its own, and no panel that follows another one.
    fn shared_list(&self) -> Vec<LivePanel> {
        let server = &self.template_config.server;
        if !server.share_terminals {
            return Vec::new();
        }
        let listed = self.board.panels.iter().filter_map(|panel| {
            let terminal = panel.terminal().filter(|terminal| terminal.follow_spec().is_none())?;
            let workspace = self.board.workspace(panel.workspace_id);
            let attention = self.panel_attention_of(panel);
            let state = STATES.iter().find(|(state, _)| *state == attention);
            Some(LivePanel {
                id: panel.local_id.clone(),
                title: panel.display_title().into_owned(),
                kind: kind_code(panel.kind),
                state: state.map(|(_, name)| (*name).to_owned()).unwrap_or_default(),
                workspace: workspace.map(|workspace| workspace.name.clone()).unwrap_or_default(),
                cols: terminal.cols(),
                rows: terminal.rows(),
                typing: server.remote_typing,
            })
        });
        listed.take(MAX_LIVE_PANELS).collect()
    }

    /// Gives the link the list of this computer when a trigger asked for a new
    /// build and the list changed, and then the output of every watched panel.
    pub(super) fn share_panels(&mut self, link: &dyn LiveLink, ctx: &Context, now: Instant) {
        let server = &self.template_config.server;
        let key: ListKey = (
            self.board.panels.len(),
            self.board.workspaces.len(),
            self.attention.version(),
            server.share_terminals,
            server.remote_typing,
        );
        let owner = &self.shared_terminals.owner;
        let waited = owner
            .built_at
            .map_or(Duration::MAX, |at| now.saturating_duration_since(at));
        let asked = owner.dirty || owner.key != Some(key) || waited >= LIST_CHECK;
        if asked && waited < LIST_EVERY {
            ctx.request_repaint_after(LIST_EVERY.saturating_sub(waited));
        } else if asked {
            let list = self.shared_list();
            let owner = &mut self.shared_terminals.owner;
            (owner.dirty, owner.key, owner.built_at) = (false, Some(key), Some(now));
            if list != owner.sent {
                owner.sent.clone_from(&list);
                link.set_panels(list);
            }
        }
        self.drain_watched(link, ctx, now);
    }

    /// Drains every watched panel, and ends the watch of a panel that closed,
    /// that left the list or that nobody watches any more.
    fn drain_watched(&mut self, link: &dyn LiveLink, ctx: &Context, now: Instant) {
        let (board, shared) = (&self.board, &mut self.shared_terminals);
        let (chips, sent) = (&mut shared.chips, &mut shared.owner.sent);
        shared.owner.watched.retain(|id, watch| {
            // A board that replaced the old one can give its number to another panel.
            let panel = board.panel(watch.panel).filter(|panel| panel.local_id == *id);
            let terminal = panel.and_then(Panel::terminal);
            let listed = sent.iter().any(|panel| panel.id == *id);
            if let Some(terminal) = terminal.filter(|_| listed && !watch.devices.is_empty()) {
                watch.drain(link, id, terminal, now);
                return true;
            }
            if let Some(terminal) = terminal {
                terminal.share_outlet().set_wanted(false);
            } else {
                // The panel closed, so it leaves the list of the link at once.
                link.ended(id);
                sent.retain(|panel| panel.id != *id);
            }
            chips.remove(&watch.panel);
            false
        });
        if !shared.owner.watched.is_empty() {
            ctx.request_repaint_after(DRAIN_EVERY);
        }
    }

    /// Takes who watches the panel `id` now. A new follower gets a screen at
    /// once and a second one a moment later, and `refresh` asks for one screen.
    pub(super) fn shared_watchers(&mut self, id: &str, watchers: &[LiveWatcher], refresh: bool, now: Instant) {
        let shared = &mut self.shared_terminals;
        // Only a panel of the list is watched, whatever the server names.
        let listed = shared.owner.sent.iter().any(|panel| panel.id == id);
        let known = !watchers.is_empty() || shared.owner.watched.contains_key(id);
        let panel = self.board.panel_id_by_local_id(id);
        let Some(panel) = panel.filter(|_| listed && known) else {
            return;
        };
        let watch = shared.owner.watched.entry(id.to_owned()).or_insert(Watched {
            panel,
            devices: Vec::new(),
            screen_at: None,
            second_at: None,
        });
        let joined = watchers.iter().any(|watcher| !watch.devices.contains(&watcher.device));
        if refresh || joined {
            watch.screen_at = Some(now);
        }
        if joined {
            watch.second_at = Some(now + NEXT_SCREEN);
        }
        watch.devices = watchers.iter().map(|watcher| watcher.device.clone()).collect();
        shared.chips.insert(panel, watched_chip(watchers));
    }

    /// Writes what another computer typed into the program of the panel `id`,
    /// while this computer lets the others type and the panel is in its list.
    pub(super) fn shared_typed(&mut self, id: &str, bytes: &[u8]) {
        let server = &self.template_config.server;
        let listed = self.shared_terminals.owner.sent.iter().any(|panel| panel.id == id);
        if server.share_terminals
            && server.remote_typing
            && listed
            && let Some(panel) = self.board.panels.iter_mut().find(|panel| panel.local_id == id)
        {
            panel.write_input(bytes);
            // The frame that drains the echo follows it at once, as for a key.
            if let Some(terminal) = panel.terminal() {
                terminal.arm_output_wake();
            }
        }
    }

    /// Ends every watch, as the end of the link does.
    pub(super) fn unwatch_all(&mut self) {
        for (_, watch) in self.shared_terminals.owner.watched.drain() {
            if let Some(terminal) = self.board.panel(watch.panel).and_then(Panel::terminal) {
                terminal.share_outlet().set_wanted(false);
            }
            self.shared_terminals.chips.remove(&watch.panel);
        }
    }
}
