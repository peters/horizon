//! The frames that wait to be written to one live link, under one ceiling.
//!
//! The hub puts a frame here and returns at once, so the thread that reads the
//! link of an owner never waits for a follower. A screen and its output may
//! leave: output that would pass the ceiling drops the waiting output of its panel
//! and is not queued. Every other frame is kept: a list replaces the list that
//! still waits, and a kept frame that would pass the ceiling first drops all
//! waiting output. Kept frames that alone pass the ceiling close the outbox,
//! because the computer does not read its link.

use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
};

use super::protocol::LiveEvent;

/// The most bytes that wait for one live link.
pub(crate) const CEILING_BYTES: usize = 1024 * 1024;
/// The bytes under which a link that fell behind is ready again.
pub(crate) const RESUME_BYTES: usize = 256 * 1024;

/// What one waiting frame is, which decides whether it may leave.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Tag {
    Kept,
    /// The list of the computers: the newest one replaces it.
    List,
    /// The lists of the session mirror: the newest one replaces it. It is a tag
    /// of its own, because the list of the computers and the lists of the mirror
    /// are two documents, and one must not replace the other while it waits.
    MirrorList,
    /// A screen or output of one panel of one owner.
    Stream(String, String),
}

/// What became of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Push {
    Queued,
    /// The frame waits, and all waiting output of the link left to make room.
    Evicted,
    /// The frame does not wait: output fell behind, or the outbox closed.
    Full,
}

/// What the thread of a link does next.
pub(crate) enum Next {
    /// Write this frame, and report [`Outbox::written`].
    Event(LiveEvent),
    /// Output was dropped, and the link is under [`RESUME_BYTES`] again.
    Resume,
    Closed,
}

#[derive(Default)]
struct State {
    queue: VecDeque<(LiveEvent, usize, Tag)>,
    /// The cost of the queue and of the frame that is being written.
    bytes: usize,
    writing: usize,
    behind: bool,
    closed: bool,
}

impl State {
    fn drop_waiting(&mut self, leaves: impl Fn(&Tag) -> bool) {
        self.queue.retain(|(_, _, tag)| !leaves(tag));
        self.bytes = self.writing + self.queue.iter().map(|(_, cost, _)| cost).sum::<usize>();
    }
}

#[derive(Default)]
pub(crate) struct Outbox {
    state: Mutex<State>,
    ready: Condvar,
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues one frame under the rules of the ceiling. It never waits.
    pub(crate) fn push(&self, event: LiveEvent, tag: Tag) -> Push {
        let cost = event.cost();
        let mut state = self.lock();
        let mut answer = Push::Queued;
        if state.closed {
            return answer;
        }
        if matches!(tag, Tag::List | Tag::MirrorList) {
            state.drop_waiting(|waiting| *waiting == tag);
        }
        if state.bytes + cost > CEILING_BYTES {
            let stream = matches!(tag, Tag::Stream(..));
            state.drop_waiting(|waiting| {
                if stream {
                    *waiting == tag
                } else {
                    matches!(waiting, Tag::Stream(..))
                }
            });
            state.behind = true;
            answer = Push::Evicted;
            if stream || state.bytes + cost > CEILING_BYTES {
                state.closed = !stream;
                self.ready.notify_all();
                return Push::Full;
            }
        }
        state.queue.push_back((event, cost, tag));
        state.bytes += cost;
        self.ready.notify_all();
        answer
    }

    /// Drops the waiting screen and output of one panel of one owner.
    pub(crate) fn drop_stream(&self, owner: &str, panel: &str) {
        self.lock()
            .drop_waiting(|tag| matches!(tag, Tag::Stream(of, id) if of == owner && id == panel));
    }

    pub(crate) fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// Whether this outbox takes no more frames, because its link ended.
    pub(crate) fn closed(&self) -> bool {
        self.lock().closed
    }

    /// Waits for the next step of the thread of this link.
    pub(crate) fn next(&self) -> Next {
        let mut state = self.lock();
        loop {
            if state.closed {
                return Next::Closed;
            }
            if state.behind && state.bytes < RESUME_BYTES {
                state.behind = false;
                return Next::Resume;
            }
            if let Some((event, cost, _)) = state.queue.pop_front() {
                state.writing = cost;
                return Next::Event(event);
            }
            state = self.ready.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The frame of the last [`Next::Event`] reached the link.
    pub(crate) fn written(&self) {
        let mut state = self.lock();
        state.bytes -= std::mem::take(&mut state.writing);
    }

    /// The bytes that wait now, and whether [`Self::next`] would wait.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> (usize, bool) {
        let state = self.lock();
        let step = state.closed || !state.queue.is_empty() || (state.behind && state.bytes < RESUME_BYTES);
        (state.bytes, !step)
    }
}
