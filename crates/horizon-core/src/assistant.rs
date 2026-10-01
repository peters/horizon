//! Settings for the assistant drawer: which agent answers and how it signs in.
//!
//! The assistant is an ordinary agent panel marked by a well-known local id.
//! Its choices live in a small private store under the Horizon home, next to
//! the optional API key file, so the drawer can change them without rewriting
//! the user's YAML config.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, HorizonHome, PanelKind, Result};

/// Local id of the one panel the drawer hosts.
pub const ASSISTANT_PANEL_LOCAL_ID: &str = "horizon-assistant";

/// Agents the drawer can host. Every one is a terminal agent, so the drawer
/// shows its own interface; only some can call Horizon's MCP tools.
pub const ASSISTANT_AGENTS: [PanelKind; 5] = [
    PanelKind::Claude,
    PanelKind::Codex,
    PanelKind::Gemini,
    PanelKind::OpenCode,
    PanelKind::Grok,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantAuth {
    /// Use whatever the CLI is already signed in to.
    #[default]
    Subscription,
    /// Pass an API key from the private key file as the CLI's environment variable.
    ApiKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantSettings {
    pub agent: PanelKind,
    #[serde(default)]
    pub auth: AssistantAuth,
    /// Ask the person before the assistant types into another agent.
    #[serde(default = "ask_before_send_by_default")]
    pub ask_before_send: bool,
}

const fn ask_before_send_by_default() -> bool {
    true
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            agent: PanelKind::Claude,
            auth: AssistantAuth::Subscription,
            ask_before_send: true,
        }
    }
}

/// The environment variable and key file name an agent uses for API-key mode.
/// Agents without an entry sign in only through their own CLI.
#[must_use]
pub const fn api_key_binding(kind: PanelKind) -> Option<(&'static str, &'static str)> {
    match kind {
        PanelKind::Claude => Some(("ANTHROPIC_API_KEY", "anthropic-api-key")),
        PanelKind::Codex => Some(("OPENAI_API_KEY", "openai-api-key")),
        _ => None,
    }
}

impl AssistantSettings {
    /// Whether another set of settings needs the same agent process: the agent
    /// and how it signs in, but not the approval preference.
    #[must_use]
    pub fn same_engine(&self, other: &Self) -> bool {
        self.agent == other.agent && self.auth == other.auth
    }

    /// Reads the stored choice, falling back to the default when absent or unreadable.
    #[must_use]
    pub fn load(home: &HorizonHome) -> Self {
        std::fs::read_to_string(settings_path(home))
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .filter(|settings| ASSISTANT_AGENTS.contains(&settings.agent))
            .unwrap_or_default()
    }

    /// Persists the choice.
    ///
    /// # Errors
    /// Returns an I/O error when the directory or file cannot be written.
    pub fn save(&self, home: &HorizonHome) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|error| Error::State(error.to_string()))?;
        write_private(&settings_path(home), text.as_bytes())
    }

    /// Whether the agent can start with the chosen sign-in, and why not.
    ///
    /// # Errors
    /// Returns a user-facing reason when API-key mode lacks a supported agent or a saved key.
    pub fn launch_readiness(&self, home: &HorizonHome) -> std::result::Result<(), String> {
        if self.auth == AssistantAuth::Subscription {
            return Ok(());
        }
        let Some((_, file)) = api_key_binding(self.agent) else {
            return Err(format!(
                "{} has no API key mode here. Use its own sign-in.",
                crate::agent_definition(self.agent).map_or("This agent", |agent| agent.display_name)
            ));
        };
        if key_path(home, file).is_file() {
            Ok(())
        } else {
            Err("Add an API key to start the assistant.".to_string())
        }
    }

    /// Environment the agent process needs for the chosen sign-in.
    ///
    /// # Errors
    /// Returns an error when API-key mode is selected but the key cannot be read.
    pub fn launch_env(&self, home: &HorizonHome) -> Result<HashMap<String, String>> {
        let mut env = HashMap::new();
        if self.auth != AssistantAuth::ApiKey {
            return Ok(env);
        }
        let Some((variable, file)) = api_key_binding(self.agent) else {
            return Ok(env);
        };
        let key = std::fs::read_to_string(key_path(home, file))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Config("the assistant API key file is empty".to_string()));
        }
        env.insert(variable.to_string(), key.to_string());
        Ok(env)
    }
}

/// Stores the API key for `kind`, replacing any earlier one.
///
/// # Errors
/// Returns an error for an unsupported agent, a blank or multi-line key, or an I/O failure.
pub fn save_api_key(home: &HorizonHome, kind: PanelKind, key: &str) -> Result<()> {
    let Some((_, file)) = api_key_binding(kind) else {
        return Err(Error::Config("this agent does not use an API key".to_string()));
    };
    let key = key.trim();
    if key.is_empty() || key.contains(['\n', '\r']) {
        return Err(Error::Config("enter a nonempty API key on one line".to_string()));
    }
    write_private(&key_path(home, file), key.as_bytes())
}

/// Removes the stored API key for `kind`; a missing key is not an error.
///
/// # Errors
/// Returns an I/O error other than "not found".
pub fn remove_api_key(home: &HorizonHome, kind: PanelKind) -> Result<()> {
    let Some((_, file)) = api_key_binding(kind) else {
        return Ok(());
    };
    match std::fs::remove_file(key_path(home, file)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Whether a key is stored for `kind`.
#[must_use]
pub fn has_api_key(home: &HorizonHome, kind: PanelKind) -> bool {
    api_key_binding(kind).is_some_and(|(_, file)| key_path(home, file).is_file())
}

fn directory(home: &HorizonHome) -> PathBuf {
    home.root().join("assistant")
}

fn settings_path(home: &HorizonHome) -> PathBuf {
    directory(home).join("settings.json")
}

fn key_path(home: &HorizonHome, file: &str) -> PathBuf {
    directory(home).join(file)
}

/// Writes `bytes` so that only the current user can read the file.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict(parent, 0o700)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    temporary.write_all(bytes)?;
    restrict(temporary.path(), 0o600)?;
    temporary.persist(path).map_err(|error| Error::Io(error.error))?;
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
