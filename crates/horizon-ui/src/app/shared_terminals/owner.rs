//! The owner side of the pump: who watches each terminal panel of this
//! computer, the screens and the output that the watchers get, and what they
//! type. The pump calls the screen and the takes of an outlet, so both come
//! from one thread, and a watch always starts with a screen: a take then never
//! continues a screen that another thread started.

use std::time::{Duration, Instant};

use horizon_core::ShareSource;
use horizon_sync::{LiveWatcher, MAX_LIVE_OUTPUT_BYTES};

use super::pump::Run;
use super::{LiveLink, shown};

/// The wait before the second screen of a new follower, which repairs a first
/// one that caught the program in the middle of an escape sequence, and before
/// the next screen after the link refused one.
const NEXT_SCREEN: Duration = Duration::from_millis(250);
/// The most bytes of a screen that the pump keeps while the link takes it in
/// parts, and how long the chip of a watched panel says who typed.
const KEPT_SCREEN_BYTES: usize = 8 * 1024 * 1024;
const TYPED_CHIP: Duration = Duration::from_secs(2);

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
    /// The screen that this panel still owes the link. No output follows a
    /// screen that is not whole.
    owed: Option<Owed>,
    /// The computer whose bytes arrived last, and when; and the chip of the
    /// followers, for the time after those bytes.
    typed: Option<(Instant, String)>,
    chip: String,
    /// The computer that typed last in the panel of this computer, while its
    /// program holds the grid that this computer gave it.
    sized: Option<String>,
}

/// The bytes of one screen that the link takes in parts, and how far they went.
struct Owed {
    cols: u16,
    rows: u16,
    why: Why,
    /// Whether the link has the grid of this screen and who takes it.
    grid: bool,
    bytes: Vec<u8>,
    sent: usize,
    /// The bytes of the next part. A part that the link refuses halves it.
    part: usize,
}

impl Watched {
    /// The chip of this panel: who holds the size of its program, if a follower
    /// does, and who watches the panel or typed in it last.
    fn chip_text(&self) -> String {
        match &self.sized {
            Some(name) => format!("sized by {name} \u{00b7} {}", self.chip),
            None => self.chip.clone(),
        }
    }

    /// Sends what the followers of this panel need now: a new screen when one
    /// is due, or else the output since the last take.
    fn drain(&mut self, link: &dyn LiveLink, id: &str, source: &ShareSource, now: Instant) {
        let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
        let outlet = source.outlet();
        // A restarted terminal has a new outlet, whose first take asks for a screen.
        outlet.set_wanted(true);
        if self.owed.is_some() {
            // The screen owes its bytes first, and the output of the panel waits.
            self.send_owed(link, id);
            return;
        }
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
        let kept = screen.is_some_and(|screen| {
            if screen.bytes.len() > KEPT_SCREEN_BYTES {
                // A screen above the bound would cycle for ever: the followers
                // keep the screen that they have.
                return true;
            }
            self.owed = Some(Owed {
                cols: screen.cols,
                rows: screen.rows,
                why,
                grid: false,
                bytes: screen.bytes,
                sent: 0,
                part: MAX_LIVE_OUTPUT_BYTES,
            });
            self.send_owed(link, id);
            true
        });
        self.screen_at = if kept {
            None
        } else {
            Some((now + NEXT_SCREEN, Why::All))
        };
        if kept {
            self.second_at = self.second_at.filter(|_| !why.everyone());
        }
    }

    /// Gives the link the next part of the screen that this panel owes: the grid
    /// before the first part, then the bytes in order. A part that the link
    /// refuses waits for the next pass with a smaller part.
    fn send_owed(&mut self, link: &dyn LiveLink, id: &str) {
        let Some(owed) = &mut self.owed else {
            return;
        };
        if !owed.grid {
            if !link.screen(id, owed.cols, owed.rows, owed.why.fresh(), owed.why.everyone()) {
                return;
            }
            owed.grid = true;
        }
        while owed.sent < owed.bytes.len() {
            let end = (owed.sent + owed.part).min(owed.bytes.len());
            if !link.output(id, &owed.bytes[owed.sent..end], true) {
                owed.part = (owed.part / 2).max(1);
                return;
            }
            owed.sent = end;
        }
        self.owed = None;
    }
}

/// The name of the computer that typed, for the chip: its label, or its device
/// name, cut and cleaned as each other text of another computer.
fn typed_name<'a>(from: &'a str, label: &'a str) -> &'a str {
    Some(label).filter(|label| !label.is_empty()).unwrap_or(from)
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
                let overdue = watch
                    .typed
                    .as_ref()
                    .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= TYPED_CHIP);
                if overdue {
                    watch.typed = None;
                    *changed = true;
                    chips.insert(id.clone(), watch.chip_text());
                }
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
            owed: None,
            typed: None,
            chip: String::new(),
            sized: None,
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
        watch.chip = watched_chip(watchers);
        self.found.chips.insert(id.to_owned(), watch.chip_text());
        self.changed = true;
    }

    /// Gives the program of the panel `id` the grid that a follower of it holds,
    /// after the four checks of a typed byte: both switches are on, the list that
    /// the link has names the panel, and a panel of this computer has that id and
    /// is no follow panel. The terminal then pushes no size of its own body, and
    /// the chip of the panel says who holds the grid.
    pub(super) fn resized(&mut self, id: &str, cols: u16, rows: u16, from: &str, label: &str) {
        let allowed = self.wanted.sharing && self.wanted.typing;
        let listed = self.sent.iter().any(|panel| panel.id == id);
        let own = self.wanted.panels.iter().find(|own| own.entry.id == id);
        let Some(own) = own.filter(|_| allowed && listed) else {
            return;
        };
        if !own.source.hold_grid(cols, rows) {
            return;
        }
        let name = shown(typed_name(from, label));
        if let Some(watch) = self.watched.get_mut(id) {
            watch.sized = Some(name);
            self.found.chips.insert(id.to_owned(), watch.chip_text());
            self.changed = true;
        }
    }

    /// Writes what another computer typed into the program of the panel `id`. The
    /// link gives the event as the server sent it, so this is the last guard: both
    /// switches are on, the list that the link has names the panel, and a panel of
    /// this computer has that id. A follow panel has no source and is none. The
    /// chip of a panel that somebody watches then says who types.
    pub(super) fn typed(&mut self, id: &str, bytes: &[u8], from: &str, label: &str, now: Instant) {
        let allowed = self.wanted.sharing && self.wanted.typing;
        let listed = self.sent.iter().any(|panel| panel.id == id);
        let own = self.wanted.panels.iter().find(|own| own.entry.id == id);
        let Some(own) = own.filter(|_| allowed && listed) else {
            return;
        };
        own.source.write_input(bytes);
        let Some(watch) = self.watched.get_mut(id) else {
            return;
        };
        // The person of this computer takes the size back to its own body.
        watch.sized = None;
        let name = shown(typed_name(from, label));
        self.found.chips.insert(id.to_owned(), format!("typed by {name}"));
        watch.typed = Some((now, name));
        self.changed = true;
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
