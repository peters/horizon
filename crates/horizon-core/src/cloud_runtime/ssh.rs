//! Per-worker SSH identity, known-host binding and persistent tmux attachment.
use super::{Error, Result, WorkerContract, command::Runner, settings::Settings, state::Session, worker_contract};
use horizon_cloud::{Worker, valid_id};
use std::{path::Path, process::Command, time::Duration};
#[derive(Clone, Debug)]
pub struct Connection {
    pub host: String,
    pub port: u16,
    pub identity: std::path::PathBuf,
    pub known_hosts: std::path::PathBuf,
    pub host_key_alias: String,
}
impl Connection {
    /// # Errors
    /// Requires a usable public SSH mapping from the reconciled worker.
    pub fn new(worker: &Worker, settings: &Settings, root: &Path) -> Result<Self> {
        let address = worker
            .ssh_address()
            .ok_or(Error::Invalid("Worker has no SSH endpoint yet"))?;
        if !valid_id(&worker.id) {
            return Err(Error::Invalid("Invalid worker ID"));
        }
        Ok(Self {
            host: address.ip().to_string(),
            port: address.port(),
            identity: settings.ssh_identity_file.clone(),
            known_hosts: root.join(format!("known-hosts-{}", worker.id)),
            host_key_alias: format!("horizon-cloud-{}", worker.id),
        })
    }
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        self.arguments("accept-new")
    }
    fn arguments(&self, host_check: &str) -> Vec<String> {
        vec![
            "-F".into(),
            "none".into(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ControlPath=none".into(),
            "-o".into(),
            "ControlPersist=no".into(),
            "-o".into(),
            "ForkAfterAuthentication=no".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            format!("StrictHostKeyChecking={host_check}"),
            "-o".into(),
            format!("HostKeyAlias={}", self.host_key_alias),
            "-o".into(),
            format!("UserKnownHostsFile={}", self.known_hosts.display()),
            "-o".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "-i".into(),
            self.identity.to_string_lossy().into_owned(),
            "-p".into(),
            self.port.to_string(),
            format!("root@{}", self.host),
        ]
    }
    #[must_use]
    pub fn command(&self, remote: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(self.args()).arg(remote);
        cmd
    }
    /// Use only a previously pinned host key; never enroll a key during recovery.
    #[must_use]
    pub fn pinned_command(&self, remote: &str) -> Command {
        let mut command = Command::new("ssh");
        command.args(self.arguments("yes")).arg(remote);
        command
    }
    pub(super) fn pinned_attachment(&self, encoded: &str) -> Vec<String> {
        let mut arguments = self.arguments("yes");
        arguments.insert(0, "-tt".into());
        arguments.push(format!("horizon-cloud-worker attach-project-session {encoded}"));
        arguments
    }
    /// # Errors
    /// Checks the worker runtime through the existing OpenSSH transport and reports
    /// the optional features of the image that is actually running.
    pub fn ready(
        &self,
        runner: &Runner<'_>,
        capabilities: &horizon_cloud::Capabilities,
        timeout: Duration,
    ) -> Result<WorkerContract> {
        let output = runner.run(
            "SSH readiness",
            &mut self.command(&worker_contract::readiness_command(capabilities)?),
            timeout.min(Duration::from_secs(40)),
        )?;
        worker_contract::validate(&output, capabilities, false, false)?;
        Ok(WorkerContract::reported(&output))
    }
    /// # Errors
    /// Transfers the exact task-owned pack; does not overwrite a remote worktree.
    pub fn transfer(&self, pack: &Path, revision: &str, runner: &Runner<'_>) -> Result<()> {
        if !valid_revision(revision) {
            return Err(Error::Invalid("Invalid committed revision"));
        }
        self.upload(pack, "horizon-transfer.pack", runner)?;
        runner.run(
            super::timeline::IMPORTING_OBJECTS,
            &mut self.command(&format!("horizon-worker-import {revision}")),
            Duration::from_secs(120),
        )?;
        Ok(())
    }
    /// # Errors
    /// Uploads only verified source dependencies, then imports them on the worker.
    pub fn transfer_material(&self, archive: &Path, runner: &Runner<'_>) -> Result<()> {
        self.upload(archive, "horizon-source.tar", runner)?;
        runner.run(
            super::timeline::IMPORTING_DEPENDENCIES,
            &mut self.command("horizon-worker-source import"),
            Duration::from_secs(300),
        )?;
        Ok(())
    }
    /// # Errors
    /// Uploads a sibling's pack into its own upload directory and imports it into that
    /// sibling's repository only.
    pub fn transfer_sibling(&self, alias: &str, pack: &Path, revision: &str, runner: &Runner<'_>) -> Result<()> {
        if !valid_revision(revision) {
            return Err(Error::Invalid("Invalid committed revision"));
        }
        let remote = SiblingRemote::new(alias)?;
        runner.run(
            "Sibling upload directory",
            &mut self.command(&remote.stage),
            Duration::from_secs(20),
        )?;
        self.upload(pack, &remote.pack, runner)?;
        runner.run(
            super::timeline::IMPORTING_OBJECTS,
            &mut self.command(&remote.import(revision)),
            Duration::from_secs(120),
        )?;
        Ok(())
    }
    /// # Errors
    /// Uploads a sibling's verified source dependencies and imports them for it alone.
    pub fn transfer_sibling_material(&self, alias: &str, archive: &Path, runner: &Runner<'_>) -> Result<()> {
        let remote = SiblingRemote::new(alias)?;
        self.upload(archive, &remote.archive, runner)?;
        runner.run(
            super::timeline::IMPORTING_DEPENDENCIES,
            &mut self.command(&remote.material),
            Duration::from_secs(300),
        )?;
        Ok(())
    }
    /// # Errors
    /// Records the imported siblings and their checkout directories from `manifest`, a
    /// [`super::siblings::Set::manifest`]. The worker refuses a manifest naming a revision
    /// it has not imported and keeps the previous one.
    pub fn record_siblings(&self, manifest: &str, runner: &Runner<'_>) -> Result<()> {
        runner.run(
            "Sibling manifest",
            &mut self.command(&manifest_command(manifest)?),
            Duration::from_secs(20),
        )?;
        Ok(())
    }
    fn upload(&self, source: &Path, destination: &str, runner: &Runner<'_>) -> Result<()> {
        let mut scp = Command::new("scp");
        let args = self.args();
        // scp uses -P for the port, but otherwise shares OpenSSH's options.
        let mut index = 0;
        while index + 1 < args.len() {
            let flag = if args[index] == "-p" {
                "-P"
            } else {
                args[index].as_str()
            };
            scp.arg(flag).arg(&args[index + 1]);
            index += 2;
        }
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        scp.arg(source).arg(format!("root@{host}:/workspace/{destination}"));
        runner.transfer(
            super::timeline::UPLOADING_SOURCE,
            &scp,
            super::command::terminal_progress::Transfer::File(source.metadata()?.len()),
            Duration::from_secs(600),
        )?;
        Ok(())
    }
    /// # Errors
    /// Checks session identifiers before constructing an interactive attachment.
    pub fn attach_args(&self, session: &Session, revision: &str) -> Result<Vec<String>> {
        if !valid_id(&session.panel_id)
            || !valid_id(&session.tmux)
            || !valid_revision(revision)
            || !matches!(session.agent.as_str(), "codex" | "claude" | "grok" | "shell")
        {
            return Err(Error::Invalid("Invalid remote session identity"));
        }
        let mut args = self.args();
        args.insert(0, "-tt".into());
        args.push(format!(
            "horizon-worker-session {} {} {revision}",
            session.panel_id, session.agent
        ));
        Ok(args)
    }
}
/// The worker commands and `/workspace`-relative upload paths of one sibling.
struct SiblingRemote {
    alias: String,
    stage: String,
    pack: String,
    archive: String,
    material: String,
}

impl SiblingRemote {
    /// Only an alias the worker accepts, which also keeps it safe as shell text.
    fn new(alias: &str) -> Result<Self> {
        if !horizon_cloud::companions::valid_alias(alias) {
            return Err(Error::Invalid("Invalid same-worker sibling alias"));
        }
        Ok(Self {
            alias: alias.to_owned(),
            stage: format!("horizon-worker-siblings stage {alias}"),
            pack: format!("siblings/{alias}/horizon-transfer.pack"),
            archive: format!("siblings/{alias}/horizon-source.tar"),
            material: format!("horizon-worker-source import --sibling {alias}"),
        })
    }

    fn import(&self, revision: &str) -> String {
        format!("horizon-worker-import {revision} --sibling {}", self.alias)
    }
}

/// `horizon-worker-siblings set` reading `manifest` from a quoted here-document, which the
/// manifest's character set cannot end early or expand.
fn manifest_command(manifest: &str) -> Result<String> {
    if !manifest
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"{}[]\":,._-".contains(&byte))
    {
        return Err(Error::Invalid("Invalid same-worker sibling manifest"));
    }
    Ok(format!(
        "horizon-worker-siblings set <<'HORIZON_SIBLINGS'\n{manifest}\nHORIZON_SIBLINGS"
    ))
}
fn valid_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_cloud::{Cancellation, Capabilities};
    use std::{net::TcpListener, sync::mpsc, thread, time::Instant};

    #[test]
    fn readiness_budget_interrupts_a_stalled_ssh_handshake() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let connection = Connection {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            identity: root.path().join("missing-test-key"),
            known_hosts: root.path().join("known-hosts"),
            host_key_alias: "readiness-budget-fixture".into(),
        };
        let (accepted, connected) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if let Ok((stream, _)) = listener.accept() {
                    accepted.send(()).unwrap();
                    let _ = wait.recv_timeout(Duration::from_secs(2));
                    drop(stream);
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: vec![],
        };
        let started = Instant::now();
        let result = connection.ready(&runner, &Capabilities::default(), Duration::from_millis(150));
        let elapsed = started.elapsed();
        let did_connect = connected.try_recv().is_ok();
        let _ = release.send(());
        server.join().unwrap();
        assert!(did_connect, "SSH must reach the stalled handshake");
        assert!(result.is_err());
        assert!(
            elapsed < Duration::from_secs(1),
            "probe ignored its remaining budget: {elapsed:?}"
        );
        assert!(!connection.known_hosts.exists());
    }

    #[test]
    fn sibling_commands_and_uploads_stay_with_an_accepted_siblings_own_material() {
        let remote = SiblingRemote::new("native-lib").unwrap();
        assert_eq!(remote.stage, "horizon-worker-siblings stage native-lib");
        assert_eq!(remote.pack, "siblings/native-lib/horizon-transfer.pack");
        assert_eq!(remote.archive, "siblings/native-lib/horizon-source.tar");
        assert_eq!(remote.material, "horizon-worker-source import --sibling native-lib");
        assert_eq!(
            remote.import(&"a".repeat(40)),
            format!("horizon-worker-import {} --sibling native-lib", "a".repeat(40))
        );
        for alias in ["", "../escape", "Native", "native lib", "native;rm", &"a".repeat(65)] {
            assert!(SiblingRemote::new(alias).is_err(), "{alias:?}");
        }
    }

    #[test]
    fn the_manifest_travels_in_a_quoted_here_document() {
        let manifest = r#"{"version":1,"primary":"app","siblings":[]}"#;
        assert_eq!(
            manifest_command(manifest).unwrap(),
            format!("horizon-worker-siblings set <<'HORIZON_SIBLINGS'\n{manifest}\nHORIZON_SIBLINGS")
        );
        for unsafe_text in ["{'}", "{$HOME}", "{\nHORIZON_SIBLINGS\n}", "{`id`}"] {
            assert!(manifest_command(unsafe_text).is_err(), "{unsafe_text:?}");
        }
    }
}
