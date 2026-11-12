//! The program of this system that shows a notification, and the worker that runs
//! it.
//!
//! The lookup of the program reads the folders of the `PATH`, and the program
//! runs for a bounded time, so both run on a worker thread: a frame only sends one
//! value into a channel. A program that is absent, that fails or that does not end
//! within [`BOUND`] changes no state of Horizon, and one line at the level `INFO`
//! tells what happened, once for the run.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crate::ProgramEnvironment;
use crate::bounded_program::{self, ProgramFailure};

use super::{Notice, TITLE};

/// How long the program of one notification may run.
pub const BOUND: Duration = Duration::from_secs(5);

/// The program that a notification runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    /// `notify-send`, which a notification server of Linux reads.
    Linux,
    /// `osascript`, which shows the notification of macOS.
    MacOs,
}

impl System {
    /// The system of the computer that runs this process.
    #[must_use]
    pub const fn of_this_computer() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// The name of the program that shows a notification on `system`.
#[must_use]
pub const fn program_name(system: System) -> &'static str {
    match system {
        System::Linux => "notify-send",
        System::MacOs => "osascript",
    }
}

/// The sentence of the settings row when this computer has no such program.
#[must_use]
pub fn missing_sentence(system: System) -> String {
    format!("{} is not installed", program_name(system))
}

/// The arguments that show `notice` with the program of `system`.
///
/// Every line puts `--` before the text, so a name that starts with `-` is text
/// and never an option. The body of Linux is escaped, because the server of the
/// notification reads it as markup; the script of macOS is a constant, and the
/// text arrives as an argument of it.
#[must_use]
pub fn arguments(system: System, notice: &Notice) -> Vec<OsString> {
    match system {
        System::Linux => vec![
            OsString::from("--app-name=Horizon"),
            OsString::from("--"),
            OsString::from(TITLE),
            OsString::from(markup_escaped(&notice.body)),
        ],
        System::MacOs => [
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            "--",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([OsString::from(TITLE), OsString::from(&notice.body)])
        .collect(),
    }
}

/// `text` with the three characters that a notification server reads as markup.
#[must_use]
pub fn markup_escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The path of the program that shows a notification on this computer.
///
/// The folders of the `PATH` are read here and never on a frame, so the run of a
/// program is one value of this answer.
#[must_use]
pub fn find_program(system: System) -> Option<PathBuf> {
    find_program_in(system, &ProgramEnvironment::of_this_process())
}

/// [`find_program`] in the folders of `environment`, for a test whose folders hold
/// a stand-in program.
#[must_use]
pub fn find_program_in(system: System, environment: &ProgramEnvironment) -> Option<PathBuf> {
    crate::ui_thread::blocking("the program of a desktop notification is looked up");
    environment.find(program_name(system))
}

/// What shows the notifications of this run.
pub trait Sink: Send + Sync {
    /// Sends one notification, or drops it when the run cannot show it.
    fn send(&self, notice: Notice);

    /// Whether a program that shows a notification is on this computer; `None`
    /// while the run has not looked yet, and on a system that needs none.
    fn available(&self) -> Option<bool> {
        None
    }
}

/// The program of this computer, on the worker that a frame only writes to.
pub struct DesktopSink {
    notices: Sender<Notice>,
    available: std::sync::Arc<OnceLock<bool>>,
}

impl DesktopSink {
    /// Starts the worker that shows the notifications of this run.
    #[must_use]
    pub fn start() -> Self {
        let system = System::of_this_computer();
        Self::start_with_lookup(system, move || find_program(system))
    }

    /// Starts the worker with another lookup, so a test shows its own program and
    /// never the program of the computer of the person.
    #[must_use]
    pub fn start_with_lookup(system: System, lookup: impl FnOnce() -> Option<PathBuf> + Send + 'static) -> Self {
        let (notices, received) = channel();
        let available = std::sync::Arc::new(OnceLock::new());
        let thread_available = std::sync::Arc::clone(&available);
        let spawned = std::thread::Builder::new()
            .name("desktop-notification".into())
            .spawn(move || work(system, lookup, &received, &thread_available));
        if let Err(error) = spawned {
            tracing::info!(%error, "the notifications of this run run on no worker");
        }
        Self { notices, available }
    }
}

impl Sink for DesktopSink {
    fn send(&self, notice: Notice) {
        // A worker that ended drops the notification, and the window keeps its
        // state: the sidebar still shows it.
        let _ = self.notices.send(notice);
    }

    fn available(&self) -> Option<bool> {
        self.available.get().copied()
    }
}

/// Looks the program up, and runs it for every notification that arrives.
fn work(
    system: System,
    lookup: impl FnOnce() -> Option<PathBuf>,
    notices: &Receiver<Notice>,
    available: &OnceLock<bool>,
) {
    let program = lookup();
    let _ = available.set(program.is_some());
    let Some(program) = program else {
        tracing::info!(
            program = program_name(system),
            "no program shows a desktop notification on this computer"
        );
        // The channel still drains, so a frame never waits on a worker that shows
        // nothing.
        for _notice in notices {}
        return;
    };
    let mut told = false;
    for notice in notices {
        if let Err(failure) = show(&program, system, &notice, BOUND)
            && !std::mem::replace(&mut told, true)
        {
            // One line for the first program that did not show a notification.
            tracing::info!(
                program = %program.display(),
                failure = ?failure,
                "the program of a desktop notification did not show it"
            );
        }
    }
}

/// Runs the program of `notice` for `bound`, and answers how it ended.
///
/// A program that is still there at the bound is ended by its own process id and
/// reaped, so no program of the person is left behind.
fn show(program: &std::path::Path, system: System, notice: &Notice, bound: Duration) -> Result<(), ProgramFailure> {
    let args = arguments(system, notice);
    bounded_program::run(program, &args, &ProgramEnvironment::of_this_process(), bound).map(|_output| ())
}

#[cfg(test)]
mod tests;
