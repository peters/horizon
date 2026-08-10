//! Bounded Unix login-shell environment probing.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{ProbeEnvironment, available_executable, executable_file_state, path_candidates};
use crate::app::settings::speech::agent_setup::detection::SpeechSetupAgentAvailability;

pub(in crate::app::settings::speech::agent_setup::detection) const LOGIN_SHELL_ENV_COMMAND: &str = "env";
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(in crate::app::settings::speech::agent_setup::detection) const MAX_LOGIN_SHELL_LINE_BYTES: usize = 256 * 1024;
pub(in crate::app::settings::speech::agent_setup::detection) const MAX_LOGIN_SHELL_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub(in crate::app::settings::speech::agent_setup::detection) enum LoginShellProbe {
    Available(String),
    Missing,
    Timeout,
    Failed(String),
}

pub(in crate::app::settings::speech::agent_setup::detection) fn probe_login_shell(
    shell: &Path,
    candidate: &str,
    environment: &ProbeEnvironment,
    timeout: Duration,
) -> LoginShellProbe {
    let mut command = Command::new(shell);
    command
        .args(login_shell_probe_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = environment.cwd.as_deref() {
        command.current_dir(cwd);
    }
    if let Some(path) = environment.path.as_deref() {
        command.env("PATH", path);
    }
    if let Some(home) = environment.home.as_deref() {
        command.env("HOME", home);
    }

    let output = run_bounded_with_path_capture(&mut command, timeout);
    match output.command {
        BoundedCommandResult::Exited(status) if status.success() => {
            let path = match output.path {
                Ok(Some(path)) => path,
                Ok(None) => {
                    return LoginShellProbe::Failed("login-shell environment probe returned no PATH".to_string());
                }
                Err(error) => return LoginShellProbe::Failed(error),
            };
            if path.is_empty() {
                return LoginShellProbe::Failed("login-shell environment probe returned no PATH".to_string());
            }
            resolve_login_shell_candidate(candidate, environment, path)
        }
        BoundedCommandResult::Exited(_) => {
            LoginShellProbe::Failed("login shell exited before the environment probe completed".to_string())
        }
        BoundedCommandResult::Timeout => LoginShellProbe::Timeout,
        BoundedCommandResult::Failed(error) => LoginShellProbe::Failed(error),
    }
}

fn resolve_login_shell_candidate(candidate: &str, environment: &ProbeEnvironment, path: OsString) -> LoginShellProbe {
    let mut login_environment = environment.clone();
    login_environment.path = Some(path);
    let mut first_error = None;
    for resolved in path_candidates(candidate, &login_environment) {
        match executable_file_state(&resolved) {
            Ok(true) => {
                let SpeechSetupAgentAvailability::Available { executable } =
                    available_executable(resolved, &login_environment)
                else {
                    return LoginShellProbe::Failed("resolved executable path is not valid UTF-8".to_string());
                };
                return LoginShellProbe::Available(executable);
            }
            Ok(false) => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(format!("could not inspect login-shell candidate: {error}"));
                }
            }
        }
    }
    first_error.map_or(LoginShellProbe::Missing, LoginShellProbe::Failed)
}

pub(in crate::app::settings::speech::agent_setup::detection) fn login_shell_probe_args() -> [OsString; 2] {
    [OsString::from("-lic"), OsString::from(LOGIN_SHELL_ENV_COMMAND)]
}

#[cfg(test)]
pub(in crate::app::settings::speech::agent_setup::detection) fn login_shell_path(output: &[u8]) -> Option<OsString> {
    read_login_shell_path(output).ok().flatten()
}

#[cfg(test)]
pub(in crate::app::settings::speech::agent_setup::detection) fn read_login_shell_path(
    mut output: impl Read,
) -> io::Result<Option<OsString>> {
    let mut capture = LoginShellPathCapture::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = output.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        capture.push(&chunk[..read])?;
    }
    capture.finish()
}

struct LoginShellPathCapture {
    total_bytes: usize,
    line: Vec<u8>,
    line_overflowed: bool,
    last_path: Option<OsString>,
}

impl LoginShellPathCapture {
    fn new() -> Self {
        Self {
            total_bytes: 0,
            line: Vec::with_capacity(1024),
            line_overflowed: false,
            last_path: None,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.total_bytes = self.total_bytes.checked_add(bytes.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "login-shell output exceeded the capture limit",
            )
        })?;
        if self.total_bytes > MAX_LOGIN_SHELL_OUTPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "login-shell output exceeded the capture limit",
            ));
        }

        for byte in bytes {
            if *byte == b'\n' {
                self.finish_line()?;
            } else if self.line.len() < MAX_LOGIN_SHELL_LINE_BYTES {
                self.line.push(*byte);
            } else {
                self.line_overflowed = true;
            }
        }
        Ok(())
    }

    fn finish(mut self) -> io::Result<Option<OsString>> {
        if !self.line.is_empty() || self.line_overflowed {
            self.finish_line()?;
        }
        Ok(self.last_path)
    }

    fn finish_line(&mut self) -> io::Result<()> {
        use std::os::unix::ffi::OsStringExt;

        if let Some(path) = self.line.strip_prefix(b"PATH=") {
            if self.line_overflowed {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "login-shell PATH exceeded the capture limit",
                ));
            }
            self.last_path = Some(OsString::from_vec(path.to_vec()));
        }
        self.line.clear();
        self.line_overflowed = false;
        Ok(())
    }
}

#[derive(Debug)]
pub(in crate::app::settings::speech::agent_setup::detection) enum BoundedCommandResult {
    Exited(std::process::ExitStatus),
    Timeout,
    Failed(String),
}

struct BoundedLoginShellOutput {
    command: BoundedCommandResult,
    path: Result<Option<OsString>, String>,
}

#[cfg(test)]
pub(in crate::app::settings::speech::agent_setup::detection) fn run_bounded(
    command: &mut Command,
    timeout: Duration,
) -> BoundedCommandResult {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return BoundedCommandResult::Failed(format!("failed to launch login-shell probe: {error}"));
        }
    };
    wait_for_bounded_child(&mut child, timeout)
}

fn run_bounded_with_path_capture(command: &mut Command, timeout: Duration) -> BoundedLoginShellOutput {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return BoundedLoginShellOutput {
                command: BoundedCommandResult::Failed(format!("failed to launch login-shell probe: {error}")),
                path: Err("login-shell environment capture did not start".to_string()),
            };
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        terminate_bounded_child(&mut child);
        return BoundedLoginShellOutput {
            command: BoundedCommandResult::Failed("login-shell environment pipe was unavailable".to_string()),
            path: Err("login-shell environment capture did not start".to_string()),
        };
    };
    let stdout_flags = match rustix::fs::fcntl_getfl(&stdout) {
        Ok(flags) => flags,
        Err(error) => {
            terminate_bounded_child(&mut child);
            return BoundedLoginShellOutput {
                command: BoundedCommandResult::Failed(format!("failed to inspect login-shell output pipe: {error}")),
                path: Err("login-shell environment capture did not start".to_string()),
            };
        }
    };
    if let Err(error) = rustix::fs::fcntl_setfl(&stdout, stdout_flags | rustix::fs::OFlags::NONBLOCK) {
        terminate_bounded_child(&mut child);
        return BoundedLoginShellOutput {
            command: BoundedCommandResult::Failed(format!("failed to configure login-shell output pipe: {error}")),
            path: Err("login-shell environment capture did not start".to_string()),
        };
    }

    let started_at = Instant::now();
    let mut capture = LoginShellPathCapture::new();
    let mut exit_status = None;
    loop {
        match drain_login_shell_output(&mut stdout, &mut capture) {
            Ok(true) => {
                if let Some(status) = exit_status {
                    return BoundedLoginShellOutput {
                        command: BoundedCommandResult::Exited(status),
                        path: capture
                            .finish()
                            .map_err(|error| format!("failed to read login-shell environment: {error}")),
                    };
                }
            }
            Ok(false) => {}
            Err(error) => {
                terminate_bounded_child(&mut child);
                return BoundedLoginShellOutput {
                    command: BoundedCommandResult::Failed(format!("failed to read login-shell environment: {error}")),
                    path: Err(error.to_string()),
                };
            }
        }

        if exit_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_status = Some(status);
                    kill_bounded_process_group(&child);
                }
                Ok(None) => {}
                Err(error) => {
                    terminate_bounded_child(&mut child);
                    return BoundedLoginShellOutput {
                        command: BoundedCommandResult::Failed(format!(
                            "failed while waiting for login-shell probe: {error}"
                        )),
                        path: Err("login-shell environment capture did not complete".to_string()),
                    };
                }
            }
        }

        if started_at.elapsed() >= timeout {
            terminate_bounded_child(&mut child);
            return BoundedLoginShellOutput {
                command: BoundedCommandResult::Timeout,
                path: Err("login-shell environment capture timed out".to_string()),
            };
        }
        let remaining = timeout.saturating_sub(started_at.elapsed());
        std::thread::sleep(PROCESS_POLL_INTERVAL.min(remaining));
    }
}

fn drain_login_shell_output(
    stdout: &mut std::process::ChildStdout,
    capture: &mut LoginShellPathCapture,
) -> io::Result<bool> {
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(read) => capture.push(&chunk[..read])?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Ok(false);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
fn wait_for_bounded_child(child: &mut std::process::Child, timeout: Duration) -> BoundedCommandResult {
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return BoundedCommandResult::Exited(status),
            Ok(None) => {}
            Err(error) => {
                terminate_bounded_child(child);
                return BoundedCommandResult::Failed(format!("failed while waiting for login-shell probe: {error}"));
            }
        }

        if started_at.elapsed() >= timeout {
            terminate_bounded_child(child);
            return BoundedCommandResult::Timeout;
        }
        let remaining = timeout.saturating_sub(started_at.elapsed());
        std::thread::sleep(PROCESS_POLL_INTERVAL.min(remaining));
    }
}

fn kill_bounded_process_group(child: &std::process::Child) {
    if let Ok(raw_pid) = i32::try_from(child.id())
        && let Some(process_group) = rustix::process::Pid::from_raw(raw_pid)
    {
        let _ = rustix::process::kill_process_group(process_group, rustix::process::Signal::KILL);
    }
}

fn terminate_bounded_child(child: &mut std::process::Child) {
    kill_bounded_process_group(child);
    let _ = child.kill();
    let _ = child.wait();
}
