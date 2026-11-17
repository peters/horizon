//! The bounded hand-over from the thread that reads a program's output to a frame.
//!
//! The reader of a PTY runs on the event loop thread of a panel, and the frame
//! that shows the state of the panel runs on the main thread. A status is a level
//! and not a queue of events, so the newest report is the one that counts: the
//! hand-over holds a small fixed number of reports and drops the oldest when it is
//! full. The event loop thread never waits for the frame, and a frame that runs
//! late reads the newest report and not a backlog.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::scanner::Payload;

/// How many reports wait for a frame at most.
pub const QUEUE_DEPTH: usize = 8;

/// The payload of the feature detection query, `ESC ] 7501 ; ? ST`.
///
/// A program asks whether the terminal knows the protocol. The payoff of the
/// protocol is that the terminal drops no answer and no report, so a program may
/// send its state without asking; this one answers because the answer is a
/// constant sequence and the protocol asks a supporting terminal to reply.
pub const QUERY_PAYLOAD: &[u8] = b"?";

#[derive(Default)]
struct Shared {
    reports: Mutex<VecDeque<Payload>>,
    query: AtomicBool,
}

/// The place that a scanner of one terminal fills.
#[derive(Clone, Default)]
pub struct StatusTap {
    shared: Arc<Shared>,
}

/// The place where the frame of one panel reads the newest status of its program.
#[derive(Clone, Default)]
pub struct StatusInbox {
    shared: Arc<Shared>,
}

impl StatusTap {
    /// A tap with its inbox, sharing one place.
    #[must_use]
    pub fn new() -> (Self, StatusInbox) {
        let shared = Arc::new(Shared::default());
        (
            Self {
                shared: Arc::clone(&shared),
            },
            StatusInbox { shared },
        )
    }

    /// Records one complete payload of a status sequence.
    pub fn report(&self, payload: &Payload) {
        if payload.as_bytes() == QUERY_PAYLOAD {
            self.shared.query.store(true, Ordering::Release);
            return;
        }
        let mut reports = self.shared.reports.lock().unwrap_or_else(PoisonError::into_inner);
        if reports.len() >= QUEUE_DEPTH {
            reports.pop_front();
        }
        reports.push_back(*payload);
    }
}

impl StatusInbox {
    /// The newest report that arrived since the last call, and no older one.
    #[must_use]
    pub fn take_report(&self) -> Option<Payload> {
        let mut reports = self.shared.reports.lock().unwrap_or_else(PoisonError::into_inner);
        let newest = reports.pop_back();
        reports.clear();
        newest
    }

    /// Whether a program asked for the feature detection reply since the last call.
    #[must_use]
    pub fn take_query(&self) -> bool {
        self.shared.query.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests;
