//! An existing Docker engine as a host for workers: this computer's own, or one
//! reached over SSH or Tailscale SSH. Horizon keeps no secret for it: SSH uses the
//! person's own configuration, keys and agent. A worker publishes its SSH only on the
//! host's loopback and is reached through the host, so neither the Docker API nor the
//! worker is exposed to the network.
mod command;
#[cfg(test)]
mod tests;
mod worker;

use crate::{CloudError, SshHost};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};

pub use command::{Docker, Engine, Failure};
pub use worker::{Container, Journal, Records, delete, describe, find, power, provision};

/// The profile's `provider` value.
pub const PROVIDER: &str = "docker-host";
/// The label on every container and volume a cloud creates on a host, with the cloud's ID.
pub const OWNER_LABEL: &str = "io.horizon.cloud";
/// The engine a remote docker CLI talks to. Docker Desktop links it to its own socket.
const REMOTE_SOCKET: &str = "unix:///var/run/docker.sock";
/// The docker CLI's private configuration on every host. A configuration without a
/// login makes the CLI use the platform's credential helper, such as the macOS
/// keychain, which over SSH can wait for a person at the host; one placeholder login
/// for a registry that does not exist keeps it to plain files.
const PRIVATE_CONFIG: &str = r#"{"auths":{"horizon.invalid":{}}}"#;
/// Writes the private configuration on a remote host once, then runs docker. A
/// non-interactive SSH session often lacks the directories where Docker Desktop and
/// Homebrew install docker.
const REMOTE_PREFIX: &str = "c=\"$HOME/.horizon/docker-config\"; [ -f \"$c/config.json\" ] || \
    { mkdir -p \"$c\" && printf %s \"$1\" > \"$c/config.$$\" && mv -f \"$c/config.$$\" \"$c/config.json\"; } || exit 125; \
    PATH=\"$PATH:/usr/local/bin:/opt/homebrew/bin\" DOCKER_CONFIG=\"$c\" exec";
/// Options for every SSH connection to a host. The person's configuration still
/// applies; a shared master connection does not, because it could outlive the call
/// while it holds the call's output open.
const SSH_OPTIONS: [&str; 10] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=5",
    "-o",
    "ControlMaster=no",
    "-o",
    "ControlPath=none",
];

/// How Horizon reaches a host's Docker engine.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    /// This computer's own engine.
    ThisPc,
    /// OpenSSH with the person's own configuration, keys and agent.
    Ssh,
    /// `tailscale ssh`, which the tailnet authenticates. The host's Tailscale SSH must
    /// accept without a browser check.
    TailscaleSsh,
}

/// A saved Docker host. It holds no secret.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// A unique name, such as `build-server`, which each cloud on the host records.
    pub name: String,
    pub access: Access,
    /// The SSH destination, `[user@]host`, such as an alias from the person's SSH
    /// configuration. None for this computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The absolute path of docker on the host, such as `/usr/local/bin/docker`; else
    /// docker is looked up on the host's `PATH` and where desktop installs keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<String>,
}

impl Host {
    /// # Errors
    /// Refuses a name that is not an ID, an address that does not fit the access, and
    /// a docker path that is not a plain absolute path.
    pub fn validate(&self) -> Result<(), CloudError> {
        if !crate::valid_id(&self.name) || self.name.len() > 63 {
            return Err(CloudError::Invalid(
                "Name the Docker host with letters, digits, - and _ (63 at most)",
            ));
        }
        let address = self.address.as_deref();
        match self.access {
            Access::ThisPc if address.is_some() => {
                return Err(CloudError::Invalid("This computer's Docker host takes no address"));
            }
            Access::Ssh | Access::TailscaleSsh if !address.is_some_and(valid_destination) => {
                return Err(CloudError::Invalid(
                    "Give the Docker host an SSH address such as user@build-server",
                ));
            }
            Access::ThisPc | Access::Ssh | Access::TailscaleSsh => {}
        }
        if self.docker.as_deref().is_some_and(|path| {
            !path.starts_with('/')
                || path.len() > 256
                || !path
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/._+-".contains(&byte))
        }) {
            return Err(CloudError::Invalid(
                "The docker path must be absolute, such as /usr/local/bin/docker",
            ));
        }
        Ok(())
    }

    /// The command that runs `docker <args>` on this host with the private configuration
    /// in `local_config` or the remote one. This computer's docker therefore reads no
    /// Docker context: it talks to the engine `DOCKER_HOST` names, else to the default
    /// socket. A remote host runs it in `sh` with every
    /// argument shell-quoted, whatever its login shell, and talks to the engine's socket,
    /// not to a Docker context.
    /// # Errors
    /// As [`Host::validate`].
    pub fn command(&self, args: &[String], local_config: &Path) -> Result<Command, CloudError> {
        self.validate()?;
        let docker = self.docker.as_deref().unwrap_or("docker");
        let address = self.address.as_deref().unwrap_or_default();
        let script = || {
            let quoted: Vec<String> = args.iter().map(|arg| quote(arg)).collect();
            let script = format!(
                "{REMOTE_PREFIX} {} -H {REMOTE_SOCKET} {}",
                quote(docker),
                quoted.join(" ")
            );
            format!("sh -c {} sh {}", quote(&script), quote(PRIVATE_CONFIG))
        };
        Ok(match self.access {
            Access::ThisPc => {
                let mut command = Command::new(self.docker.as_deref().map_or_else(|| local_docker(), Path::new));
                command.env("DOCKER_CONFIG", local_config).args(args);
                command
            }
            Access::Ssh => {
                let mut command = Command::new("ssh");
                command.args(SSH_OPTIONS).arg("--").arg(address).arg(script());
                command
            }
            Access::TailscaleSsh => {
                // The wrapper takes the destination first and passes the rest to OpenSSH.
                let mut command = Command::new("tailscale");
                command.args(["ssh", address]).args(SSH_OPTIONS).arg(script());
                command
            }
        })
    }

    /// The SSH `ProxyCommand` that reaches a worker on this host's loopback, or None
    /// for this computer, whose loopback is the worker's own. OpenSSH runs it in a
    /// POSIX shell, so a Windows controller cannot use it.
    /// # Errors
    /// As [`Host::validate`].
    pub fn proxy_command(&self) -> Result<Option<String>, CloudError> {
        self.validate()?;
        let address = self.address.as_deref().map(quote);
        Ok(match (self.access, address) {
            (Access::Ssh, Some(address)) => Some(format!(
                "ssh -o BatchMode=yes -o ConnectTimeout=10 -o ControlMaster=no -o ControlPath=none -W 127.0.0.1:%p -- {address}"
            )),
            (Access::TailscaleSsh, Some(address)) => Some(format!(
                "tailscale ssh {address} -o BatchMode=yes -o ConnectTimeout=10 -o ControlMaster=no -o ControlPath=none -W 127.0.0.1:%p"
            )),
            _ => None,
        })
    }
}

/// This computer's docker: the one on `PATH`, else where Docker Desktop and Homebrew
/// install it, as an app started from a desktop often has a short `PATH`.
fn local_docker() -> &'static Path {
    ["/usr/local/bin/docker", "/opt/homebrew/bin/docker"]
        .into_iter()
        .map(Path::new)
        .find(|path| path.is_file() && !on_path("docker"))
        .unwrap_or(Path::new("docker"))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|directory| directory.join(program).is_file()))
}

/// `[user@]host`, with a user that cannot be read as an option.
fn valid_destination(address: &str) -> bool {
    let (user, host) = address
        .split_once('@')
        .map_or((None, address), |(user, host)| (Some(user), host));
    let user_ok = user.is_none_or(|user| {
        !user.is_empty()
            && user.len() <= 32
            && !user.starts_with('-')
            && user
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    });
    // An alias from the SSH configuration may hold `_`, which a DNS name may not.
    let alias = |host: &str| {
        !host.starts_with('-')
            && host.len() <= 253
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    user_ok && (host.parse::<SshHost>().is_ok() || (!host.is_empty() && alias(host)))
}

/// `value` as one POSIX shell word.
#[must_use]
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
