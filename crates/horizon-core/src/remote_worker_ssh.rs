//! Crate-private pinned SSH command construction, not remote attachment authority.

pub(crate) mod query;
mod trust;

use trust::KnownHosts;
pub(crate) use trust::known_hosts;

use crate::{
    Terminal, cloud_run::CloudJobId, cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    remote_ssh_identity::RemoteSshIdentity, remote_worker_status::RemotePanelStatusError as Error,
    remote_workspace::valid_local_id, terminal::TerminalSpawnOptions,
};
use std::{path::Path, process::Command, sync::Arc};

pub(crate) const HOST_ALIAS: &str = "horizon-retained-worker";

/// Proves the original saved host key and client identity at candidate coordinates.
/// An open socket or a timeout is not success; the fixed no-op must exit successfully.
pub(crate) fn prove_endpoint(
    identity: &RemoteSshIdentity,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<(), query::Error> {
    let trust = known_hosts(identity, endpoint).map_err(|_| query::Error::QueryFailed)?;
    let command = command_for(identity.private_key_path(), trust.path(), endpoint, Operation::Probe)
        .map_err(|_| query::Error::QueryFailed)?;
    let output = query::run(command, &[], std::time::Duration::from_secs(10), 1)?;
    if !output.is_empty() {
        return Err(query::Error::QueryFailed);
    }
    Ok(())
}

pub(crate) fn prepared_command(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<Command, Error> {
    command_for(identity, known_hosts, endpoint, Operation::Request)
}

pub(crate) fn prepared_pack_status(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<Command, Error> {
    command_for(identity, known_hosts, endpoint, Operation::PackStatus)
}

pub(crate) fn prepared_intake(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
    observe: bool,
) -> Result<Command, Error> {
    command_for(
        identity,
        known_hosts,
        endpoint,
        if observe {
            Operation::IntakeStatus
        } else {
            Operation::Intake
        },
    )
}

pub(crate) fn prepared_storage_status(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<Command, Error> {
    command_for(identity, known_hosts, endpoint, Operation::StorageStatus)
}

pub(crate) fn prepared_github_install(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
) -> Result<Command, Error> {
    command_for(identity, known_hosts, endpoint, Operation::GithubInstall)
}

#[derive(Clone, Copy)]
enum Operation<'a> {
    Probe,
    Request,
    PackStatus,
    Intake,
    IntakeStatus,
    StorageStatus,
    GithubInstall,
    GitSetup { observe: bool },
    Attach { runtime: CloudJobId, panel: &'a str },
}

fn command_for(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
    operation: Operation<'_>,
) -> Result<Command, Error> {
    if !endpoint.is_complete() {
        return Err(Error::WorkerUnavailable);
    }
    let mut command = Command::new("ssh");
    let terminal_mode = match operation {
        Operation::Probe
        | Operation::Request
        | Operation::PackStatus
        | Operation::Intake
        | Operation::IntakeStatus
        | Operation::StorageStatus
        | Operation::GithubInstall
        | Operation::GitSetup { .. } => "-T",
        Operation::Attach { panel, .. } if valid_local_id(panel) => "-tt",
        Operation::Attach { .. } => return Err(Error::UnknownPanel),
    };
    command.args(["-F", "none", "-S", "none", terminal_mode]);
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
    command.arg(match operation {
        Operation::Probe => "/usr/bin/true".into(),
        Operation::Request => "/usr/local/bin/horizon-panel-session request".into(),
        Operation::PackStatus => "/usr/local/bin/horizon-repository pack-status".into(),
        Operation::Intake => "/usr/local/bin/horizon-repository intake".into(),
        Operation::IntakeStatus => "/usr/local/bin/horizon-repository intake-status".into(),
        Operation::StorageStatus => "/usr/local/bin/horizon-repository storage-status".into(),
        Operation::GithubInstall => "/usr/local/bin/horizon-github-credential install".into(),
        Operation::GitSetup { observe: true } => "/usr/local/bin/horizon-repository git-status".into(),
        Operation::GitSetup { observe: false } => "/usr/local/bin/horizon-setup-launch --git".into(),
        Operation::Attach { runtime, panel } => {
            format!("/usr/local/bin/horizon-panel-session attach -- {runtime} {panel}")
        }
    });
    command.env("SSH_ASKPASS_REQUIRE", "never");
    Ok(command)
}

pub(crate) fn prepared_git_setup(
    identity: &Path,
    known_hosts: &Path,
    endpoint: &InteractiveWorkerSshEndpoint,
    observe: bool,
) -> Result<Command, Error> {
    command_for(identity, known_hosts, endpoint, Operation::GitSetup { observe })
}

/// Private preparation must be consumed by the fresh admission boundary, never persisted.
pub(crate) struct PreparedAttachment {
    command: Command,
    known_hosts: KnownHosts,
}

impl PreparedAttachment {
    pub(crate) fn new(
        identity: &RemoteSshIdentity,
        endpoint: &InteractiveWorkerSshEndpoint,
        runtime: CloudJobId,
        panel: &str,
    ) -> Result<Self, Error> {
        let known_hosts = known_hosts(identity, endpoint)?;
        let command = command_for(
            identity.private_key_path(),
            known_hosts.path(),
            endpoint,
            Operation::Attach { runtime, panel },
        )?;
        Ok(Self { command, known_hosts })
    }

    pub(crate) fn spawn(self, mut options: TerminalSpawnOptions) -> crate::Result<Terminal> {
        options.program = "ssh".into();
        options.args = self
            .command
            .get_args()
            .map(|argument| argument.to_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| crate::Error::State("protected SSH arguments are not supported".into()))?;
        options.env.insert("SSH_ASKPASS_REQUIRE".into(), "never".into());
        options.env.insert("TERM".into(), "xterm-256color".into());
        Terminal::spawn_with_ssh_trust(options, Arc::new(self.known_hosts.into_file()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HorizonHome, cloud_run::CloudWorkflowId, remote_ssh_identity::RemoteSshIdentityStore};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn storage_status_only_changes_the_fixed_remote_command() {
        use base64::Engine as _;
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend_from_slice(&[1; 32]);
        let endpoint = InteractiveWorkerSshEndpoint {
            host: "127.0.0.1".into(),
            port: 2222,
            username: "root".into(),
            host_key: format!("ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(blob)),
        };
        let identity = Path::new("/private/client key");
        let trust = Path::new("/private/known hosts");
        let baseline = prepared_command(identity, trust, &endpoint).expect("query");
        let probe = command_for(identity, trust, &endpoint, Operation::Probe).expect("probe");
        let mut probe_args: Vec<_> = baseline.get_args().map(std::ffi::OsStr::to_os_string).collect();
        *probe_args.last_mut().expect("command") = "/usr/bin/true".into();
        assert_eq!(probe.get_args().collect::<Vec<_>>(), probe_args);
        assert_eq!(
            probe.get_envs().collect::<Vec<_>>(),
            baseline.get_envs().collect::<Vec<_>>()
        );
        let storage = prepared_storage_status(identity, trust, &endpoint).expect("storage query");
        let mut expected: Vec<_> = baseline.get_args().map(std::ffi::OsStr::to_os_string).collect();
        *expected.last_mut().expect("fixed command") = "/usr/local/bin/horizon-repository storage-status".into();
        assert_eq!(storage.get_args().collect::<Vec<_>>(), expected);
        assert_eq!(
            storage.get_envs().collect::<Vec<_>>(),
            baseline.get_envs().collect::<Vec<_>>()
        );
    }

    #[test]
    fn interactive_mode_shares_all_isolation_options_and_owns_unique_private_trust() {
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        let identities = RemoteSshIdentityStore::new(&HorizonHome::from_root(directory.path().join("home")));
        let runtime = CloudJobId::new();
        let identity = identities
            .prepare_new(CloudWorkflowId::new(), runtime)
            .expect("identity");
        let endpoint = InteractiveWorkerSshEndpoint {
            host: "127.0.0.1".into(),
            port: 2222,
            username: "horizon".into(),
            host_key: identity.public_key().into(),
        };
        let first = PreparedAttachment::new(&identity, &endpoint, runtime, "-h").expect("first");
        let second = PreparedAttachment::new(&identity, &endpoint, runtime, "terminal").expect("second");
        let first_path = first.known_hosts.path().to_path_buf();
        let second_path = second.known_hosts.path().to_path_buf();
        let first_inode = std::fs::metadata(&first_path).expect("first inode");
        let second_inode = std::fs::metadata(&second_path).expect("second inode");
        assert_ne!(first_path, second_path);
        assert_eq!(
            first_path.parent(),
            Some(Path::new(&format!("/proc/{}/fd", std::process::id())))
        );
        assert_eq!(std::fs::metadata(&first_path).expect("anonymous inode").nlink(), 0);
        assert_eq!(
            std::fs::metadata(&first_path).expect("mode").permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            std::fs::read_to_string(&first_path).expect("pin"),
            format!("{HOST_ALIAS} {}\n", endpoint.host_key)
        );
        let query = prepared_command(identity.private_key_path(), &first_path, &endpoint).expect("query");
        let pack = prepared_pack_status(identity.private_key_path(), &first_path, &endpoint).expect("pack query");
        let mut pack_args: Vec<_> = query.get_args().map(std::ffi::OsStr::to_os_string).collect();
        *pack_args.last_mut().expect("fixed helper") = "/usr/local/bin/horizon-repository pack-status".into();
        assert_eq!(pack.get_args().collect::<Vec<_>>(), pack_args);
        assert_eq!(
            pack.get_envs().collect::<Vec<_>>(),
            query.get_envs().collect::<Vec<_>>()
        );
        for (observe, operation) in [(false, "intake"), (true, "intake-status")] {
            let intake = prepared_intake(identity.private_key_path(), &first_path, &endpoint, observe).expect("intake");
            *pack_args.last_mut().expect("fixed intake") =
                format!("/usr/local/bin/horizon-repository {operation}").into();
            assert_eq!(intake.get_args().collect::<Vec<_>>(), pack_args);
            assert_eq!(
                intake.get_envs().collect::<Vec<_>>(),
                query.get_envs().collect::<Vec<_>>()
            );
        }
        let mut expected: Vec<_> = query.get_args().map(std::ffi::OsStr::to_os_string).collect();
        expected[4] = "-tt".into();
        *expected.last_mut().expect("helper") =
            format!("/usr/local/bin/horizon-panel-session attach -- {runtime} -h").into();
        assert_eq!(first.command.get_args().collect::<Vec<_>>(), expected);
        assert_eq!(
            first.command.get_envs().collect::<Vec<_>>(),
            query.get_envs().collect::<Vec<_>>()
        );
        let parsed = Command::new("ssh")
            .arg("-G")
            .args(first.command.get_args())
            .output()
            .expect("SSH parser");
        assert!(parsed.status.success());
        assert!(
            String::from_utf8(parsed.stdout)
                .expect("config")
                .lines()
                .any(|line| line == "requesttty force")
        );
        for panel in ["", "../panel", "panel;start", "panel\nstart", "panel with spaces"] {
            assert!(matches!(
                PreparedAttachment::new(&identity, &endpoint, runtime, panel),
                Err(Error::UnknownPanel)
            ));
        }
        drop(first);
        assert!(
            !std::fs::metadata(&first_path)
                .is_ok_and(|metadata| { metadata.dev() == first_inode.dev() && metadata.ino() == first_inode.ino() })
        );
        assert!(second_path.exists());
        drop(second);
        assert!(
            !std::fs::metadata(&second_path)
                .is_ok_and(|metadata| { metadata.dev() == second_inode.dev() && metadata.ino() == second_inode.ino() })
        );
        assert_eq!(
            std::fs::read_dir(identity.private_key_path().parent().expect("parent"))
                .expect("directory")
                .count(),
            1
        );
    }
}
