//! The end of the child of one bridge.
//!
//! A bridge that outlives the close of its standard input gets the termination
//! signal and then the kill signal, and each one has its own bound in time. Every
//! signal is guarded by the id and the start time of the process that the bridge
//! recorded at the start of the child, so no signal reaches a later process that
//! the system gave the same id.
//!
//! A system that reads the start time of a process with a program fills that
//! value after the start of the child, so a signal waits for it within
//! [`IDENTITY_GRACE`] and writes a log line when it cannot name the process.

use std::process::Child;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use rustix::process::Signal;

use crate::process::HangingChild;

/// The time between two reads of the state of a process or of a thread.
const STOP_POLL: Duration = Duration::from_millis(5);

/// How long a stop waits for the program after the termination signal, before
/// the kill signal.
const TERM_GRACE: Duration = Duration::from_millis(1500);

/// How long a stop waits for the program after the kill signal.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// How long a signal waits for the start time of the child to fill. A system
/// that reads that time with a program fills the cell after the start, and a
/// signal that leaves before the cell is set reaches nothing.
const IDENTITY_GRACE: Duration = Duration::from_secs(1);

/// Waits for the child to end and answers its exit code.
pub(super) fn wait_child(child: &Arc<Mutex<Option<Child>>>) -> Option<i32> {
    let mut slot = child.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.as_mut()?.wait() {
        Ok(status) => status.code(),
        Err(_) => None,
    }
}

/// Ends the child of `child`: the termination signal, then the kill signal, each
/// step guarded by the start time of the process so that no signal reaches a
/// later process that the system gave the same id.
pub(super) fn end_child(child: &HangingChild) {
    if child.has_ended() || !signal(child, Signal::TERM, "SIGTERM") {
        return;
    }
    if ended_within(child, TERM_GRACE) || !signal(child, Signal::KILL, "SIGKILL") {
        return;
    }
    let _ = ended_within(child, KILL_GRACE);
}

/// Sends one signal to the exact process id of the child.
fn signal(child: &HangingChild, signal: Signal, step: &'static str) -> bool {
    if !named(child) {
        tracing::warn!(
            pid = child.pid,
            step,
            "the bridge cannot name the process of its child, so it sent no signal"
        );
        return false;
    }
    let Ok(raw) = i32::try_from(child.pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    let reached = rustix::process::kill_process(pid, signal).is_ok();
    if reached {
        tracing::warn!(pid = child.pid, step, "the program of the bridge outlived its stop");
    }
    reached
}

/// Waits up to [`IDENTITY_GRACE`] for the start time of the child to fill, and
/// answers whether the id still names that process. A system that reads the start
/// time with a program fills the cell after the start of the child, so the first
/// read of a signal can be too early; a signal that leaves before the cell is set
/// reaches no process.
fn named(child: &HangingChild) -> bool {
    let deadline = Instant::now() + IDENTITY_GRACE;
    loop {
        if child.still_names_its_process() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(STOP_POLL);
    }
}

/// Whether the program of `child` ended within `within`.
fn ended_within(child: &HangingChild, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if child.has_ended() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        sleep(STOP_POLL.min(deadline.saturating_duration_since(now)));
    }
}

/// The time that is left of `deadline`.
pub(super) fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Waits up to `limit` for `thread`, and answers whether it ended. A thread that
/// did not end is left to its own end.
pub(super) fn join_within(thread: &JoinHandle<()>, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if thread.is_finished() {
            return true;
        }
        sleep(STOP_POLL);
    }
    thread.is_finished()
}
