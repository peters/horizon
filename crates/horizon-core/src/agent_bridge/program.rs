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
//!
//! [`claude_for_front_desk`] is the program of the front desk: the same command
//! line as an orchestrator, and then three additions that make it a
//! conversation and nothing else. It gets no built-in tool ([`NO_TOOLS`]), the
//! model of the front desk ([`front_desk_model`]) and one system text
//! ([`FRONT_DESK_PROMPT`]) that names its job, so it writes the words of the
//! person to the work items and never does the work itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The first parts of the names of every variable that a nested agent program
/// must not inherit.
pub const ENV_REMOVE_PREFIXES: [&str; 2] = ["CLAUDE_CODE_", "HORIZON_WORK_"];

/// The names of the variables that a nested Claude Code must not inherit, and
/// that no prefix above matches.
///
/// The last four are the identity that
/// [`crate::panel::spawn::agent_identity_vars`] gives to an agent panel: a
/// Horizon that an agent panel started holds them, so a start without an
/// identity of its own must not pass them on. The program of a workspace sets
/// its own values after this removal.
pub const ENV_REMOVE_NAMES: [&str; 5] = [
    "CLAUDECODE",
    "HORIZON",
    "HORIZON_BROWSER_ACTOR",
    crate::browser::manifest::HOST_INSTANCE_ENV,
    "HORIZON_EXECUTABLE",
];

/// The flags of the line format of the codec of Claude Code.
const CLAUDE_ARGS: [&str; 5] = ["-p", "--input-format", "stream-json", "--output-format", "stream-json"];

/// The value of `--tools` that leaves the front desk no built-in tool. The help
/// of Claude Code names the empty word as the form for none.
pub const NO_TOOLS: &str = "";

/// The name of the variable that gives the front desk another model than
/// [`FRONT_DESK_MODEL`] of this line of Horizon. A value that is not of the form
/// [`MODEL_BYTES`] is ignored.
pub const FRONT_DESK_MODEL_ENV: &str = "HORIZON_FRONT_DESK_MODEL";

/// The model of the front desk of a start that names none.
pub const FRONT_DESK_MODEL: &str = "haiku";

/// The greatest length of a model name that Horizon passes as one argument. A
/// longer value is not a model of Claude Code.
const MODEL_BYTES: usize = 64;

/// The text that the front desk gets after the system text of Claude Code: what
/// the conversation is for, and what it cannot do in this part.
pub const FRONT_DESK_PROMPT: &str = "You are the front desk of Horizon. The person writes one message that \
holds one or more tasks, and you turn it into work items. You have no built-in tool and you do not do the \
work yourself. Your tool block_panel has three operations: work_item_propose offers the work items that \
you read in the words of the person, and nothing starts before the person presses a control that Horizon \
draws; work_items reads their state; work_item_message passes one message of the person to one work item. \
Each task then has its own orchestrator. Answer in a few short lines: say which tasks you read, and ask \
one question only when a task names no outcome. Never answer a question of a work item for the person, \
and never claim that work is done.";

/// The text that the orchestrator of a workspace gets after the system text of
/// Claude Code: what its conversation is, and what a worker is. A run with the
/// real program showed why the second part is there: asked for a worker, the
/// program started a subagent of its own, because nothing told it that a worker
/// is a panel that Horizon starts.
pub const ORCHESTRATOR_PROMPT: &str = "You are the orchestrator of one work item of Horizon. The person reads your conversation in the dock of the Horizon window, beside the panels of this workspace. Show a plan, a choice or a result as a card with your tool block_panel, as the skill horizon-block says. A worker is another agent program that Horizon starts in a panel of this workspace for you. When the person asks for a worker, or names another agent program for a part of the work, call block_panel with the operation worker_start, the name of that program and one task. Do not use a subagent tool of your own in place of a worker. The report of a worker comes to you as a turn, the operation workers reads the workers that you have, and worker_message sends one message to one of them. Ask the person on a card before you rent a machine or spend money.";

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
        Some(Self::in_this_horizon(actor))
    }

    /// The identity of the front desk of this Horizon, which owns no workspace.
    #[must_use]
    pub fn front_desk() -> Self {
        Self::in_this_horizon(super::actor::FRONT_DESK.to_owned())
    }

    /// The identity of one actor of this Horizon: the host instance, the home
    /// that holds the request queues, the program that runs this Horizon and the
    /// plugin directory of Claude Code.
    fn in_this_horizon(actor: String) -> Self {
        let host_instance = crate::browser::manifest::host_instance().to_string();
        let home = crate::HorizonHome::resolve();
        let plugin_dir = home.claude_plugin_dir_for_host(&host_instance);
        Self {
            actor,
            host_instance,
            home: home.root().to_path_buf(),
            executable: crate::browser_mcp_executable().unwrap_or_default(),
            plugin_dir: plugin_dir.is_dir().then_some(plugin_dir),
        }
    }
}

/// The program of the orchestrator of one workspace, with no terminal.
///
/// It is [`claude`] plus the identity of that workspace: the `--plugin-dir` that
/// an agent panel of Claude Code gets, so the program has the Horizon MCP server
/// and the Horizon skills, and the environment that names it to that server. It
/// adds no work-resume plugin, no `HORIZON_WORK_*` variable and no named secret,
/// because the orchestrator of this part has no turn ledger and no panel of the
/// board. Its last addition is [`ORCHESTRATOR_PROMPT`], a constant text of this
/// Horizon.
#[must_use]
pub fn claude_for_workspace(cwd: &Path, session: SessionChoice, identity: &BridgeIdentity) -> ProgramSpec {
    let mut spec = claude_with_identity(cwd, session, identity);
    spec.args.push(OsString::from("--append-system-prompt"));
    spec.args.push(OsString::from(ORCHESTRATOR_PROMPT));
    spec
}

/// [`claude`] with the identity of one actor of this Horizon: the plugin
/// directory and the environment that names the actor to the Horizon MCP server.
fn claude_with_identity(cwd: &Path, session: SessionChoice, identity: &BridgeIdentity) -> ProgramSpec {
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

/// The program of the front desk: the conversation that belongs to no workspace.
///
/// It is the command line of [`claude_for_workspace`] without the prompt of an
/// orchestrator, and then three additions: [`NO_TOOLS`], the model of
/// [`front_desk_model`] and [`FRONT_DESK_PROMPT`]. The front desk does no work itself, so it has no tool
/// to do it with. The person of that conversation is the person of this computer
/// and the model comes from this Horizon, so neither value can arrive from the
/// output of a program.
#[must_use]
pub fn claude_for_front_desk(cwd: &Path, session: SessionChoice, identity: &BridgeIdentity) -> ProgramSpec {
    let mut spec = claude_with_identity(cwd, session, identity);
    spec.args.push(OsString::from("--tools"));
    spec.args.push(OsString::from(NO_TOOLS));
    spec.args.push(OsString::from("--model"));
    spec.args.push(OsString::from(front_desk_model(
        std::env::var(FRONT_DESK_MODEL_ENV).ok().as_deref(),
    )));
    spec.args.push(OsString::from("--append-system-prompt"));
    spec.args.push(OsString::from(FRONT_DESK_PROMPT));
    spec
}

/// The model of the front desk: the value of [`FRONT_DESK_MODEL_ENV`] while it
/// is a name of a model, and [`FRONT_DESK_MODEL`] for every other value.
///
/// A name holds [`MODEL_BYTES`] bytes or less of the letters, digits, dot, hyphen
/// and underscore, so it is one argument of the program and no word of a shell.
/// A name does not start with a hyphen, so the value of the variable can never be
/// read as another option of the program.
#[must_use]
pub fn front_desk_model(value: Option<&str>) -> String {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MODEL_BYTES
                && !value.starts_with('-')
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
        .unwrap_or(FRONT_DESK_MODEL)
        .to_owned()
}

/// The program of one Claude Code conversation, with no terminal.
///
/// This function checks the session choice, because its id becomes one argument
/// of the program. An id that is not of the form of a UUID (see
/// [`super::actor::is_uuid`]) is read as another option of the program, or it is
/// above the length of one argument. Such a start is a new session with a fresh
/// id and the option `--session-id` instead. A [`SessionChoice::Resume`] of that
/// form continues its session with `--resume`.
#[must_use]
pub fn claude(cwd: &Path, session: SessionChoice) -> ProgramSpec {
    let mut args: Vec<OsString> = CLAUDE_ARGS.iter().map(OsString::from).collect();
    args.push(OsString::from("--verbose"));
    let (flag, session_id) = match session {
        SessionChoice::New(session_id) if super::actor::is_uuid(&session_id) => ("--session-id", session_id),
        SessionChoice::Resume(session_id) if super::actor::is_uuid(&session_id) => ("--resume", session_id),
        SessionChoice::New(_) | SessionChoice::Resume(_) => ("--session-id", fresh_session()),
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

/// The id of the session of a start that does not continue the session that it
/// was asked for.
fn fresh_session() -> String {
    crate::new_local_id()
}
