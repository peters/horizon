//! The holder that runs the program of an agent panel.
//!
//! An agent program that a person keeps through a restart runs in a holder of this
//! computer, and the terminal of its panel attaches to the socket of that holder
//! instead of starting the program in the process of Horizon. The decision runs on
//! the coordination worker: a start and a link both wait.

use crate::panel_holder::{self, Hello, HolderError, Launch, Linked};
use crate::{HorizonHome, PanelOptions};

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
    match start_holder(home, launch).and_then(|_| panel_holder::linked(home, &launch.local_id)) {
        Ok(Some(link)) => (Some(link), None),
        Ok(None) => (
            None,
            Some(notice(&HolderError::NoAnswer(launch.local_id.clone().into()))),
        ),
        Err(error) => (None, Some(notice(&error))),
    }
}

/// Starts the holder of `launch`, or serves it on a thread of a test.
fn start_holder(home: &HorizonHome, launch: &Launch) -> Result<Hello, HolderError> {
    #[cfg(test)]
    {
        return panel_holder::serve_on_a_thread(home, launch);
    }
    #[cfg(not(test))]
    {
        panel_holder::start(home, launch)
    }
}

/// The one line that a panel says when a holder cannot serve it.
fn notice(error: &HolderError) -> String {
    format!("This program runs in this window and ends with Horizon: {error}.\r\n")
}

#[cfg(test)]
mod tests;
