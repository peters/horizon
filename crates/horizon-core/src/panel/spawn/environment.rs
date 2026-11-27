//! The environment of a panel's process, and the one line that a panel says when
//! the named secrets of a start were not read in time.
//!
//! The values of an agent panel are what a tool of that agent reads to answer as
//! this panel, and the named secrets of this computer take part, so a tool reads
//! its key without an export in the shell of the computer.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::PanelKind;

/// The line that a panel says when the named secrets of this start were not read
/// before its launch. One line, and the panel runs: an agent without its keys is
/// better than a panel that never starts.
pub(crate) const NAMED_SECRETS_LATE_LINE: &str =
    "The named secrets of this computer were not read in time, so this panel has none of them.\r\n";

/// Waits for the read of the named secrets that this start announced, so an agent
/// panel that starts in the first frames of a start still gets the values of the
/// store. Answers `true` when the values stand, and `false` when the bound ended
/// first, which the panel then says in one line.
///
/// The wait runs on the worker thread of a panel start. A plain terminal reads no
/// named secret, so it never waits.
pub fn wait_for_named_secrets(kind: PanelKind) -> bool {
    if !kind.is_agent() {
        return true;
    }
    crate::named_secrets::wait_until_published(crate::named_secrets::PUBLISH_BOUND)
}

/// The bytes that a terminal replays before the output of its process: the saved
/// screen of the panel, and the one line of a launch whose named secrets were not
/// read in time.
pub fn replay_bytes_with_notice(mut replay_bytes: Vec<u8>, named_secrets_late: bool) -> Vec<u8> {
    if named_secrets_late {
        replay_bytes.extend_from_slice(NAMED_SECRETS_LATE_LINE.as_bytes());
    }
    replay_bytes
}

pub fn agent_env(kind: PanelKind, local_id: &str, uses_default_command: bool) -> HashMap<String, String> {
    let mut env = HashMap::new();
    // The process of a panel inherits the environment of this Horizon, so it also
    // inherits the identity of a panel of another board when this Horizon was
    // started from one. An empty value overrides that inherited value and reads as
    // "no panel" to the tools, so a nested launch never speaks as another panel.
    // The values of an agent panel are set below, over these two.
    for key in crate::agent_work::WORK_ENV_KEYS
        .into_iter()
        .chain(PANEL_IDENTITY_ENV_KEYS)
    {
        env.insert(key.to_owned(), String::new());
    }
    if kind.is_agent() {
        let executable = executable_path();
        for (name, value) in agent_identity_vars(
            browser_actor(local_id),
            crate::browser::manifest::host_instance(),
            &executable,
        ) {
            env.insert(name.to_owned(), value);
        }
        // Every named secret whose value this computer holds, as one environment
        // variable each, so a tool that the agent starts reads its key without an
        // export in the shell of this computer. A terminal panel, and a terminal on
        // a machine over ssh, gets none: a value never reaches a command line and
        // never leaves this computer with a login. The names that Horizon or an
        // agent own are refused when a person adds them, so this never replaces a
        // variable that the lines above set.
        crate::named_secrets::apply(&mut env);
    }
    if kind == PanelKind::Grok {
        let command = uses_default_command
            .then(crate::browser_mcp_executable)
            .flatten()
            .and_then(|path| path.into_os_string().into_string().ok())
            .unwrap_or_default();
        env.insert("HORIZON_BROWSER_MCP_EXECUTABLE".to_string(), command);
    }
    if kind == PanelKind::Claude {
        // Keep the conversation in Horizon's terminal history so its scrollbar
        // and history meter remain usable instead of hiding it in a fullscreen buffer.
        //
        // This costs something and overrides the user's own `tui` setting: Claude
        // Code's inline renderer leaves a copy of the old frame in the scrollback
        // after a width change, so drag-resizing a panel duplicates bands of a long
        // answer. Idle panels stay clean. See
        // `docs/architecture/claude-panel-rendering.md` for the measurements and for
        // the two repairs that were tried and discarded.
        env.insert("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".to_string(), "1".to_string());
    }
    env
}

/// The variables that name one Horizon agent to the tools that it starts: the
/// mark of an agent, the actor of this program, the host instance of the running
/// Horizon and the full path of the program that runs it.
///
/// An agent panel and a program that the orchestrator dock runs get the same
/// names from this function, so a tool reads one identity and the two callers
/// cannot drift apart. A caller that is not an agent gets none of them.
#[must_use]
pub fn agent_identity_vars(actor: String, host_instance: &str, executable: &str) -> [(&'static str, String); 4] {
    [
        ("HORIZON", "1".to_string()),
        ("HORIZON_BROWSER_ACTOR", actor),
        (crate::browser::manifest::HOST_INSTANCE_ENV, host_instance.to_string()),
        (EXECUTABLE_ENV, executable.to_string()),
    ]
}

/// The full path of the program that runs this Horizon, as a string, or the
/// empty string when this build cannot resolve its own path.
///
/// A skill copies the binary of this computer instead of looking for it on a
/// `PATH` that holds no `horizon` when the person starts it from a folder.
fn executable_path() -> String {
    crate::browser_mcp_executable()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_default()
}

/// Stable private control identity injected into one Horizon agent panel.
#[must_use]
pub fn browser_actor(local_id: &str) -> String {
    if !local_id.is_empty() && local_id.len() <= 120 && !local_id.chars().any(char::is_control) {
        return format!("horizon:{local_id}");
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    local_id.hash(&mut hasher);
    format!("horizon:{:016x}", hasher.finish())
}

/// The variables that name the panel of a Horizon to the tools that it starts.
///
/// A panel sets the values of its own panel, and a process without one of these
/// variables reads no actor at all. A value of the Horizon that started this one
/// names another panel, or a panel of a board that does not stand any more, so no
/// panel may keep it.
pub(crate) const PANEL_IDENTITY_ENV_KEYS: [&str; 2] =
    ["HORIZON_BROWSER_ACTOR", crate::browser::manifest::HOST_INSTANCE_ENV];

/// The environment variable that holds the full path of the program that runs
/// Horizon, in every agent panel.
///
/// A person who starts Horizon from a folder has no `horizon` on `PATH`, so a
/// skill that copies the binary of this computer to another machine reads this
/// variable and never `command -v horizon`.
///
/// The value is the running program itself, not the path that a newer build may
/// have taken over: on Linux it is `/proc/<pid>/exe`, which follows the live
/// inode after the file at the original path was replaced or removed, and it is
/// the only name that a copy of a replaced build would get wrong. On macOS it is
/// [`std::env::current_exe`], the path that this Horizon started from, and a file
/// that the person deleted is not there for the copy. A build that cannot resolve
/// its own path sets the variable to the empty string, as the Grok panel already
/// handles `HORIZON_BROWSER_MCP_EXECUTABLE`.
pub const EXECUTABLE_ENV: &str = "HORIZON_EXECUTABLE";
