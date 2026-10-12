//! The setup agent's process. Horizon records it when the agent's panel starts and ends
//! it, with everything it started, before it reads a setup for Use or lets a run go:
//! closing a terminal only hangs up, and an agent or a command it started may ignore
//! that and keep writing to the repository. On Linux with a systemd user manager the
//! agent goes into a scope of its own, a cgroup that holds every process it starts, even
//! one that leaves its process group; elsewhere its process group is what Horizon ends.
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
    /// The agent's scope, when it has one.
    scope: Option<Scope>,
}

/// A systemd user scope and its cgroup folder, found once the scope exists: `systemd-run`
/// may still be making it when the agent's panel has just started.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    unit: String,
    cgroup: std::sync::OnceLock<std::path::PathBuf>,
}

impl Scope {
    fn new(unit: String) -> Self {
        Self {
            unit,
            cgroup: std::sync::OnceLock::new(),
        }
    }

    /// The scope's cgroup folder; none while systemd has no such scope, which also
    /// means nothing runs in it.
    fn cgroup(&self) -> Option<&std::path::Path> {
        if let Some(cgroup) = self.cgroup.get() {
            return Some(cgroup);
        }
        let found = cgroup_of(&self.unit)?;
        Some(self.cgroup.get_or_init(|| found))
    }
}

impl AgentProcess {
    /// The agent process `id`, which the terminal started inside the scope `unit`.
    pub(super) fn in_scope(id: u32, unit: &str) -> Self {
        Self {
            id,
            started: start_time(id),
            scope: Some(Scope::new(unit.to_owned())),
        }
    }

    /// The agent process `id`, started without a scope: Horizon moves it into one now
    /// where it can, which holds what it starts from then on.
    pub(super) fn of(id: u32) -> Self {
        let started = start_time(id);
        // Only the process Horizon just started goes into the scope, checked by its start.
        let scope = started.is_some().then(|| contain(id)).flatten();
        Self { id, started, scope }
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
        // The scope holds everything the agent started: it must be empty.
        let in_scope = self.scope.as_ref().and_then(Scope::cgroup).is_some_and(|cgroup| {
            std::fs::read_to_string(cgroup.join("cgroup.procs")).is_ok_and(|procs| !procs.trim().is_empty())
        });
        in_scope
            || self
                .group()
                .is_some_and(|group| rustix::process::test_kill_process_group(group).is_ok())
    }

    #[cfg(not(unix))]
    fn alive(&self) -> bool {
        false
    }

    #[cfg(unix)]
    fn kill(&self) {
        if let Some(scope) = &self.scope {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "kill", "--signal=SIGKILL", &scope.unit])
                .output();
        }
        if let Some(group) = self.group() {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }

    #[cfg(not(unix))]
    fn kill(&self) {}
}

/// Moves the process `id` into a new transient scope of the systemd user manager, so
/// every process it starts from now on stays in that cgroup; the scope and its cgroup
/// folder, or none without a user manager or off Linux.
fn contain(id: u32) -> Option<Scope> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let unit = format!("horizon-setup-agent-{id}-{}.scope", std::process::id());
    let created = std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            &unit,
            "fail",
            "1",
            "PIDs",
            "au",
            "1",
            &id.to_string(),
            "0",
        ])
        .output()
        .is_ok_and(|output| output.status.success());
    if !created {
        return None;
    }
    let scope = Scope::new(unit);
    scope.cgroup()?;
    Some(scope)
}

/// The cgroup folder of the systemd user scope `unit`.
fn cgroup_of(unit: &str) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("systemctl")
        .args(["--user", "show", "--property=ControlGroup", "--value", unit])
        .output()
        .ok()?;
    let group = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let cgroup = std::path::Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    (!group.is_empty() && cgroup.is_dir()).then_some(cgroup)
}

/// The command line that starts a program inside a new systemd user scope `unit`, or
/// none where Horizon cannot make one: everything the program starts is held in it
/// from its first instruction.
pub(super) fn scope_launcher(unit: &str) -> Option<Vec<String>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let works = std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    works.then(|| {
        ["systemd-run", "--user", "--scope", "--quiet", "--collect"]
            .into_iter()
            .map(str::to_owned)
            .chain([format!("--unit={unit}"), "--".to_owned()])
            .collect()
    })
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
    fn a_process_that_leaves_the_group_still_ends_with_the_agent() {
        // The shell starts a sleep in a session of its own, outside its process group.
        let mut agent = std::process::Command::new("sh")
            .args(["-c", "setsid sleep 30 & sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let process = AgentProcess::of(agent.id());
        let Some(cgroup) = process
            .scope
            .as_ref()
            .and_then(Scope::cgroup)
            .map(std::path::Path::to_path_buf)
        else {
            agent.kill().unwrap();
            agent.wait().unwrap();
            return; // No systemd user manager here: only the process group can be ended.
        };
        std::thread::sleep(Duration::from_millis(300));
        let reaper = std::thread::spawn(move || agent.wait());
        assert!(process.end(), "the agent and the sleep that left its group are gone");
        reaper.join().unwrap().unwrap();
        let left = std::fs::read_to_string(cgroup.join("cgroup.procs")).unwrap_or_default();
        assert!(left.trim().is_empty(), "nothing is left in the scope: {left}");
    }

    #[test]
    fn an_agent_started_in_its_scope_ends_with_all_it_started() {
        let unit = format!("horizon-setup-agent-test-{}.scope", std::process::id());
        let Some(launcher) = scope_launcher(&unit) else {
            return; // No systemd user manager here.
        };
        let mut agent = std::process::Command::new(&launcher[0])
            .args(&launcher[1..])
            .args(["sh", "-c", "setsid sleep 30 & sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let started = Instant::now();
        let process = loop {
            let process = AgentProcess::in_scope(agent.id(), &unit);
            if process.scope.as_ref().and_then(Scope::cgroup).is_some() || started.elapsed() > Duration::from_secs(5) {
                break process;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let cgroup = process
            .scope
            .as_ref()
            .and_then(Scope::cgroup)
            .map(std::path::Path::to_path_buf)
            .expect("the agent runs in its scope");
        std::thread::sleep(Duration::from_millis(300));
        let reaper = std::thread::spawn(move || agent.wait());
        assert!(process.end(), "everything in the scope is gone");
        reaper.join().unwrap().unwrap();
        let left = std::fs::read_to_string(cgroup.join("cgroup.procs")).unwrap_or_default();
        assert!(left.trim().is_empty(), "{left}");
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
            scope: None,
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
