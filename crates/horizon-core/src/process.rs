//! The presence and the start time of one process of this computer.
//!
//! A process id alone is a weak name: the system gives the same id again after
//! the process ends. Signal 0 answers whether an id still names a process on
//! every Unix: `ESRCH` says that no such process exists, and `EPERM` says that
//! it exists and belongs to another person. The start time of a process tells
//! two processes that had one id apart.
//!
//! The read of a presence uses `rustix`, which the workspace already holds, and
//! no shell of the person. The read of a start time uses `/proc` on Linux and
//! the `ps` of the system on every other Unix; a caller runs the program through
//! the bounded program runner and never during a frame.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::bounded_program::{ProgramEnvironment, run};

/// Whether a process with `pid` exists on this computer.
///
/// `pid` 0 names the process group of the caller and never one process, so it
/// answers `false`. An id that is not a valid process id answers `false` too.
#[must_use]
pub(crate) fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        // A success and `EPERM` both say that the process exists. `EPERM`
        // means that it belongs to another person, so a signal would need a
        // right that this process does not have; `ESRCH` is the answer that
        // says it is gone.
        Ok(()) | Err(rustix::io::Errno::PERM) => true,
        Err(_) => false,
    }
}

/// The longest time that the read of one start time may run.
pub(crate) const START_TIME_BOUND: Duration = Duration::from_secs(2);

/// The start time of `pid`, or `None` when this computer cannot name it.
///
/// Linux answers from `/proc/<pid>/stat` without a program. Every other Unix
/// runs `ps` through the bounded program runner, which a caller must call off
/// the UI thread.
#[must_use]
pub(crate) fn start_time(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    if let Some(value) = linux_start_time(pid) {
        return Some(format!("linux:{value}"));
    }
    unix_start_time(pid, &ProgramEnvironment::of_this_process(), START_TIME_BOUND)
}

/// The start time that the `ps` of the system reports for `pid`.
///
/// This is the reader of every Unix, and a test of Linux calls it directly. The
/// program comes from `programs`, so a test names its own stand-in. The runner
/// clears the environment of the child, so a Mac writes the date in the `C`
/// locale of the shared reader whatever the person set.
#[must_use]
pub(crate) fn unix_start_time(pid: u32, programs: &ProgramEnvironment, bound: Duration) -> Option<String> {
    let program = programs.find(horizon_browser_control::process_info::START_TIME_PROGRAM)?;
    let args: Vec<OsString> = horizon_browser_control::process_info::start_time_arguments(pid)
        .into_iter()
        .map(OsString::from)
        .collect();
    let output = run(Path::new(&program), &args, programs, bound).ok()?;
    if !output.status.success() {
        return None;
    }
    horizon_browser_control::process_info::parse_start_time(&output.stdout)
}

#[cfg(target_os = "linux")]
fn linux_start_time(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// The start time of `pid` as a value that fills itself: `/proc` on Linux, and
/// one thread on the systems that run `ps`, so the caller of a frame only reads
/// the value later.
#[must_use]
pub(crate) fn start_identity(pid: u32) -> Arc<OnceLock<Option<String>>> {
    let identity = Arc::new(OnceLock::new());
    #[cfg(target_os = "linux")]
    if let Some(value) = linux_start_time(pid) {
        let _ = identity.set(Some(format!("linux:{value}")));
        return identity;
    }
    let cell = Arc::clone(&identity);
    let _ = std::thread::Builder::new()
        .name("process-identity".to_owned())
        .spawn(move || {
            let _ = cell.set(start_time(pid));
        });
    identity
}

/// The id and the start time of a process that this Horizon started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnedProcess {
    pub(crate) pid: u32,
    pub(crate) start_time: String,
}

impl OwnedProcess {
    /// Whether the id still names the process that this value recorded, and not
    /// one that the system gave the same id after it.
    #[must_use]
    pub(crate) fn still_names_this_process(&self) -> bool {
        start_time(self.pid).as_deref() == Some(self.start_time.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::{START_TIME_BOUND, is_alive, start_time, unix_start_time};
    use crate::bounded_program::ProgramEnvironment;

    /// A child that ended and was reaped, so its id names no process.
    fn reaped_child_pid() -> u32 {
        let mut child = Command::new(crate::util::stand_in_program())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a stand-in program");
        let pid = child.id();
        child.wait().expect("the child ends");
        pid
    }

    #[test]
    fn this_process_is_alive() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn a_child_that_ended_and_was_reaped_is_gone() {
        assert!(!is_alive(reaped_child_pid()));
    }

    #[test]
    fn pid_one_is_alive_with_either_answer_of_signal_zero() {
        // A success and `EPERM` both say that the process exists. Every Unix
        // that runs this test has a process 1.
        assert!(is_alive(1));
    }

    #[test]
    fn an_id_that_is_not_a_process_is_gone() {
        for pid in [0, u32::MAX] {
            assert!(!is_alive(pid), "{pid}");
        }
    }

    #[test]
    fn the_unix_reader_names_one_start_time_twice() {
        let programs = ProgramEnvironment::of_this_process();
        let pid = std::process::id();
        let first = unix_start_time(pid, &programs, START_TIME_BOUND).expect("the start time of this process");
        let second = unix_start_time(pid, &programs, START_TIME_BOUND).expect("the start time again");
        assert_eq!(first, second, "a process keeps one start time");
        assert!(start_time(pid).is_some(), "the reader of this system names it");
    }

    #[test]
    fn the_unix_reader_has_no_start_time_of_a_process_that_is_gone() {
        let programs = ProgramEnvironment::of_this_process();
        let pid = reaped_child_pid();
        assert_eq!(unix_start_time(pid, &programs, START_TIME_BOUND), None);
        assert_eq!(start_time(pid), None);
    }
}
