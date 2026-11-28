//! The end of a program that outlives the hangup of its terminal.
//!
//! Horizon ends a panel by closing the master of its terminal, and the system
//! sends the hangup to the process group of the program. A program that ignores
//! the hangup would then hold the join of its event loop, and the exit or the
//! close of the panel would wait for it without a bound.
//!
//! The watchdog here gives that program two bounded steps: `SIGTERM` to its
//! process group after [`AFTER_HANGUP`], and `SIGKILL` after [`AFTER_TERM`] when
//! it still runs. Each signal re-reads the start time of the process, so it never
//! reaches a later process that the system gave the same id.
//!
//! The end of the program is also what completes the shutdown of its panel. A
//! program that ended is never waited for: the reaping of a process that the
//! kernel still holds after its end (a zombie, or the exiting state of a Mac)
//! has no bound, so the shutdown counts the panel when the program ended and
//! lets its event loop go on its own.

use std::time::{Duration, Instant};

use rustix::process::Signal;

use crate::panel_shutdown::PanelCompletion;
use crate::process::HangingChild;

/// The time that a program has after the hangup of its terminal, before
/// `SIGTERM` reaches its process group.
pub const AFTER_HANGUP: Duration = Duration::from_secs(2);

/// The time that `SIGTERM` has, before `SIGKILL` reaches the process group.
pub const AFTER_TERM: Duration = Duration::from_millis(1500);

/// The last time that a program has after `SIGKILL`, before the shutdown stops
/// waiting for the end of the process table entry.
pub const AFTER_KILL: Duration = Duration::from_millis(500);

/// The time between two reads of the state of a program.
const READ_INTERVAL: Duration = Duration::from_millis(100);

/// Watches `child` on its own thread and ends it when it outlives the hangup.
///
/// `name` is the panel whose program this is, and it reaches the log with the
/// step that ended the program. `completion` counts that panel once its program
/// has ended, or once every step below ran without an end.
pub(crate) fn watch(child: HangingChild, name: &str, completion: Option<PanelCompletion>) {
    let watched = name.to_owned();
    let spawned = std::thread::Builder::new()
        .name("panel-exit-watchdog".to_owned())
        .spawn(move || end_after_hangup(&child, &watched, completion.as_ref()));
    if let Err(error) = spawned {
        tracing::warn!(panel = %name, %error, "cannot watch a program that outlives its terminal");
    }
}

fn end_after_hangup(child: &HangingChild, name: &str, completion: Option<&PanelCompletion>) {
    if !ended_within(child, AFTER_HANGUP) {
        end_now(child, name);
    }
    finish(completion);
}

/// Ends the program now, with the two steps of a close and no wait for the
/// hangup, and answers whether it ended within them.
///
/// A panel that a board closes gives its program [`AFTER_HANGUP`] first, because
/// the hangup ends most programs. A caller that has no panel of a board, such as
/// the fixture of a test, ends its program at once.
pub(crate) fn end_now(child: &HangingChild, name: &str) -> bool {
    if child.has_ended() {
        return true;
    }
    if !signal(child, Signal::TERM, "SIGTERM", name) {
        return child.has_ended();
    }
    if ended_within(child, AFTER_TERM) {
        return true;
    }
    let _ = signal(child, Signal::KILL, "SIGKILL", name);
    // A program that the kernel already tore down cannot answer a signal and
    // never leaves its entry of the process table on a Mac, so the last wait has
    // its own end too.
    ended_within(child, AFTER_KILL)
}

/// Counts the panel of this program, whichever step ended it.
fn finish(completion: Option<&PanelCompletion>) {
    if let Some(completion) = completion {
        completion.finish();
    }
}

/// Waits up to `within` for the program to end, and answers whether it did.
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
        std::thread::sleep(READ_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

/// Sends one signal to the process group of `child` when its id still names that
/// process, and answers whether the signal left.
fn signal(child: &HangingChild, signal: Signal, step: &'static str, name: &str) -> bool {
    if !child.still_names_its_process() {
        return false;
    }
    let Some(pid) = i32::try_from(child.pid).ok().and_then(rustix::process::Pid::from_raw) else {
        return false;
    };
    // The program of a terminal leads the process group of that terminal, so the
    // group reaches the program and every process that it started.
    match rustix::process::kill_process_group(pid, signal) {
        Ok(()) => {
            tracing::warn!(
                panel = %name,
                step,
                pid = child.pid,
                "a program outlived the hangup of its terminal"
            );
            true
        }
        Err(error) => {
            tracing::debug!(
                panel = %name,
                step,
                pid = child.pid,
                %error,
                "the process group of a program is gone"
            );
            false
        }
    }
}
