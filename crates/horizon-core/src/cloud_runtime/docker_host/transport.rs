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
            command.arg(format!("sh -c {}", quote(&format!("LC_ALL=C df -Pk {}", quote(root)))));
            command
        } else {
            let mut command = Command::new("df");
            command.env("LC_ALL", "C");
            command.args(["-Pk", root]);
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
    let mut lines = output.lines();
    lines.next()?;
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    if line.matches('%').count() != 1 {
        return None;
    }
    let (columns, _) = line.split_once('%')?;
    let mut fields = columns.split_whitespace().rev();
    fields.next()?.parse::<u64>().ok()?;
    let available = fields.next()?.parse::<u64>().ok()?;
    let used = fields.next()?.parse::<u64>().ok()?;
    let total = fields.next()?.parse::<u64>().ok()?;
    (available <= total && used <= total)
        .then_some(available)?
        .checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::free_bytes;

    #[test]
    fn free_space_columns_ignore_spaces_in_sources_and_mounts() {
        for line in [
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 100 30 60 30% /workspace",
            "Filesystem 1024-blocks Used Available Capacity Mounted on\nserver:/shared disk 100 30 60 30% /workspace directory",
        ] {
            assert_eq!(free_bytes(line), Some(60 * 1024));
        }
        for line in [
            "not a capacity report",
            "/dev/disk 100 30 120 30% /workspace",
            "/dev/disk 100 30 60 30% /ambiguous 1 2 3 4% path",
            "/dev/disk 18446744073709551615 0 18446744073709551615 0% /workspace",
        ] {
            assert_eq!(free_bytes(&format!("Filesystem header\n{line}")), None);
        }
        assert_eq!(free_bytes("Filesystem header\nsource\n100 30 60 30% /workspace"), None);
    }
}
