//! One bounded command inside a cloud worker, run as the worker's unprivileged agent user.
//!
//! Horizon enters the worker over the recorded SSH route, which pins the host key and the
//! identity file, and hands the command to the worker's own launcher
//! (`horizon-worker-tailnet agent`). That launcher refuses to start without agent
//! isolation and drops to uid 10001 with `HOME=/workspace/home`, so `git` and `gh` there
//! are the brokered agent-user ones and no token is in the process. Horizon never offers a
//! user or a uid, and it refuses rather than sanitizes a command that would switch user.
use super::{Cancellation, Error, command::Runner, lifecycle, settings::Settings, ssh::Connection};
use horizon_browser_control::manifest::exec::{MAX_COMMAND_BYTES, MAX_TIMEOUT_MILLIS, MIN_TIMEOUT_MILLIS};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// The account every isolated worker's launcher drops to, and the only one this tool runs as.
pub const AGENT_UID: u32 = 10001;
/// The most text of either stream that comes back to the person's Horizon. A command that
/// writes more is a failure, not a silently cut answer.
pub const MAX_ANSWER_BYTES: u64 = 256 * 1024;
/// The launcher's own refusals, returned as exit codes. Horizon reads them only when the
/// frame is absent, so a command that exits with one of them keeps its own status.
const REFUSED_NO_ISOLATION: i32 = 20;
const REFUSED_NOT_ROOT: i32 = 21;
const REFUSED_NO_LAUNCHER: i32 = 22;
/// OpenSSH reports its own failures, such as an unreachable worker, with this code.
const SSH_FAILURE: i32 = 255;
const UID_MARKER: &str = "HORIZON-EXEC uid=";
const LAUNCHER: &str = "/usr/local/bin/horizon-worker-tailnet";
/// The worker's record that agents are isolated, which its GitHub service requires too.
const ISOLATION_MARKER: &str = "/run/horizon-tailnet/agent-isolation";
/// Programs that would change the account the command runs as.
const USER_SWITCH: [&str; 8] = [
    "su", "sudo", "doas", "setpriv", "runuser", "pkexec", "nsenter", "chroot",
];

/// Wraps `value` in single quotes for a POSIX shell, so the shell reads it as one literal
/// word. Each embedded quote is closed, escaped and reopened, which no shell interprets.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Where a command runs: the cloud's own record and the route it pins to its worker.
pub struct Target<'a> {
    /// The cloud's store directory, which holds the host key Horizon pinned for its worker.
    pub directory: &'a Path,
    /// The cloud settings file, whose SSH identity signs the route.
    pub settings: &'a Path,
    /// The placement the cloud was created with, which its settings resolve against.
    pub placement: &'a crate::cloud_panel::Placement,
}

/// What the person's agent asks one worker to run.
#[derive(Clone, Debug)]
pub struct Command {
    pub text: String,
    pub timeout: Duration,
}

/// What one worker command answered. A non-zero `exit_status` is a result, not a failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub exit_status: i32,
    pub uid: u32,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

/// Why Horizon ran nothing, or why the worker's answer is not a result.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("cloud_exec_invalid_request: {0}")]
    Invalid(&'static str),
    #[error("cloud_exec_refused: user switch")]
    UserSwitch,
    #[error("cloud_exec_refused: no agent isolation")]
    NoIsolation,
    #[error("cloud_exec_refused: not root")]
    NotRoot,
    #[error("cloud_exec_refused: uid {0}")]
    Uid(u32),
    #[error("cloud_exec_not_ready: {0}")]
    NotReady(String),
    #[error("cloud_exec_timed_out")]
    TimedOut,
    #[error("cloud_exec_failed: worker contract")]
    Contract,
    #[error("cloud_exec_failed: transport")]
    Transport,
    #[error("cloud_exec_failed: output exceeded its bound")]
    OutputBound,
}

/// # Errors
/// Refuses a command that cannot run as the agent user, and reports a cloud whose route,
/// settings or worker Horizon cannot prove.
pub fn execute(target: &Target<'_>, command: &Command, cancel: &Cancellation) -> Result<Answer, Failure> {
    check(command)?;
    let settings =
        Settings::for_cloud(target.settings, target.placement).map_err(|error| Failure::NotReady(error.to_string()))?;
    let connection = lifecycle::endpoint(target.directory, &settings, cancel)
        .map_err(|error| Failure::NotReady(error.to_string()))?;
    run(&connection, command, cancel)
}

/// Runs one command through the recorded route. The caller has checked the command text.
///
/// # Errors
/// Reports a launcher refusal, a timeout, a broken transport and an answer without the
/// markers that prove the account and the exit status.
pub fn run(connection: &Connection, command: &Command, cancel: &Cancellation) -> Result<Answer, Failure> {
    execute_ssh(
        connection.pinned_command(&remote(&command.text)),
        command.timeout,
        cancel,
    )
}

/// Runs the prepared route under its bound and turns what came back into a result or a
/// typed failure. A test supplies a route that only runs the remote text.
pub(super) fn execute_ssh(
    mut ssh: std::process::Command,
    timeout: Duration,
    cancel: &Cancellation,
) -> Result<Answer, Failure> {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let started = Instant::now();
    let captured = runner
        .run_captured("Cloud worker command", &mut ssh, MAX_ANSWER_BYTES, timeout)
        .map_err(|error| match error {
            Error::Invalid(super::command::TIMED_OUT) => Failure::TimedOut,
            Error::Invalid(super::command::OUTPUT_BOUND) => Failure::OutputBound,
            _ => Failure::Transport,
        })?;
    answer_from(captured, started.elapsed())
}

/// Refuses the command texts Horizon will not run, before any route is resolved.
///
/// # Errors
/// Reports an empty, oversized or NUL-carrying text, and a direct user switch.
pub fn check(command: &Command) -> Result<(), Failure> {
    if command.text.trim().is_empty() {
        return Err(Failure::Invalid("the command is empty"));
    }
    if command.text.len() > MAX_COMMAND_BYTES {
        return Err(Failure::Invalid("the command is longer than 8192 bytes"));
    }
    if command.text.contains('\0') {
        return Err(Failure::Invalid("the command contains a NUL byte"));
    }
    if !(MIN_TIMEOUT_MILLIS..=MAX_TIMEOUT_MILLIS)
        .contains(&u64::try_from(command.timeout.as_millis()).unwrap_or(u64::MAX))
    {
        return Err(Failure::Invalid("the timeout is outside 1000 to 120000 ms"));
    }
    if heads(&command.text).iter().any(|head| USER_SWITCH.contains(head)) {
        return Err(Failure::UserSwitch);
    }
    Ok(())
}

/// The programs each simple command in `text` starts, as the bare names a shell resolves. A
/// word after another word is an argument, so `git commit -m 'try sudo'` names none, while
/// `sudo -i`, `; su -` and `/usr/bin/setpriv` do. An environment assignment passes the
/// command on to the next word, as a shell does.
fn heads(text: &str) -> Vec<&str> {
    let mut heads = Vec::new();
    let mut word: Option<(usize, bool)> = None;
    let mut expect_head = true;
    for (index, character) in text.char_indices() {
        let separator = matches!(character, ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`');
        if character.is_whitespace() || separator {
            if let Some((from, head)) = word.take()
                && head
            {
                expect_head = record(text[from..index].trim(), &mut heads);
            }
            if separator {
                expect_head = true;
            }
        } else if word.is_none() {
            word = Some((index, expect_head));
            expect_head = false;
        }
    }
    if let Some((from, true)) = word {
        record(text[from..].trim(), &mut heads);
    }
    // A path or an escape names the same program as its bare name does.
    heads
        .into_iter()
        .map(|head| {
            head.trim_start_matches(['\\', '"', '\''])
                .rsplit('/')
                .next()
                .unwrap_or(head)
        })
        .collect()
}

/// Records one command head and answers whether the next word starts the command.
fn record<'a>(word: &'a str, heads: &mut Vec<&'a str>) -> bool {
    if let Some((name, _)) = word.split_once('=')
        && !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return true;
    }
    heads.push(word);
    false
}

/// The remote text one `ssh` argument carries. Root's login shell parses only Horizon's own
/// words here, and the caller's text arrives as a single quoted word.
fn remote(command: &str) -> String {
    format!(
        "if [ \"$(id -u)\" != 0 ]; then printf '%s\\n' 'HORIZON-EXEC refused=not-root' >&2; exit {REFUSED_NOT_ROOT}; fi\n\
         if [ ! -f {isolation} ]; then printf '%s\\n' 'HORIZON-EXEC refused=no-isolation' >&2; exit {REFUSED_NO_ISOLATION}; fi\n\
         if [ ! -x {launcher} ]; then printf '%s\\n' 'HORIZON-EXEC refused=no-launcher' >&2; exit {REFUSED_NO_LAUNCHER}; fi\n\
         exec {launcher} agent /bin/sh -c {wrapper}\n",
        isolation = quote(ISOLATION_MARKER),
        launcher = quote(LAUNCHER),
        wrapper = quote(&wrapper(command)),
    )
}

/// The shell text the dropped process parses: the account frame, printed before any caller
/// byte, and then the caller's own command, which replaces this shell. Nothing of Horizon's
/// follows it, so `ssh`'s own exit code is the command's status and no later writer can
/// change it.
fn wrapper(command: &str) -> String {
    format!(
        "printf '{UID_MARKER}%s\\n' \"$(id -u)\"\nexec /bin/sh -c {command}\n",
        command = quote(command),
    )
}

fn answer_from(captured: super::command::Captured, duration: Duration) -> Result<Answer, Failure> {
    let lines: Vec<&str> = captured.stdout.split_inclusive('\n').collect();
    let Some((uid_line, uid)) = uid_frame(&lines) else {
        // No frame, so the exit code is the worker's own refusal or the transport's, and
        // never a command's. Nothing ran as the agent user.
        return Err(match captured.exit_status {
            Some(REFUSED_NO_ISOLATION) => Failure::NoIsolation,
            Some(REFUSED_NOT_ROOT) => Failure::NotRoot,
            Some(SSH_FAILURE) | None => Failure::Transport,
            Some(_) => Failure::Contract,
        });
    };
    if uid != AGENT_UID {
        return Err(Failure::Uid(uid));
    }
    // The frame arrived, so `ssh` reports the status of the command the wrapper became,
    // whatever the command or a process it left behind writes to its streams.
    let exit_status = captured.exit_status.ok_or(Failure::Transport)?;
    let stdout = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != uid_line)
        .map(|(_, line)| *line)
        .collect();
    Ok(Answer {
        exit_status,
        uid,
        stdout,
        stderr: captured.stderr,
        duration,
    })
}

/// The first line that carries the account frame and a value, with that line's index. The
/// wrapper prints it before any caller byte, so a later line can only be caller output.
fn uid_frame(lines: &[&str]) -> Option<(usize, u32)> {
    lines
        .iter()
        .enumerate()
        .find_map(|(index, line)| Some((index, line.trim_end().strip_prefix(UID_MARKER)?.parse().ok()?)))
}

#[cfg(all(test, unix))]
mod tests;
