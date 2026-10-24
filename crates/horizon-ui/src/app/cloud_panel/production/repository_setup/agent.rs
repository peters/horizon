//! The setup agent's process. Horizon records it when the agent's panel starts and ends
//! it, with everything it started, before it reads a setup for Use or lets a run go:
//! closing a terminal only hangs up, and an agent or a command it started may ignore
//! that and keep writing to the repository. The agent starts in a systemd user scope of
//! its own, a cgroup that holds every process it starts as its descendant, even one that
//! leaves its process group; Horizon starts no setup agent where it cannot make one. The
//! scope is not a sandbox: the agent runs as the person, and a program it deliberately
//! hands to another service of theirs, such as Docker or their systemd user manager, runs
//! outside it, as any program they start does. What Use commits does not rest on the
//! scope: it is the setup Horizon proved, read after the agent ended, with every other
//! change named first.
use std::time::{Duration, Instant};

/// How long the agent may take to end on its own once its panel closed.
const GRACE: Duration = Duration::from_millis(1500);

/// How long Horizon waits for the processes it killed to go.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// How long Horizon waits before it ends processes that outlived a kill once more.
const RETRY: Duration = Duration::from_secs(1);

/// The most one command to the systemd user manager may take, so a manager that does
/// not answer never holds Stop, Use, a new setup or Horizon's exit.
const SYSTEMD_TIMEOUT: Duration = Duration::from_secs(10);

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

    /// What systemd says of the scope now. Its cgroup folder, once found, is kept: one
    /// that is gone means the scope ended.
    #[cfg(unix)]
    fn state(&self) -> ScopeState {
        if let Some(cgroup) = self.cgroup.get() {
            return if cgroup.is_dir() {
                ScopeState::Running(cgroup.clone())
            } else {
                ScopeState::Gone
            };
        }
        let state = scope_state(&self.unit);
        if let ScopeState::Running(cgroup) = &state {
            let _ = self.cgroup.set(cgroup.clone());
        }
        state
    }
}

/// What systemd says of a scope.
#[cfg(unix)]
#[derive(Debug)]
enum ScopeState {
    /// It runs, with this cgroup folder.
    Running(std::path::PathBuf),
    /// systemd has no such scope, or it ended: nothing runs in it.
    Gone,
    /// systemd did not answer, or the scope is still being made: counted as running,
    /// so nothing is taken for ended that systemd has not shown ended.
    Unknown,
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

    /// Ends the agent and its group: they get a short grace after the hang-up of the
    /// closed panel, then are killed. Returns whether all of them have ended.
    pub(super) fn end(&self) -> bool {
        if self.ended_within(GRACE) {
            return true;
        }
        self.kill();
        self.ended_within(KILL_WAIT)
    }

    /// Ends the agent and everything it started, trying again until all of them have
    /// gone: a process killed while it waits on a device or a file system goes once that
    /// wait ends, and until then the run that holds it, a new setup and Horizon's exit
    /// wait for it.
    pub(super) fn end_completely(&self) {
        while !self.end() {
            std::thread::sleep(RETRY);
        }
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
        // The scope holds every descendant of the agent: systemd must show it empty.
        let in_scope = match self.scope.as_ref().map(Scope::state) {
            None | Some(ScopeState::Gone) => false,
            Some(ScopeState::Unknown) => true,
            Some(ScopeState::Running(cgroup)) => std::fs::read_to_string(cgroup.join("cgroup.procs"))
                .map_or_else(|_| cgroup.is_dir(), |procs| !procs.trim().is_empty()),
        };
        in_scope
            || self
                .group()
                .is_some_and(|group| rustix::process::test_kill_process_group(group).is_ok())
    }

    #[cfg(unix)]
    fn kill(&self) {
        if let Some(scope) = &self.scope {
            let mut kill = std::process::Command::new("systemctl");
            kill.args(["--user", "kill", "--signal=SIGKILL", &scope.unit]);
            let _ = run_bounded(kill, SYSTEMD_TIMEOUT);
        }
        if let Some(group) = self.group() {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }
}

/// What systemd says of the user scope `unit`: running with its cgroup folder, gone when
/// systemd does not know it or it ended, and unknown when systemd does not answer in time
/// or the scope is still being made.
#[cfg(unix)]
fn scope_state(unit: &str) -> ScopeState {
    let mut show = std::process::Command::new("systemctl");
    show.args(["--user", "show", "--property=LoadState,ActiveState,ControlGroup", unit]);
    let Some(output) = run_bounded(show, SYSTEMD_TIMEOUT).filter(|output| output.status.success()) else {
        return ScopeState::Unknown;
    };
    read_scope(&String::from_utf8_lossy(&output.stdout))
}

/// The scope state in what `systemctl show` printed for it.
#[cfg(unix)]
fn read_scope(text: &str) -> ScopeState {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_default()
            .trim()
    };
    if value("LoadState") == "not-found" || matches!(value("ActiveState"), "inactive" | "failed") {
        return ScopeState::Gone;
    }
    let group = value("ControlGroup");
    if group.is_empty() {
        return ScopeState::Unknown;
    }
    let cgroup = std::path::Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    if cgroup.is_dir() {
        ScopeState::Running(cgroup)
    } else {
        ScopeState::Gone
    }
}

/// Runs `command` for at most `limit`: its exit status and standard output, or none when
/// it could not start or did not end in time, ended then with what it started.
fn run_bounded(mut command: std::process::Command, limit: Duration) -> Option<std::process::Output> {
    use std::io::Read as _;
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(64 * 1024).read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(std::process::Output {
                    status,
                    stdout: reader.join().unwrap_or_default(),
                    stderr: Vec::new(),
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                #[cfg(unix)]
                if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
                    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The command line that starts a program inside a new systemd user scope `unit`, or
/// none where Horizon cannot make one: everything the program starts is held in it
/// from its first instruction. A setup agent starts on Linux only, where its dialog is
/// offered, so the probe finds a systemd user manager only where a scope can exist: a
/// Linux computer without one, or another system, gives none.
pub(super) fn scope_launcher(unit: &str) -> Option<Vec<String>> {
    let mut probe = std::process::Command::new("systemd-run");
    probe.args(["--user", "--scope", "--quiet", "--collect", "true"]);
    let works = run_bounded(probe, SYSTEMD_TIMEOUT).is_some_and(|output| output.status.success());
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
        let mut ps = std::process::Command::new("ps");
        ps.args(["-o", "lstart=", "-p", &id.to_string()]);
        let output = run_bounded(ps, SYSTEMD_TIMEOUT)?;
        let started = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !started.is_empty()).then_some(started)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;

    /// An agent process without a scope, which its group alone ends.
    fn grouped(id: u32) -> AgentProcess {
        AgentProcess {
            id,
            started: start_time(id),
            scope: None,
        }
    }

    #[test]
    fn an_agent_that_ignores_the_hang_up_is_ended_with_its_group() {
        let mut agent = std::process::Command::new("sh")
            .args(["-c", "trap '' HUP; sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let process = grouped(agent.id());
        assert!(!process.ended_within(Duration::from_millis(200)), "it still runs");
        let reaper = std::thread::spawn(move || agent.wait());
        assert!(process.end(), "the agent and its sleep are gone");
        reaper.join().unwrap().unwrap();
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
            let running = matches!(process.scope.as_ref().map(Scope::state), Some(ScopeState::Running(_)));
            if running || started.elapsed() > Duration::from_secs(5) {
                break process;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let Some(ScopeState::Running(cgroup)) = process.scope.as_ref().map(Scope::state) else {
            panic!("the agent runs in its scope");
        };
        std::thread::sleep(Duration::from_millis(300));
        let reaper = std::thread::spawn(move || agent.wait());
        assert!(process.end(), "everything in the scope is gone");
        reaper.join().unwrap().unwrap();
        let left = std::fs::read_to_string(cgroup.join("cgroup.procs")).unwrap_or_default();
        assert!(left.trim().is_empty(), "{left}");
    }

    #[test]
    fn only_what_systemd_shows_ended_counts_as_gone() {
        assert!(matches!(
            read_scope("LoadState=not-found\nActiveState=inactive\nControlGroup=\n"),
            ScopeState::Gone
        ));
        assert!(matches!(
            read_scope("LoadState=loaded\nActiveState=failed\nControlGroup=\n"),
            ScopeState::Gone
        ));
        assert!(
            matches!(
                read_scope("LoadState=loaded\nActiveState=activating\nControlGroup=\n"),
                ScopeState::Unknown
            ),
            "a scope still being made is not taken for ended"
        );
        assert!(
            matches!(read_scope(""), ScopeState::Unknown),
            "nor an answer that names nothing"
        );
        assert!(matches!(
            read_scope("LoadState=loaded\nActiveState=active\nControlGroup=/no/such/horizon/cgroup\n"),
            ScopeState::Gone
        ));
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
