//! The changes of the session index that the session writer did not write yet.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{FIRST_RETRY, IndexChange, LAST_RETRY};

#[derive(Default)]
pub(super) struct IndexQueue {
    /// In order. The first one stays here while it is written and after it failed.
    changes: VecDeque<IndexChange>,
    writing: bool,
    /// When the first change, which failed, is tried again, and the wait before that try.
    retry: Option<(Instant, Duration)>,
}

impl IndexQueue {
    pub(super) fn push(&mut self, change: IndexChange) {
        self.changes.push_back(change);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(super) fn unwritten(&self) -> Vec<IndexChange> {
        self.changes.iter().cloned().collect()
    }

    /// Whether a change is written now or was not tried yet; a failed one does not hold an
    /// exit.
    pub(super) fn pending(&self) -> bool {
        self.writing || (!self.changes.is_empty() && self.retry.is_none())
    }

    /// How long until the failed first change is tried again.
    pub(super) fn next_retry(&self, now: Instant) -> Option<Duration> {
        self.retry.map(|(at, _)| at.saturating_duration_since(now))
    }

    /// The first change if it is due; the worker writes it and then calls [`Self::written`].
    pub(super) fn take_due(&mut self, now: Instant) -> Option<IndexChange> {
        if self.writing || self.retry.is_some_and(|(at, _)| at > now) {
            return None;
        }
        let change = self.changes.front().cloned()?;
        self.writing = true;
        Some(change)
    }

    /// A written change leaves the queue; a failed one is tried again after a wait that
    /// doubles with each failure.
    pub(super) fn written(&mut self, ok: bool) {
        self.writing = false;
        if ok {
            self.changes.pop_front();
            self.retry = None;
        } else {
            let wait = self.retry.map_or(FIRST_RETRY, |(_, wait)| (wait * 2).min(LAST_RETRY));
            self.retry = Some((Instant::now() + wait, wait));
        }
    }

    /// Drops the change that is written now, after its write panicked.
    pub(super) fn abandon_writing(&mut self) {
        if std::mem::take(&mut self.writing) {
            self.changes.pop_front();
            self.retry = None;
        }
    }
}
