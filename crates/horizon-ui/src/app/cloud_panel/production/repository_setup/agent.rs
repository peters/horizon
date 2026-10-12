//! The setup agent's process. Horizon records it when the agent's panel starts and ends
//! it, with everything in its process group, before it reads a setup for Use or lets a
//! run go: closing a terminal only hangs up, and an agent or a command it started may
//! ignore that and keep writing to the repository.
use std::time::{Duration, Instant};

/// How long the agent may take to end on its own once its panel closed.
const GRACE: Duration = Duration::from_millis(1500);

/// How long Horizon waits for the processes it killed to go.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// The agent's process: its id, which also names its process group because the terminal
/// starts it as a group leader, and on Linux its start time, so a later process that
/// reuses the id is never taken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AgentProcess {
    id: u32,
    started: Option<u64>,
}

impl AgentProcess {
    pub(super) fn of(id: u32) -> Self {
        Self {
            id,
            started: start_time(id),
        }
    }

    /// Ends the agent and its group: they get a short grace after the hang-up of the
    /// closed panel, then are killed. Returns whether all of them have ended.
    pub(super) fn end(self) -> bool {
        if self.ended_within(GRACE) {
            return true;
        }
        self.kill();
        self.ended_within(KILL_WAIT)
    }

    /// Whether the agent and its group have ended, waiting up to `wait`.
    fn ended_within(self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if !self.alive() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    fn alive(self) -> bool {
        let Some(id) = self.pid() else {
            return false;
        };
        // While any process of the group lives, no new process can take its id, so the
        // group is the agent's own.
        rustix::process::test_kill_process_group(id).is_ok()
            || (rustix::process::test_kill_process(id).is_ok() && start_time(self.id) == self.started)
    }

    #[cfg(not(unix))]
    fn alive(self) -> bool {
        false
    }

    #[cfg(unix)]
    fn kill(self) {
        let Some(id) = self.pid() else {
            return;
        };
        let _ = rustix::process::kill_process_group(id, rustix::process::Signal::KILL);
        if start_time(self.id) == self.started {
            let _ = rustix::process::kill_process(id, rustix::process::Signal::KILL);
        }
    }

    #[cfg(not(unix))]
    fn kill(self) {}

    #[cfg(unix)]
    fn pid(self) -> Option<rustix::process::Pid> {
        i32::try_from(self.id).ok().and_then(rustix::process::Pid::from_raw)
    }
}

/// When the process `id` started, in clock ticks since boot; elsewhere than Linux none.
fn start_time(id: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{id}/stat"))
            .ok()?
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)?
            .parse()
            .ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;

    #[test]
    fn an_agent_that_ignores_the_hang_up_is_ended_with_its_group() {
        let mut agent = std::process::Command::new("sh")
            .args(["-c", "trap '' HUP; sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let process = AgentProcess::of(agent.id());
        assert!(!process.ended_within(Duration::from_millis(200)), "it still runs");
        let reaper = std::thread::spawn(move || agent.wait());
        assert!(process.end(), "the agent and its sleep are gone");
        reaper.join().unwrap().unwrap();
    }
}
