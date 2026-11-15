//! The agents that run on the machines of the machine list, seen without a panel.
//!
//! A person rents a machine, starts agents there in `tmux` and closes the panels
//! or the laptop lid. Horizon keeps one watch per machine of its list: one `ssh`
//! process of this computer that runs one constant text of POSIX `sh` on the
//! machine, and that prints a report only when the report changed. Each report
//! names the panes of the `tmux` sessions of that user, the program of a pane, its
//! folder, its title, the command line of its program when an interpreter runs
//! there, and the last rows of its screen.
//!
//! The watch turns that report into the agents of the machine: one row for each
//! pane whose program is one of the agent kinds of [`crate::agents`], with the
//! name of its folder and the state that a local panel of that kind would show.
//! The state reads the title rule of the kind first ([`crate::agent_work`]) and
//! then the working line of the screen, exactly as a panel does, so one agent
//! looks the same here and there.
//!
//! Every byte that a machine returns is untrusted, and the page
//! `docs/review/trust-boundaries/machine-watch.md` states that boundary. The
//! report is one fixed format with the bounds below, a report that breaks one is
//! dropped whole, and no text of a machine screen is stored.
//!
//! The watch lives on one worker thread per machine, and a frame reads
//! [`MachineWatch::snapshot`]: one value that changes only when a row changed, so
//! a frame that sees the same version does nothing. No machine in the list, and
//! the setting `machines.watch_agents` off, start no thread and no process.

use std::time::Duration;

mod agents;
mod remote;
mod report;
#[cfg(test)]
mod tests;

pub use agents::{AgentObservation, AgentPrograms, observations};
pub use remote::{REMOTE_TEXT, WatchTarget};
pub use report::{MachineNote, PaneReport, Report, ReportError, parse_report};

/// The version of the format of one report of a machine. A report of another
/// version is dropped whole.
pub const WATCH_FORMAT_VERSION: u32 = 1;

/// How often the watch of a machine asks for a report, in seconds: the `sleep`
/// of the loop of [`REMOTE_TEXT`].
pub const WATCH_INTERVAL_SECS: u16 = 2;

/// How often a machine with no `tmux` or no `tmux` server repeats that, in
/// seconds.
pub const SLOW_INTERVAL_SECS: u16 = 30;

/// The bytes of one line of a report of a machine.
pub const MAX_LINE_BYTES: usize = 8192;

/// The bytes of one whole report of a machine.
pub const MAX_REPORT_BYTES: usize = 256 * 1024;

/// The lines of one whole report of a machine.
pub const MAX_REPORT_LINES: usize = 704;

/// The panes that one report of a machine may hold: the `tmux` panes of the
/// user of that machine.
pub const MAX_PANES: usize = 64;

/// The rows of its screen that one pane of a report may hold.
pub const MAX_ROWS: usize = 6;

/// The command lines of the processes of one pane that a report may hold.
pub const MAX_COMMAND_LINES: usize = 3;

/// The characters of the name of one agent of a machine. A longer name is cut,
/// not refused.
pub const MAX_NAME_CHARS: usize = 64;

/// The machines that this computer watches at one time. A later machine of the
/// list is not watched, and its state says so.
pub const MAX_WATCHED_MACHINES: usize = 8;

/// How long the stop of one watch waits for its `ssh` child and for its reader
/// before it kills the child and gives up on the thread.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);
