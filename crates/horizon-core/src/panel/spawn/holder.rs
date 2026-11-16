//! The holder that runs the program of an agent panel.
//!
//! An agent program that a person keeps through a restart runs in a holder of this
//! computer, and the terminal of its panel attaches to the socket of that holder
//! instead of starting the program in the process of Horizon. The decision runs on
//! the coordination worker: a start and a link both wait.

use std::thread;
use std::time::{Duration, Instant};

use crate::panel_holder::{self, Hello, HolderError, Launch, Linked};
use crate::{HorizonHome, PanelOptions};

/// The longest time that a new holder waits for the holder of the program that a
/// restart ended to go.
const CLEAR_BOUND: Duration = Duration::from_secs(5);
/// The pause between two probes of that wait.
const WAIT_PAUSE: Duration = Duration::from_millis(10);

/// Whether the panel of `opts` runs its program in a holder: an agent panel that
/// runs on this computer, and not a terminal of a machine or an ssh terminal.
pub(in crate::panel) fn may_hold(opts: &PanelOptions) -> bool {
    opts.kind.is_agent() && opts.ssh_connection.is_none() && opts.machine.is_none()
}

/// The link to the holder of a restored panel whose program still runs, and the
/// line that a holder which answers no link leaves for the person.
pub(in crate::panel) fn attached(
    home: &HorizonHome,
    opts: &PanelOptions,
    local_id: &str,
    keep: bool,
) -> (Option<Linked>, Option<String>) {
    if !(keep && opts.is_restore && may_hold(opts)) {
        return (None, None);
    }
    match panel_holder::linked(home, local_id) {
        Ok(link) => (link, None),
        Err(error) => (None, Some(notice(&error))),
    }
}

/// The link to the new holder that starts `launch`, and the line that a holder
/// which does not start leaves for the person.
pub(in crate::panel) fn started(
    home: &HorizonHome,
    opts: &PanelOptions,
    keep: bool,
    launch: &Launch,
) -> (Option<Linked>, Option<String>) {
    if !(keep && may_hold(opts)) {
        return (None, None);
    }
    started_here(home, launch)
}

/// The link to the new holder that starts `launch`, once the holder of the program
/// before is gone. The caller judged that this panel runs its program in a holder.
pub(in crate::panel) fn started_here(home: &HorizonHome, launch: &Launch) -> (Option<Linked>, Option<String>) {
    let deadline = Instant::now() + CLEAR_BOUND;
    while !matches!(panel_holder::probe(home, &launch.local_id), Ok(None)) && Instant::now() < deadline {
        thread::sleep(WAIT_PAUSE);
    }
    match start_holder(home, launch).and_then(|_| panel_holder::linked(home, &launch.local_id)) {
        Ok(Some(link)) => (Some(link), None),
        Ok(None) => (
            None,
            Some(notice(&HolderError::NoAnswer(launch.local_id.clone().into()))),
        ),
        Err(error) => (None, Some(notice(&error))),
    }
}

/// Starts the holder of `launch` on a thread of a test, which starts no process.
#[cfg(test)]
fn start_holder(home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    panel_holder::serve_on_a_thread(home, launch)
}

/// Starts the holder of `launch` as a process of this computer.
#[cfg(not(test))]
fn start_holder(home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    panel_holder::start(home, launch)
}

/// The one line that a panel says when a holder cannot serve it.
fn notice(error: &HolderError) -> String {
    format!("This program runs in this window and ends with Horizon: {error}.\r\n")
}

#[cfg(test)]
mod tests;
