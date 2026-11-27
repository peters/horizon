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
//!
//! [`claude_for_workspace`] is the program of the orchestrator dock: it speaks
//! for one workspace with no panel, so it gets the actor of that workspace, the
//! identity variables of an agent panel and the plugin directory of Claude Code,
//! and no work-resume plugin.

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

/// What a program that speaks for one Horizon agent needs besides its command
/// line: the actor that names it to the host, the host instance of the running
/// Horizon, the home that holds the request queues, the program that runs this
/// Horizon, and the plugin directory of Claude Code, which holds the Horizon MCP
/// server, its tools and its skills.
///
/// An agent panel gets the same values, so a program of a bridge reads one
/// identity and reaches the same host. The home stays a value of this identity
/// and is not written into the environment of the child: the child inherits
/// `HORIZON_HOME` from this Horizon exactly as a panel does, so the tools of the
/// orchestrator resolve the folder that the host reads, and a home that this
/// process merely resolved keeps the rule that a panel's home keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeIdentity {
    pub actor: String,
    pub host_instance: String,
    pub home: PathBuf,
    pub executable: PathBuf,
    /// The plugin directory, while it is a directory of this computer. It is
    /// absent when the start of this Horizon could not write it, and the program
    /// then starts with no Horizon tool at all.
    pub plugin_dir: Option<PathBuf>,
}

impl BridgeIdentity {
    /// The identity of the orchestrator of the workspace with `local_id` in this
    /// Horizon, or `None` when that local id is not of the form that an actor of
    /// an orchestrator holds (see [`super::actor`]).
    ///
    /// The three values of this Horizon are read once, here, and the plugin
    /// directory is checked once: a start is not a per-frame path.
    #[must_use]
    pub fn for_workspace(local_id: &str) -> Option<Self> {
        let actor = super::actor::of_workspace(local_id)?;
        let host_instance = crate::browser::manifest::host_instance().to_string();
        let home = crate::HorizonHome::resolve();
        let plugin_dir = home.claude_plugin_dir_for_host(&host_instance);
        Some(Self {
            actor,
            host_instance,
            home: home.root().to_path_buf(),
            executable: crate::browser_mcp_executable().unwrap_or_default(),
            plugin_dir: plugin_dir.is_dir().then_some(plugin_dir),
        })
    }
}

/// The program of the orchestrator of one workspace, with no terminal.
///
/// It is [`claude`] plus the identity of that workspace: the `--plugin-dir` that
/// an agent panel of Claude Code gets, so the program has the Horizon MCP server
/// and the Horizon skills, and the environment that names it to that server. It
/// adds no work-resume plugin, no `HORIZON_WORK_*` variable and no named secret,
/// because the orchestrator of this part has no turn ledger and no panel of the
/// board.
#[must_use]
pub fn claude_for_workspace(cwd: &Path, session: SessionChoice, identity: &BridgeIdentity) -> ProgramSpec {
    let mut spec = claude(cwd, session);
    if let Some(plugin_dir) = &identity.plugin_dir {
        spec.args.push(OsString::from("--plugin-dir"));
        spec.args.push(plugin_dir.clone().into_os_string());
    }
    let executable = identity
        .executable
        .clone()
        .into_os_string()
        .into_string()
        .unwrap_or_default();
    spec.env = crate::panel::spawn::agent_identity_vars(identity.actor.clone(), &identity.host_instance, &executable)
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    spec
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
