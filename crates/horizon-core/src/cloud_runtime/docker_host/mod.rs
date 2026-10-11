//! Read-only admission checks for existing Docker engines.
mod probe;
mod tailscale;
#[cfg(test)]
mod tests;
mod transport;

use super::{Error, Result};
pub use probe::{Check, CheckState, Probe, probe};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub(crate) use transport::Transport;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    pub name: String,
    /// None selects the current computer's Docker engine.
    pub ssh: Option<Ssh>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub allow_emulation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ssh {
    pub host: String,
    pub user: String,
    pub port: u16,
    #[serde(default)]
    pub authentication: SshAuthentication,
    #[serde(default)]
    pub identity_file: PathBuf,
    #[serde(default)]
    pub known_hosts: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SshAuthentication {
    #[default]
    Key,
    Tailscale,
}

impl Binding {
    /// # Errors
    /// Rejects shell syntax, ambiguous identities and non-local credential paths.
    pub fn validate(&self) -> Result<()> {
        if !horizon_cloud::valid_id(&self.id) || self.name.trim().is_empty() || self.name.len() > 128 {
            return Err(Error::Invalid("Give the Docker host a stable ID and a name"));
        }
        if self.context.as_deref().is_some_and(|name| !valid_context_name(name)) {
            return Err(Error::Invalid("Invalid Docker context"));
        }
        if let Some(ssh) = &self.ssh {
            ssh.host.parse::<horizon_cloud::SshHost>().map_err(Error::Invalid)?;
            if ssh.port == 0
                || ssh.user.is_empty()
                || ssh.user.len() > 64
                || ssh.user.starts_with('-')
                || !ssh
                    .user
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            {
                return Err(Error::Invalid("Docker hosts need a valid SSH user and port"));
            }
            match ssh.authentication {
                SshAuthentication::Key if !ssh.identity_file.is_absolute() || !ssh.known_hosts.is_absolute() => {
                    return Err(Error::Invalid(
                        "Key-based SSH needs absolute private-key and trusted-host-key paths",
                    ));
                }
                SshAuthentication::Tailscale if ssh.port != 22 => {
                    return Err(Error::Invalid("Tailscale SSH uses port 22"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn valid_context_name(name: &str) -> bool {
    name.len() >= 2
        && name.bytes().next().is_some_and(|byte| byte.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}
