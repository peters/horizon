//! When a change becomes one notification: the group of two seconds, the least
//! time between two notifications, and the share of one window of ten minutes.
//!
//! Every rule here takes the time as a value, so a test moves a clock and never
//! sleeps. The state holds no clock of its own and starts no timer.

use std::time::{Duration, Instant};

use super::{Change, Notice, SILENCE, body};

/// How long changes collect into one notification.
pub const GROUP: Duration = Duration::from_secs(2);

/// The least time between two notifications.
pub const SPACING: Duration = Duration::from_secs(2);

/// The most notifications that one window holds.
pub const CAPACITY: usize = 20;

/// The length of that window.
pub const WINDOW: Duration = Duration::from_mins(10);

/// The changes that wait and the notifications that left.
#[derive(Debug, Default)]
pub struct Bounds {
    /// The changes that the next notification tells.
    pending: Vec<Change>,
    /// The moment that the first pending change arrived.
    held_since: Option<Instant>,
    /// The moment of the last notification.
    sent_at: Option<Instant>,
    /// The start of the window and how many notifications it holds.
    window: Option<(Instant, usize)>,
    /// Whether the share of the window is spent, so only the focus of a window of
    /// Horizon ends the silence.
    silenced: bool,
}

impl Bounds {
    /// Records one change that the person did not see.
    pub fn note(&mut self, change: Change, now: Instant) {
        if self.silenced {
            return;
        }
        self.held_since.get_or_insert(now);
        self.pending.push(change);
    }

    /// The notice that is due at `now`, or `None` while the changes wait.
    ///
    /// A change leaves at the end of its group, and never sooner than
    /// [`SPACING`] after the notification before it. The share of the window
    /// becomes the one line of [`SILENCE`] and then silence.
    pub fn due(&mut self, several_workspaces: bool, now: Instant) -> Option<Notice> {
        if self.silenced {
            return None;
        }
        if now < self.due_instant()? {
            return None;
        }
        let notice = if self.spends_share(now) {
            self.pending.clear();
            Notice::new(SILENCE)
        } else {
            Notice::new(body(&self.pending, several_workspaces))
        };
        self.pending.clear();
        self.held_since = None;
        self.sent_at = Some(now);
        Some(notice)
    }

    /// Whether the pending changes wait for the group or for the spacing.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.held_since.is_some()
    }

    /// When the next notification is due, for a frame that has to wake for it.
    #[must_use]
    pub fn due_at(&self) -> Option<Instant> {
        self.due_instant()
    }

    /// The moment that the pending changes leave: the end of their group, and
    /// never sooner than [`SPACING`] after the notification before them.
    fn due_instant(&self) -> Option<Instant> {
        let first = self.held_since?;
        let due = first + GROUP;
        Some(self.sent_at.map_or(due, |sent| due.max(sent + SPACING)))
    }

    /// Whether the share of the window is spent.
    #[must_use]
    pub fn silenced(&self) -> bool {
        self.silenced
    }

    /// The person is in front of a window of Horizon again: the changes that
    /// waited are seen in the sidebar, so they go, and the silence ends. The
    /// share of the window starts again with the person, because the share
    /// protects a person who is away from a flood of notifications.
    pub fn note_front(&mut self) {
        self.pending.clear();
        self.held_since = None;
        self.silenced = false;
        self.window = None;
    }

    /// Counts the notification that is due now in the window, and answers whether
    /// it spends the share of the window.
    fn spends_share(&mut self, now: Instant) -> bool {
        let (start, count) = match self.window {
            Some((start, count)) if now.duration_since(start) < WINDOW => (start, count),
            _ => (now, 0),
        };
        self.window = Some((start, count.saturating_add(1)));
        if count >= CAPACITY {
            self.silenced = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
