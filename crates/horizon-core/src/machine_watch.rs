//! The agents that run on the machines of the machine list, seen without a panel.
//!
//! Horizon keeps one watch per machine of its list: one `ssh` process of this computer
//! that runs one constant text of POSIX `sh` on the machine, and that prints a report
//! only when the report changed. The command of that child is the one word `sh`, which
//! no login shell of a machine has to read, and the text arrives on its input. The
//! report names the panes of the `tmux` sessions of that user, the program of a pane,
//! its folder, its title, the state that its program reports in the pane option
//! `@horizon_state`, the command line of its program when an interpreter runs there,
//! and the last rows of its screen. The watch turns it into one row for each pane whose
//! program is one of the agent kinds of [`crate::agents`], or whose program reports a
//! state, with the state that a local panel of that kind would show.
//!
//! Every byte that a machine returns is untrusted, and the page
//! `docs/review/trust-boundaries/machine-watch.md` states that boundary: the report
//! is one fixed format with the bounds below, a report that breaks one is dropped
//! whole, and no text of a machine screen is stored.
//!
//! The watch lives on one worker thread per machine, and a frame reads
//! [`MachineWatch::snapshot`], which changes only when a row changed. No machine in
//! the list, and the setting `machines.watch_agents` off, start no thread.

use std::time::Duration;

mod agents;
mod open;
mod reader;
mod remote;
mod report;
mod snapshot;
mod state;
mod supervisor;
#[cfg(test)]
mod tests;

pub use agents::{AgentObservation, AgentPrograms, observations};
pub use remote::{REMOTE_PROGRAM, REMOTE_TEXT, WatchTarget, connection};
pub use report::{MachineNote, PaneReport, Report, ReportError, parse_report};
pub use snapshot::{MachineWatchSnapshot, MachineWatchState, Shared, WatchLink, WatchRefusal, unix_millis};
pub use state::{AgentState, MachineAgent, MachineModel};
pub use supervisor::MachineWatch;

/// The version of the format of one report of a machine. A report of another
/// version is dropped whole.
pub const WATCH_FORMAT_VERSION: u32 = 1;

/// How often the watch asks for a report, in seconds: the `sleep` of [`REMOTE_TEXT`].
pub const WATCH_INTERVAL_SECS: u16 = 2;

/// How often a machine with no `tmux` server repeats that, in seconds.
pub const SLOW_INTERVAL_SECS: u16 = 30;

/// The bytes of one line of a report of a machine.
pub const MAX_LINE_BYTES: usize = 8192;

/// The bytes of one whole report of a machine.
pub const MAX_REPORT_BYTES: usize = 256 * 1024;

/// The lines of one whole report of a machine.
pub const MAX_REPORT_LINES: usize = 768;

/// The panes that one report may hold: the `tmux` panes of the user, and the `tmux`
/// sessions of that user, which each get one card socket.
pub const MAX_PANES: usize = 64;
pub const MAX_SESSIONS: usize = 64;

/// The rows of its screen that one pane of a report may hold.
pub const MAX_ROWS: usize = 6;

/// The command lines of the processes of one pane that a report may hold.
pub const MAX_COMMAND_LINES: usize = 3;

/// The characters of the name of one agent. A longer name is cut, not refused.
pub const MAX_NAME_CHARS: usize = 64;

/// The machines that this computer watches at one time. A later machine of the list
/// is not watched, and its state says so.
pub const MAX_WATCHED_MACHINES: usize = 8;

/// How long the stop of one watch waits for its `ssh` child and its reader before it
/// kills the child and gives up on the thread.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The bytes of the standard error of one `ssh` child that the watch keeps to
/// classify a refusal.
pub const MAX_REFUSAL_BYTES: usize = 1024;
