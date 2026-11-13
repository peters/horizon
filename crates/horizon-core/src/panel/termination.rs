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

use std::time::Duration;

use rustix::process::Signal;

use crate::process::HangingChild;

/// The time that a program has after the hangup of its terminal, before
/// `SIGTERM` reaches its process group.
pub const AFTER_HANGUP: Duration = Duration::from_secs(2);

/// The time that `SIGTERM` has, before `SIGKILL` reaches the process group.
pub const AFTER_TERM: Duration = Duration::from_millis(1500);

/// Watches `child` on its own thread and ends it when it outlives the hangup.
///
/// `name` is the panel whose program this is, and it reaches the log with the
/// step that ended the program.
pub(crate) fn watch(child: HangingChild, name: &str) {
    let watched = name.to_owned();
    let spawned = std::thread::Builder::new()
        .name("panel-exit-watchdog".to_owned())
        .spawn(move || end_after_hangup(&child, &watched));
    if let Err(error) = spawned {
        tracing::warn!(panel = %name, %error, "cannot watch a program that outlives its terminal");
    }
}

fn end_after_hangup(child: &HangingChild, name: &str) {
    std::thread::sleep(AFTER_HANGUP);
    if !signal(child, Signal::TERM, "SIGTERM", name) {
        return;
    }
    std::thread::sleep(AFTER_TERM);
    let _ = signal(child, Signal::KILL, "SIGKILL", name);
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
