//! Crate-private pinned SSH command construction, not remote attachment authority.

use crate::{
    cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, remote_worker_status::RemotePanelStatusError as Error,
};
use std::{path::Path, process::Command};

pub(crate) const HOST_ALIAS: &str = "horizon-retained-worker";

pub(crate) fn prepared_command(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<Command, Error> {
    if !endpoint.is_complete() {
        return Err(Error::WorkerUnavailable);
    }
    let mut command = Command::new("ssh");
    command.args(["-F", "none", "-S", "none", "-T"]);
    for option in [
        "BatchMode=yes",
        "IdentitiesOnly=yes",
        "IdentityAgent=none",
        "AddKeysToAgent=no",
        "PreferredAuthentications=publickey",
        "PubkeyAcceptedAlgorithms=ssh-ed25519",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "NumberOfPasswordPrompts=0",
        "StrictHostKeyChecking=yes",
        "HostKeyAlgorithms=ssh-ed25519",
        "UpdateHostKeys=no",
        "GlobalKnownHostsFile=/dev/null",
        "KnownHostsCommand=none",
        "VerifyHostKeyDNS=no",
        "CheckHostIP=no",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "ProxyCommand=none",
        "ProxyJump=none",
        "ConnectionAttempts=1",
        "ConnectTimeout=5",
        "ServerAliveInterval=2",
        "ServerAliveCountMax=1",
        "EscapeChar=none",
    ] {
        command.args(["-o", option]);
    }
    command.arg("-o").arg(format!("HostKeyAlias={HOST_ALIAS}"));
    command.arg("-o").arg(path_option("IdentityFile", identity)?);
    command.arg("-o").arg(path_option("UserKnownHostsFile", known_hosts)?);
    command.args([
        "-p",
        &endpoint.port.to_string(),
        "-l",
        &endpoint.username,
        "--",
        &endpoint.host,
    ]);
    command.arg("/usr/local/bin/horizon-panel-session request");
    command.env("SSH_ASKPASS_REQUIRE", "never");
    Ok(command)
}

fn path_option(name: &str, path: &Path) -> Result<String, Error> {
    let path = path
        .to_str()
        .filter(|_| path.is_absolute())
        .ok_or(Error::UnsupportedPath)?;
    if path.chars().any(|character| character.is_control() || character == '$') {
        return Err(Error::UnsupportedPath);
    }
    // SSH applies its own configuration quoting and percent expansion after argv parsing.
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    Ok(format!("{name}=\"{escaped}\""))
}
