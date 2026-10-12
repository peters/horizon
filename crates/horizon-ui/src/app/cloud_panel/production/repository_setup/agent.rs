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
/// starts it as a group leader, and its start time, so a later process that reuses the
/// id is never taken for it. Without a start time Horizon neither waits on nor signals it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentProcess {
    id: u32,
    started: Option<String>,
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
    pub(super) fn end(&self) -> bool {
        if self.ended_within(GRACE) {
            return true;
        }
        self.kill();
        self.ended_within(KILL_WAIT)
    }

    /// Whether the agent and its group have ended, waiting up to `wait`.
    fn ended_within(&self, wait: Duration) -> bool {
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

    /// The id still names the agent's group: the agent itself runs, or no process has
    /// the id and only what the agent started may be left in its group. An id that a
    /// newer process took means the agent's group had already emptied, since no process
    /// can take the id of a group that still has members.
    #[cfg(unix)]
    fn group(&self) -> Option<rustix::process::Pid> {
        self.started.as_ref()?;
        let current = start_time(self.id);
        if current.is_some() && current != self.started {
            return None;
        }
        i32::try_from(self.id).ok().and_then(rustix::process::Pid::from_raw)
    }

    #[cfg(unix)]
    fn alive(&self) -> bool {
        self.group()
            .is_some_and(|group| rustix::process::test_kill_process_group(group).is_ok())
    }

    #[cfg(not(unix))]
    fn alive(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn kill(&self) {
        if let Some(group) = self.group() {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }

    #[cfg(not(unix))]
    fn kill(&self) {}
}

/// When the process `id` started, or none when no process has the id: on Linux its
/// start in clock ticks since boot, on other Unix systems as `ps` reports it.
fn start_time(id: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{id}/stat"))
            .ok()?
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)
            .map(str::to_owned)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &id.to_string()])
            .output()
            .ok()?;
        let started = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !started.is_empty()).then_some(started)
    }
    #[cfg(not(unix))]
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

    #[test]
    fn an_id_a_newer_process_took_is_never_signalled() {
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let reused = AgentProcess {
            id: other.id(),
            started: Some("an earlier start".to_owned()),
        };
        assert!(
            reused.end(),
            "a process that is not the agent counts as the agent ended"
        );
        assert!(other.try_wait().unwrap().is_none(), "and it is left alone");
        other.kill().unwrap();
        other.wait().unwrap();
    }
}
