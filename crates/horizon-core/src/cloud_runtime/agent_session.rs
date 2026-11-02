//! One instruction typed into a cloud worker's agent terminal, and the bounded tail of what
//! that terminal shows.
//!
//! A cloud's agent runs in a tmux session on the worker's socket, named after its panel. The
//! session belongs to the worker's agent user, so Horizon reaches it through the same pinned
//! route and the same launcher that `cloud_exec` uses, and every tmux call runs as uid 10001.
//! Nothing here reads a file or a credential: an instruction goes into the terminal, and a
//! read returns what that terminal shows.
use super::Cancellation;
use super::cloud_exec::{self, Answer, Command, Failure as ExecFailure, Target};
use super::state::{Deployment, Session};
use std::time::Duration;

/// The tmux socket that the worker's sessions live on, as `horizon-worker-session` names it.
pub const TMUX_SOCKET: &str = "horizon-cloud";
/// The lines a write answers with, so the caller sees the session take the instruction.
pub const CONFIRM_LINES: u32 = 40;
/// How long the terminal has to react to the Enter before the answer shows it. An agent's
/// user interface redraws after the key, and the answer must not show the text still waiting.
const SETTLE: Duration = Duration::from_secs(1);
/// The exit code of a session the worker does not run, and of a write that tmux refused.
const NO_SESSION: i32 = 4;
pub(super) const WRITE_FAILED: i32 = 5;
/// The frame the remote text prints when the session is absent. It is written before tmux is
/// asked to type anything, so a caller can tell an absent session from a refused write.
pub(super) const NO_SESSION_MARKER: &str = "HORIZON-AGENT no-session";

/// Why Horizon typed nothing, or why the worker's answer is not a result.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error(
        "cloud_agent_unknown_session: this cloud's record has no session with this panel ID; read cloud_list for its sessions"
    )]
    UnknownSession,
    #[error(
        "cloud_agent_not_running: the worker does not run this session's terminal; open the session on the cloud's card and try again"
    )]
    NotRunning,
    #[error("cloud_agent_write_failed: the worker's terminal did not accept the instruction")]
    Write,
    #[error("{0}")]
    Exec(ExecFailure),
}

impl From<ExecFailure> for Failure {
    fn from(failure: ExecFailure) -> Self {
        Self::Exec(failure)
    }
}

/// The session of `record` that the worker runs, found by the panel ID that names it.
#[must_use]
pub fn session<'a>(record: &'a Deployment, panel: &str) -> Option<&'a Session> {
    record
        .sessions
        .iter()
        .find(|session| session.panel_id == panel && valid_session_id(&session.tmux))
}

/// A session identifier as the worker accepts it: the panel ID of a cloud session.
#[must_use]
pub fn valid_session_id(value: &str) -> bool {
    horizon_cloud::valid_id(value)
}

/// What one call answered: the terminal's text under the byte bound, and the account and
/// status that prove the worker ran the call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The terminal's text, cut at its start when the bound applied.
    pub text: String,
    /// Whether the byte bound cut the start of the terminal's text.
    pub truncated: bool,
    pub exit_status: i32,
    pub uid: u32,
    pub duration: Duration,
}

/// Types `text` into `session`'s terminal and answers what the terminal shows a moment later,
/// so one call shows the instruction arriving.
///
/// # Errors
/// [`Failure::Exec`] when the route or the launcher fails, [`Failure::NotRunning`] when the
/// worker runs no such session, and [`Failure::Write`] when tmux refused the write.
pub fn message(target: &Target<'_>, session: &Session, text: &str, cancel: &Cancellation) -> Result<Outcome, Failure> {
    let command = Command {
        text: message_text(&session.tmux, text),
        timeout: Duration::from_millis(horizon_browser_control::manifest::cloud_agent::TIMEOUT_MILLIS),
    };
    outcome(target, &command, cancel)
}

/// Answers the last `lines` lines that `session`'s terminal shows.
///
/// # Errors
/// As [`message`], without [`Failure::Write`].
pub fn read(target: &Target<'_>, session: &Session, lines: u32, cancel: &Cancellation) -> Result<Outcome, Failure> {
    let command = Command {
        text: read_text(&session.tmux, lines),
        timeout: Duration::from_millis(horizon_browser_control::manifest::cloud_agent::TIMEOUT_MILLIS),
    };
    outcome(target, &command, cancel)
}

/// Runs one prepared tmux text and turns the worker's answer into a result.
///
/// # Errors
/// As [`message`].
pub fn outcome(target: &Target<'_>, command: &Command, cancel: &Cancellation) -> Result<Outcome, Failure> {
    interpret(cloud_exec::execute(target, command, cancel)?)
}

/// Reads the worker's answer: an absent session and a refused write have their own codes, and
/// the terminal's text is bounded at its start.
fn interpret(answer: Answer) -> Result<Outcome, Failure> {
    let Answer {
        exit_status,
        uid,
        stdout,
        stderr,
        duration,
    } = answer;
    match exit_status {
        NO_SESSION if stderr.contains(NO_SESSION_MARKER) => return Err(Failure::NotRunning),
        NO_SESSION | WRITE_FAILED => return Err(Failure::Write),
        _ => {}
    }
    let (text, truncated) = tail(&stdout, horizon_browser_control::manifest::cloud_agent::MAX_READ_BYTES);
    Ok(Outcome {
        text,
        truncated,
        exit_status,
        uid,
        duration,
    })
}

/// The remote text that types `text` into the session named `tmux` and presses Enter, then
/// prints what the terminal shows. The instruction reaches tmux through a buffer, never
/// through argv: tmux would read a word that starts with `-` as one of its own flags.
#[must_use]
pub fn message_text(tmux: &str, text: &str) -> String {
    let target = target(tmux);
    let socket = cloud_exec::quote(TMUX_SOCKET);
    format!(
        "{guard}\
         printf '%s' {text} | tmux -L {socket} load-buffer - \
         && tmux -L {socket} paste-buffer -d -t {target} \
         && tmux -L {socket} send-keys -t {target} Enter \
         || exit {WRITE_FAILED}\n\
         sleep {settle}\n\
         tmux -L {socket} capture-pane -p -J -t {target} -S -{CONFIRM_LINES}\n",
        guard = guard(tmux),
        text = cloud_exec::quote(text),
        settle = SETTLE.as_secs(),
    )
}

/// The remote text that prints the last `lines` lines of the session named `tmux`.
#[must_use]
pub fn read_text(tmux: &str, lines: u32) -> String {
    format!(
        "{guard}tmux -L {socket} capture-pane -p -J -t {target} -S -{lines}\n",
        guard = guard(tmux),
        socket = cloud_exec::quote(TMUX_SOCKET),
        target = target(tmux),
    )
}

/// The refuses-first part of every tmux text: a session the worker does not run is named as
/// such, so no tmux call reports its absence as a write that failed.
fn guard(tmux: &str) -> String {
    format!(
        "if ! tmux -L {socket} has-session -t {target} 2>/dev/null; then \
         printf '%s\\n' {marker} >&2; exit {NO_SESSION}; fi\n",
        socket = cloud_exec::quote(TMUX_SOCKET),
        target = target(tmux),
        marker = cloud_exec::quote(NO_SESSION_MARKER),
    )
}

/// The tmux target of one session. The `=` prefix makes tmux read the name exactly, so a
/// session whose ID is the beginning of another one's is never mistaken for it.
fn target(tmux: &str) -> String {
    cloud_exec::quote(&format!("={tmux}"))
}

/// The last `max_bytes` bytes of `text`, starting on a line, and whether the start was cut.
/// The newest output is what a caller needs, so the cut is at the start and never at the end.
#[must_use]
pub fn tail(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    // A line that the cut split is dropped, so the answer begins where the terminal did. A
    // bound that already falls on a line start keeps its line.
    let start = if text[..start].ends_with('\n') {
        start
    } else {
        text[start..]
            .find('\n')
            .map_or(start, |offset| start + offset + 1)
            .min(text.len())
    };
    (text[start..].to_owned(), true)
}

#[cfg(all(test, unix))]
mod tests;
