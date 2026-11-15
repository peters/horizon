//! The MCP entry of a card relay, in the shape of each agent program.
//!
//! The setup prefers the writer of the agent program itself, so Horizon merges no
//! file and writes no backup: `claude mcp add --scope user`, `codex mcp add` and
//! `pi mcp add` own the file of that program. Without the program the setup merges
//! one key of the user-scope file ([`entry_json`], [`entry_toml`]), keeps one
//! backup, and its `Remove` direction deletes that key again.

use serde_json::{Map, Value, json};

use super::ENTRY_NAME;

/// One agent program whose MCP entry Horizon can write on a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAgent {
    Claude,
    Codex,
    Pi,
}

impl CardAgent {
    /// The program that writes its own entry, which the setup tries first.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    /// The user-scope file that a merge writes when the program is absent.
    /// `~` is the home of the user of the machine.
    #[must_use]
    pub const fn merge_file(self) -> &'static str {
        match self {
            Self::Claude => "~/.claude.json",
            Self::Codex => "~/.codex/config.toml",
            Self::Pi => "~/.pi/agent/mcp.json",
        }
    }

    /// The words that the program's own writer takes, after its name.
    #[must_use]
    pub fn writer_args(self, command: &str, args: &[String]) -> Vec<String> {
        let mut words = match self {
            Self::Claude => vec!["mcp", "add", "--scope", "user", ENTRY_NAME, "--", command],
            Self::Codex | Self::Pi => vec!["mcp", "add", ENTRY_NAME, "--", command],
        }
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        words.extend(args.iter().cloned());
        words
    }
}

/// The name of the file that a merge writes for `agent`, in the words above.
#[must_use]
pub fn entry_file(agent: CardAgent) -> &'static str {
    agent.merge_file()
}

/// The one entry as a JSON object, which is the value of the key
/// `mcpServers.horizon-cards` of a merge of Claude Code or of pi.
#[must_use]
pub fn entry_json(command: &str, args: &[String]) -> Value {
    let mut entry = Map::new();
    entry.insert("command".to_owned(), json!(command));
    entry.insert("args".to_owned(), json!(args));
    Value::Object(entry)
}

/// The one entry as the table `[mcp_servers.horizon-cards]` of a Codex
/// configuration file. Both values are JSON strings here, which are also TOML
/// basic strings, so the text merges as it stands.
#[must_use]
pub fn entry_toml(command: &str, args: &[String]) -> String {
    format!(
        "[mcp_servers.{ENTRY_NAME}]\ncommand = {}\nargs = {}\n",
        json!(command),
        json!(args)
    )
}
