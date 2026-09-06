use super::{RemotePanelStatusError as Error, command};
use crate::{cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, remote_ssh_identity::RemoteSshIdentity};
use std::{io::Write, path::Path, process::Command, time::Duration};

const DEADLINE: Duration = Duration::from_secs(10);
const HOST_ALIAS: &str = "horizon-retained-worker";

pub(super) fn request(
    identity: &RemoteSshIdentity,
    endpoint: &InteractiveWorkerSshEndpoint,
    input: &[u8],
) -> Result<Vec<u8>, Error> {
    // The recovered key's private parent avoids ambient temporary-directory trust.
    let parent = identity.private_key_path().parent().ok_or(Error::UnsupportedPath)?;
    let mut known_hosts = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::TrustStorage)?;
    writeln!(known_hosts, "{HOST_ALIAS} {}", endpoint.host_key).map_err(|_| Error::TrustStorage)?;
    let command = prepared_command(identity.private_key_path(), known_hosts.path(), endpoint)?;
    command::run(command, input, DEADLINE)
}

pub(super) fn prepared_command(
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
