//! The owner side of the pump: who watches each terminal panel of this
//! computer, the screens and the output that the watchers get, and what they
//! type. The pump calls the screen and the takes of an outlet, so both come
//! from one thread, and a watch always starts with a screen: a take then never
//! continues a screen that another thread started.

use std::time::{Duration, Instant};

use horizon_core::ShareSource;
use horizon_sync::LiveWatcher;

use super::pump::Run;
use super::{LiveLink, shown};

/// The wait before the second screen of a new follower, which repairs a first
/// one that caught the program in the middle of an escape sequence, and before
/// the next screen after the link refused one.
const NEXT_SCREEN: Duration = Duration::from_millis(250);

/// Why the next screen of a watched panel goes out, which decides what it holds
/// and who takes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    /// A new follower: a full reset and the lines above the viewport, for that
    /// follower alone.
    Join,
    /// A follower asked for a new screen: no history, for that follower alone.
    Asked,
    /// The second screen of a join, a new grid of the owner, too much output or a
    /// frame that the link refused: no history, and for every follower.
    All,
}

impl Why {
    /// Whether the follower forgets its screen, its history and its modes.
    fn fresh(self) -> bool {
        self == Self::Join
    }

    /// Whether every follower of the panel takes this screen.
    fn everyone(self) -> bool {
        self == Self::All
    }
}

/// One panel of this computer that another computer watches.
pub(super) struct Watched {
    /// The computers that watch it. An empty list ends the watch.
    devices: Vec<String>,
    /// When the next screen goes out, and why it is due.
    screen_at: Option<(Instant, Why)>,
    second_at: Option<Instant>,
}

impl Watched {
    /// Sends what the followers of this panel need now: a new screen when one
    /// is due, or else the output since the last take.
    fn drain(&mut self, link: &dyn LiveLink, id: &str, source: &ShareSource, now: Instant) {
        let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
        let outlet = source.outlet();
        // A restarted terminal has a new outlet, whose first take asks for a screen.
        outlet.set_wanted(true);
        let pending = self.screen_at.filter(|(at, _)| *at <= now).map(|(_, why)| why);
        let why = match pending {
            Some(why) => why,
            // The second screen of a join repairs a first one that started in the
            // middle of an escape sequence.
            None if due(self.second_at) => Why::All,
            None => {
                let take = outlet.take();
                if !take.overflowed {
                    if !take.bytes.is_empty() && !link.output(id, &take.bytes, false) {
                        // The link is full: only a new screen continues the stream.
                        self.screen_at = Some((now + NEXT_SCREEN, Why::All));
                    }
                    return;
                }
                Why::All
            }
        };
        // The screen holds the output that waits in the outlet, so that is dropped.
        let screen = source.screen(why.fresh());
        let sent = screen.is_some_and(|screen| {
            link.screen(id, screen.cols, screen.rows, why.fresh(), why.everyone())
                && link.output(id, &screen.bytes, true)
        });
        self.screen_at = if sent { None } else { Some((now + NEXT_SCREEN, Why::All)) };
        if sent {
            self.second_at = self.second_at.filter(|_| !why.everyone());
        }
    }
}

/// The chip of a watched panel: the computer that watches it, or how many do.
fn watched_chip(watchers: &[LiveWatcher]) -> String {
    match watchers {
        [one] if one.label.is_empty() => format!("seen by {}", shown(&one.device)),
        [one] => format!("seen by {}", shown(&one.label)),
        many => format!("seen by {} computers", many.len()),
    }
}

impl Run {
    /// Drains every watched panel, and ends the watch of a panel that closed,
    /// that left the list or that nobody watches any more.
    pub(super) fn drain_watched(&mut self, now: Instant) {
        let (link, panels, sent) = (self.link.as_ref(), &self.wanted.panels, &self.sent);
        let (chips, changed) = (&mut self.found.chips, &mut self.changed);
        self.watched.retain(|id, watch| {
            let own = panels.iter().find(|own| own.entry.id == *id);
            let listed = sent.iter().any(|panel| panel.id == *id);
            if let Some(own) = own.filter(|_| listed && !watch.devices.is_empty()) {
                watch.drain(link, id, &own.source, now);
                return true;
            }
            if let Some(own) = own {
                own.source.outlet().set_wanted(false);
            } else {
                // The panel closed, so it leaves the list of the link.
                link.ended(id);
            }
            *changed |= chips.remove(id).is_some();
            false
        });
    }

    /// Takes who watches the panel `id` now. A new follower gets a screen at
    /// once and a second one a moment later, and `refresh` asks for one screen.
    pub(super) fn watchers(&mut self, id: &str, watchers: &[LiveWatcher], refresh: bool, now: Instant) {
        // Only a panel of the list is watched, whatever the server names.
        let listed = self.sent.iter().any(|panel| panel.id == id);
        let known = !watchers.is_empty() || self.watched.contains_key(id);
        if !listed || !known {
            return;
        }
        let watch = self.watched.entry(id.to_owned()).or_insert(Watched {
            devices: Vec::new(),
            screen_at: None,
            second_at: None,
        });
        let devices: Vec<String> = watchers.iter().map(|watcher| shown(&watcher.device)).collect();
        let joined = devices.iter().any(|device| !watch.devices.contains(device));
        if joined {
            // A new follower reads what the program wrote before it came.
            watch.screen_at = Some((now, Why::Join));
            watch.second_at = Some(now + NEXT_SCREEN);
        } else if refresh {
            watch.screen_at = Some((now, Why::Asked));
        }
        watch.devices = devices;
        self.found.chips.insert(id.to_owned(), watched_chip(watchers));
        self.changed = true;
    }

    /// Writes what another computer typed into the program of the panel `id`. The
    /// link gives the event as the server sent it, so this is the last guard: both
    /// switches are on, the list that the link has names the panel, and a panel of
    /// this computer has that id. A follow panel has no source and is none.
    pub(super) fn typed(&self, id: &str, bytes: &[u8]) {
        let wanted = &self.wanted;
        let allowed = wanted.sharing && wanted.typing;
        let listed = self.sent.iter().any(|panel| panel.id == id);
        let own = wanted.panels.iter().find(|own| own.entry.id == id);
        if let Some(own) = own.filter(|_| allowed && listed) {
            own.source.write_input(bytes);
        }
    }

    /// Ends every watch, as the end of the link does.
    pub(super) fn unwatch_all(&mut self) {
        for (id, _) in self.watched.drain() {
            if let Some(own) = self.wanted.panels.iter().find(|own| own.entry.id == id) {
                own.source.outlet().set_wanted(false);
            }
            self.changed |= self.found.chips.remove(&id).is_some();
        }
    }
}
