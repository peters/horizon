//! Bounded Docker commands over a pinned SSH connection or a local engine.
use super::{Binding, Error, Result, SshAuthentication};
use crate::cloud_runtime::command::Runner;
use std::{process::Command, time::Duration};

pub(crate) struct Transport<'a>(pub &'a Binding);

pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl Transport<'_> {
    pub(crate) fn ssh(&self) -> Result<Command> {
        self.0.validate()?;
        let ssh = self.0.ssh.as_ref().ok_or(Error::Invalid("This Docker host is local"))?;
        if ssh.authentication == SshAuthentication::Tailscale {
            let mut command = Command::new("tailscale");
            // The wrapper resolves MagicDNS and pins keys from the tailnet control plane.
            // Disable local SSH configuration, identities and connection reuse.
            // The wrapper consumes the destination first; OpenSSH parses the following options.
            command.args(["ssh", &format!("{}@{}", ssh.user, ssh.host)]);
            command.args([
                "-F",
                "none",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=2",
                "-o",
                "IdentityAgent=none",
                "-o",
                "UpdateHostKeys=no",
                "-o",
                "PreferredAuthentications=none",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
            ]);
            return Ok(command);
        }
        let mut command = Command::new("ssh");
        command.args([
            "-F",
            "none",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "GlobalKnownHostsFile=none",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
        ]);
        command.arg("-o").arg(format!(
            "UserKnownHostsFile=\"{}\"",
            ssh.known_hosts
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
        ));
        command
            .arg("-i")
            .arg(ssh.identity_file.to_string_lossy().replace('%', "%%"))
            .arg("-p")
            .arg(ssh.port.to_string());
        command.arg(format!("{}@{}", ssh.user, ssh.host));
        Ok(command)
    }

    pub(crate) fn command(&self, arguments: &[String]) -> Result<Command> {
        self.0.validate()?;
        let mut all = vec!["docker".to_owned()];
        if let Some(context) = &self.0.context {
            all.extend(["--context".to_owned(), context.clone()]);
        }
        all.extend_from_slice(arguments);
        if self.0.ssh.is_some() {
            let mut command = self.ssh()?;
            // macOS login shells do not necessarily include Docker's installation directory.
            let script = format!(
                "unset DOCKER_HOST DOCKER_CONTEXT; PATH=$PATH:/opt/homebrew/bin:/usr/local/bin; {}",
                all.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(" ")
            );
            command.arg(format!("sh -c {}", quote(&script)));
            Ok(command)
        } else {
            let mut command = Command::new("docker");
            command.env_remove("DOCKER_HOST").env_remove("DOCKER_CONTEXT");
            command.args(&all[1..]);
            Ok(command)
        }
    }

    pub(crate) fn run(&self, runner: &Runner<'_>, arguments: &[&str], timeout: Duration) -> Result<String> {
        self.run_owned(
            runner,
            &arguments.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            timeout,
        )
    }

    pub(crate) fn run_owned(&self, runner: &Runner<'_>, arguments: &[String], timeout: Duration) -> Result<String> {
        runner.run_parsed("Docker host request", &mut self.command(arguments)?, timeout)
    }

    pub(crate) fn disk_free(&self, runner: &Runner<'_>, root: &str) -> Result<u64> {
        if !root.starts_with('/') || root.contains(['\r', '\n', '\0']) {
            return Err(Error::Invalid("Docker storage needs an absolute Linux path"));
        }
        let mut command = if self.0.ssh.is_some() {
            let mut command = self.ssh()?;
            command.arg(format!(
                "sh -c {}",
                quote(&format!("LC_ALL=C stat -f -c {} {}", quote("%a %S"), quote(root)))
            ));
            command
        } else {
            let mut command = Command::new("stat");
            command.env("LC_ALL", "C");
            command.args(["-f", "-c", "%a %S", root]);
            command
        };
        let output = runner.run_parsed("Docker storage probe", &mut command, Duration::from_secs(8))?;
        free_bytes(&output).ok_or(Error::Invalid("Docker workspace free space could not be measured"))
    }

    pub(crate) fn host_kernel(&self, runner: &Runner<'_>) -> Result<String> {
        let mut command = if self.0.ssh.is_some() {
            let mut command = self.ssh()?;
            command.arg(format!("sh -c {}", quote("uname -sr")));
            command
        } else {
            let mut command = Command::new("uname");
            command.arg("-sr");
            command
        };
        runner.run_parsed("Docker engine host", &mut command, Duration::from_secs(8))
    }
}

fn free_bytes(output: &str) -> Option<u64> {
    let mut fields = output.split_whitespace();
    let available = fields.next()?.parse::<u64>().ok()?;
    let block_size = fields.next()?.parse::<u64>().ok()?;
    if fields.next().is_some() || block_size == 0 {
        return None;
    }
    available.checked_mul(block_size)
}

#[cfg(test)]
mod tests {
    use super::free_bytes;

    #[test]
    fn filesystem_capacity_uses_available_blocks_and_fundamental_block_size() {
        assert_eq!(free_bytes("60 4096\n"), Some(60 * 4096));
        assert_eq!(free_bytes("0 4096\n"), Some(0));
        for output in [
            "",
            "not capacity",
            "60",
            "60 0",
            "60 4096 extra",
            "18446744073709551615 4096",
        ] {
            assert_eq!(free_bytes(output), None);
        }
    }

    // A native Linux engine uses the host's filesystem-capable stat utility.
    #[cfg(target_os = "linux")]
    #[test]
    fn filesystem_queries_accept_spaces_and_percent_signs_in_paths() {
        use super::*;
        let root = tempfile::Builder::new()
            .prefix("storage% with spaces")
            .tempdir()
            .unwrap();
        let host = Binding {
            id: "fixture".into(),
            name: "Fixture".into(),
            ssh: None,
            context: None,
            allow_emulation: false,
        };
        let cancel = crate::cloud_runtime::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        assert!(
            Transport(&host)
                .disk_free(&runner, root.path().to_str().unwrap())
                .is_ok()
        );
    }
}
