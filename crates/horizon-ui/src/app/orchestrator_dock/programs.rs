//! The identity of one program of a conversation, for the end that its stop did
//! not reach.
//!
//! [`super::worker::Worker`] stops a bridge: it closes the standard input of the
//! program, waits for it and ends it by its process id within
//! [`horizon_core::agent_bridge::STOP_TIMEOUT`]. The close of Horizon waits for
//! those stops, and a worker that does not finish one leaves a program that would
//! outlive Horizon. The dock records the identity of each such program, and the
//! exit ends it by that identity.
//!
//! The identity is the process id together with the moment that the process
//! started, which the process table names on both platforms. A record that no
//! longer names the same moment ends nothing: the system may have given the id to
//! another program after the recorded one ended.
//!
//! The signal leaves through `/bin/kill`, as the process tree of a browser does
//! (`horizon-browser`), because this crate builds no unsafe call and sends no
//! signal of its own. A signal is not a wait: the answer of [`Program::end`] comes
//! from the process table, which names the end of the program.

use std::time::{Duration, Instant};

/// The time between two reads of the process table.
const POLL: Duration = Duration::from_millis(5);

/// The moment that the termination signal gets before the kill signal, which an
/// exit that already holds its thread gives each program.
pub(super) const END_GRACE: Duration = Duration::from_millis(250);

/// One program of a conversation that a stop of the dock still owes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Program {
    pid: u32,
    /// The moment that the process started, as the process table named it when
    /// the stop of this program went to the worker.
    started: Option<String>,
}

impl Program {
    /// The program of `pid`, with the start of that process.
    pub(super) fn record(pid: u32) -> Self {
        Self {
            pid,
            started: state_of(pid).map(|state| state.started),
        }
    }

    /// The process id of this program.
    pub(super) fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the program of this record runs now: the process table names the
    /// same start moment, and the process is no zombie of an ended program.
    pub(super) fn still_runs(&self) -> bool {
        let Some(recorded) = self.started.as_deref() else {
            return false;
        };
        state_of(self.pid).is_some_and(|state| state.running && state.started == recorded)
    }

    /// Ends this program: the termination signal, the moment that `grace` gives
    /// it, and then the kill signal, which no program ignores. A frame passes no
    /// grace, and an exit that already holds its thread passes a moment. The
    /// answer says whether the program ended.
    pub(super) fn end(&self, grace: Duration) -> bool {
        if !self.still_runs() {
            return true;
        }
        signal(self.pid, "TERM");
        if wait_for_end(self, grace) {
            return true;
        }
        signal(self.pid, "KILL");
        wait_for_end(self, grace.max(POLL))
    }
}

/// Waits for at most `grace` for the program of `program` to end.
fn wait_for_end(program: &Program, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if !program.still_runs() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(now)));
    }
}

/// Sends the signal `name` to the program of `pid`. A program that ended between
/// the read of the process table and this call takes no signal of a later one,
/// because `/bin/kill` sends it to the id of the moment.
fn signal(pid: u32, name: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .arg(format!("-{name}"))
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The state of one process: whether it runs, and the moment that it started.
struct State {
    running: bool,
    started: String,
}

/// The state of the process of `pid`, as the process table of this system names
/// it, or `None` when no process holds that id.
#[cfg(target_os = "linux")]
fn state_of(pid: u32) -> Option<State> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The fields after the name of the program, which is in parentheses and may
    // hold a space: the state is the first of them and the start is the
    // twentieth, because the state is the third field of the line.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let state = *fields.first()?;
    let started = fields.get(19)?;
    Some(State {
        running: !matches!(state, "Z" | "X" | "x"),
        started: (*started).to_owned(),
    })
}

/// The state of the process of `pid`, as the process table of this system names
/// it, or `None` when no process holds that id.
#[cfg(not(target_os = "linux"))]
fn state_of(pid: u32) -> Option<State> {
    let output = std::process::Command::new("ps")
        .args(["-o", "state=", "-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut fields = text.split_whitespace();
    let state = fields.next()?;
    let started = fields.collect::<Vec<_>>().join(" ");
    (!started.is_empty()).then(|| State {
        running: !state.starts_with('Z'),
        started,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::Program;

    /// A program of this computer that ignores the termination signal, after it
    /// wrote `ready`, which says that its signal is in place.
    fn stubborn(ready: &std::path::Path) -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .args([
                "-c",
                &format!("trap '' TERM; : > '{}'; while :; do sleep 0.05; done", ready.display()),
            ])
            .spawn()
            .expect("a program of the test")
    }

    /// Waits until the program of the test names itself as `ready`.
    fn wait_until_ready(ready: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "the program of the test never set its signal"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until the process table drops the program of `program`, which a
    /// loaded machine can take a moment to do.
    fn wait_until_gone(program: &Program) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while program.still_runs() {
            assert!(Instant::now() < deadline, "the program of the test never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_program_that_ignores_the_termination_signal_ends() {
        let folder = tempfile::tempdir().expect("a folder of the test");
        let ready = folder.path().join("ready");
        let mut child = stubborn(&ready);
        wait_until_ready(&ready);
        let program = Program::record(child.id());
        assert!(program.still_runs(), "the program of the test runs");

        assert!(
            program.end(Duration::from_millis(200)),
            "the kill signal ends a program that ignores the termination signal"
        );
        wait_until_gone(&program);
        let _ = child.wait();
    }

    /// A process that a test holds for its start moment.
    fn held() -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .args(["-c", "exec sleep 600"])
            .spawn()
            .expect("a program of the test")
    }

    #[test]
    fn a_record_of_a_program_that_ended_ends_nothing() {
        let mut child = held();
        let program = Program::record(child.id());
        let _ = child.kill();
        let _ = child.wait();

        assert!(!program.still_runs(), "the program of the test ended");
        assert!(
            program.end(Duration::from_millis(50)),
            "a program that ended needs no signal"
        );
    }

    #[test]
    fn a_record_holds_the_start_of_its_process() {
        let mut child = held();
        let program = Program::record(child.id());
        assert!(program.still_runs());
        assert_eq!(
            Program::record(child.id()),
            program,
            "the identity of one process holds for two reads"
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(program.pid(), child.id());
    }
}
