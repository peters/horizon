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
                "unset DOCKER_HOST DOCKER_CONTEXT; PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; {}",
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
        let mut command = if self.0.ssh.is_some() {
            let mut command = self.ssh()?;
            command.arg(format!("sh -c {}", quote(&format!("df -Pk {}", quote(root)))));
            command
        } else {
            let mut command = Command::new("df");
            command.args(["-Pk", root]);
            command
        };
        let output = runner.run_parsed("Docker storage probe", &mut command, Duration::from_secs(8))?;
        output
            .lines()
            .last()
            .and_then(|line| line.split_whitespace().nth(3))
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|kb| kb.checked_mul(1024))
            .ok_or(Error::Invalid("Docker workspace free space could not be measured"))
    }
}
