//! The program that one bridge runs, and the variables that it must not
//! inherit.
//!
//! [`claude`] is the only program of this slice. Its command line holds the
//! flags of the line format and the session choice, and never the prompt: a
//! prompt travels on the standard input only, so it reaches no process list and
//! no log of the system.
//!
//! A program that Horizon starts inside another agent must not inherit the
//! identity of that agent: every name below is removed from the environment of
//! the child before it starts, and [`apply_environment`] does that.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The first parts of the names of every variable that a nested agent program
/// must not inherit.
pub const ENV_REMOVE_PREFIXES: [&str; 2] = ["CLAUDE_CODE_", "HORIZON_WORK_"];

/// The names of the variables that a nested Claude Code must not inherit, and
/// that no prefix above matches.
pub const ENV_REMOVE_NAMES: [&str; 1] = ["CLAUDECODE"];

/// The flags of the line format of the codec of Claude Code.
const CLAUDE_ARGS: [&str; 5] = ["-p", "--input-format", "stream-json", "--output-format", "stream-json"];

/// One program that Horizon starts, and nothing of it comes from the program's
/// own output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
    pub cwd: PathBuf,
}

/// Puts the environment of one program on one command: the names of
/// `env_remove` are removed first, then every inherited name with a prefix
/// above, then the values of `env` are set.
pub(super) fn apply_environment(env: &[(OsString, OsString)], env_remove: &[OsString], command: &mut Command) {
    for name in env_remove {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if ENV_REMOVE_PREFIXES
            .iter()
            .any(|prefix| name.as_encoded_bytes().starts_with(prefix.as_bytes()))
        {
            command.env_remove(&name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
}

/// The session of a conversation: a new one, or the one of an earlier turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionChoice {
    /// A session with the id that Horizon chose, as a UUID.
    New(String),
    /// The session of an earlier conversation.
    Resume(String),
}

/// The program of one Claude Code conversation, with no terminal.
#[must_use]
pub fn claude(cwd: &Path, session: SessionChoice) -> ProgramSpec {
    let mut args: Vec<OsString> = CLAUDE_ARGS.iter().map(OsString::from).collect();
    args.push(OsString::from("--verbose"));
    let (flag, session_id) = match session {
        SessionChoice::New(session_id) => ("--session-id", session_id),
        SessionChoice::Resume(session_id) => ("--resume", session_id),
    };
    args.push(OsString::from(flag));
    args.push(OsString::from(session_id));
    ProgramSpec {
        program: PathBuf::from("claude"),
        args,
        env: Vec::new(),
        env_remove: ENV_REMOVE_NAMES.iter().map(OsString::from).collect(),
        cwd: cwd.to_path_buf(),
    }
}
