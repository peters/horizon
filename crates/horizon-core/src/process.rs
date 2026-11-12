//! The presence of one process of this computer.
//!
//! A process id alone is a weak name: the system gives the same id again after
//! the process ends. Signal 0 answers whether an id still names a process on
//! every Unix: `ESRCH` says that no such process exists, and `EPERM` says that
//! it exists and belongs to another person.
//!
//! The read uses `rustix`, which the workspace already holds, and no shell of
//! the person. A caller that runs during a frame calls [`is_alive`]; it starts
//! no program and waits for no file.

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

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::is_alive;

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
}
