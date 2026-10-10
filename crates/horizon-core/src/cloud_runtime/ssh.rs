//! Per-worker SSH identity, known-host binding and persistent tmux attachment.
use super::{Error, Result, WorkerContract, command::Runner, settings::Settings, state::Session, worker_contract};
use horizon_cloud::{Worker, valid_id};
use std::{path::Path, process::Command, time::Duration};
/// The account every worker's sshd accepts.
pub const USER: &str = "root";
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
            .ssh_endpoint()
            .ok_or(Error::Invalid("Worker has no SSH endpoint yet"))?;
        if !valid_id(&worker.id) {
            return Err(Error::Invalid("Invalid worker ID"));
        }
        Ok(Self {
            host: address.host().to_string(),
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
            format!("{USER}@{}", self.host),
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
    /// The connection options with only a previously pinned host key, ending with the destination.
    pub(super) fn pinned_args(&self) -> Vec<String> {
        self.arguments("yes")
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
    /// Prepares the one shared checkout on the worker, once, before any panel exists, so the
    /// first panel finds it ready and a storage or source problem fails the deployment. A
    /// failure recorded by an earlier attempt is cleared first, as the person asked to retry;
    /// no files are reset. Repeating it once the checkout is ready changes nothing.
    pub fn prepare_checkout(&self, revision: &str, runner: &Runner<'_>) -> Result<()> {
        runner.run(
            super::timeline::PREPARING_CHECKOUT,
            &mut self.command(&prepare_checkout_command(revision)?),
            Duration::from_mins(30),
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
        let directory = runner.run(
            "Sibling upload directory",
            &mut self.command(&remote.stage),
            Duration::from_secs(20),
        )?;
        self.upload(pack, &remote.upload_path(&directory, "horizon-transfer.pack")?, runner)?;
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
        let directory = runner.run(
            "Sibling upload directory",
            &mut self.command(&remote.stage),
            Duration::from_secs(20),
        )?;
        self.upload(archive, &remote.upload_path(&directory, "horizon-source.tar")?, runner)?;
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
    fn upload_command(&self, source: &Path, destination: &str) -> Command {
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
        scp
    }
    fn upload(&self, source: &Path, destination: &str, runner: &Runner<'_>) -> Result<()> {
        let scp = self.upload_command(source, destination);
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
    pub fn attach_args(
        &self,
        session: &Session,
        revision: &str,
        tailnet: &super::tailnet::Selection,
    ) -> Result<Vec<String>> {
        if !valid_id(&session.panel_id)
            || !valid_id(&session.tmux)
            || !valid_revision(revision)
            || !matches!(session.agent.as_str(), "codex" | "claude" | "grok" | "shell")
        {
            return Err(Error::Invalid("Invalid remote session identity"));
        }
        let mut args = self.args();
        args.insert(0, "-tt".into());
        let shared = session
            .worktree
            .strip_prefix(super::siblings::SHARED_CHECKOUT_ROOT)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'));
        let mut guard = String::new();
        if shared || tailnet.tailnet.is_some() {
            guard.push_str(
                "contract=$(horizon-worker-check) || { status=$?; printf '%s\\n' \"$contract\"; exit \"$status\"; }; ",
            );
        }
        if shared {
            guard.push_str("if ! printf '%s\\n' \"$contract\" | grep -qx 'horizon-shared-checkout-contract=1'; then \
             printf '%s\\n' 'Rebuild the cloud worker image before adding panels: shared checkouts are not supported.'; \
             exit 3; fi; ");
        }
        if tailnet.tailnet.is_some() {
            for marker in worker_contract::TAILNET_MARKERS {
                guard.push_str("if ! printf '%s\\n' \"$contract\" | grep -qx '");
                guard.push_str(marker);
                guard.push_str("'; then \
                    printf '%s\\n' 'Rebuild the cloud worker image before adding panels: tagged tailnet enrollment is not supported.'; \
                    exit 3; fi; ");
            }
        }
        let option = if shared { "--shared " } else { "" };
        args.push(format!(
            "{guard}horizon-worker-session {option}{} {} {revision}",
            session.panel_id, session.agent
        ));
        Ok(args)
    }
}
/// The worker commands and `/workspace`-relative upload paths of one sibling.
struct SiblingRemote {
    alias: String,
    stage: String,
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
            material: format!("horizon-worker-source import --sibling {alias}"),
        })
    }

    fn upload_path(&self, directory: &str, name: &str) -> Result<String> {
        let directory = directory.trim();
        if ![
            format!("/workspace/siblings/{}", self.alias),
            format!("/workspace/.horizon-tailnet/uploads/{}", self.alias),
        ]
        .contains(&directory.to_owned())
        {
            return Err(Error::Invalid("Worker returned an invalid sibling upload directory"));
        }
        Ok(format!("{}/{name}", &directory["/workspace/".len()..]))
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
/// The worker command that prepares the shared checkout without binding a session.
fn prepare_checkout_command(revision: &str) -> Result<String> {
    if !valid_revision(revision) {
        return Err(Error::Invalid("Invalid committed revision"));
    }
    Ok(format!(
        "horizon-worker-session --retry-shared-checkout && horizon-worker-session --shared --prepare-only prepare-checkout shell {revision}"
    ))
}
fn valid_revision(value: &str) -> bool {
    super::repository::is_commit_id(value)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_cloud::{Cancellation, Capabilities};
    use std::{net::TcpListener, sync::mpsc, thread, time::Instant};

    #[test]
    fn the_checkout_is_prepared_by_one_worker_command_for_a_valid_revision_only() {
        let revision = "a".repeat(40);
        let command = prepare_checkout_command(&revision).unwrap();
        // A failure recorded by an earlier attempt is cleared first, then the checkout is prepared
        // with no session bound.
        assert_eq!(
            command,
            format!(
                "horizon-worker-session --retry-shared-checkout && \
                 horizon-worker-session --shared --prepare-only prepare-checkout shell {revision}"
            )
        );
        for invalid in ["", "main", "a; rm -rf /", &"A".repeat(40), &"a".repeat(41)] {
            assert!(prepare_checkout_command(invalid).is_err(), "{invalid:?} is refused");
        }
    }

    fn attachment_fixture() -> (tempfile::TempDir, Connection, Session) {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        for (name, body) in [
            (
                "horizon-worker-check",
                "printf '%s\\n' \"$CHECK_MARKER\"; exit \"$CHECK_EXIT\"",
            ),
            ("horizon-worker-session", "printf '%s\\n' \"$@\" > \"$SESSION_LOG\""),
        ] {
            let file = root.path().join(name);
            fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let connection = Connection {
            host: "example.invalid".into(),
            port: 22,
            identity: root.path().join("key"),
            known_hosts: root.path().join("known-hosts"),
            host_key_alias: "fixture".into(),
        };
        let session = Session {
            panel_id: "panel".into(),
            agent: "shell".into(),
            tmux: "panel".into(),
            branch: String::new(),
            worktree: super::super::siblings::shared_worktree(None),
        };
        (root, connection, session)
    }

    #[test]
    fn attachment_refuses_old_shared_or_selected_tailnet_images_and_preserves_none() {
        use std::fs;
        let (root, connection, mut session) = attachment_fixture();
        let log = root.path().join("session.log");
        let run = |session: &Session, marker: &str, status: &str, selected: bool| {
            let tailnet = super::super::tailnet::Selection {
                tailnet: selected.then(|| "synthetic-tailnet".into()),
            };
            let args = connection.attach_args(session, &"a".repeat(40), &tailnet).unwrap();
            Command::new("/bin/sh")
                .args(["-c", args.last().unwrap()])
                .env("PATH", format!("{}:/usr/bin:/bin", root.path().display()))
                .env("CHECK_MARKER", marker)
                .env("CHECK_EXIT", status)
                .env("SESSION_LOG", &log)
                .output()
                .unwrap()
        };
        let refused = run(&session, "horizon-worker-contract=1", "0", false);
        assert_eq!(refused.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&refused.stdout).contains("Rebuild the cloud worker image"));
        assert!(!log.exists());
        let failed = run(&session, "Worker runtime validation failed", "7", false);
        assert_eq!(failed.status.code(), Some(7));
        assert_eq!(
            String::from_utf8_lossy(&failed.stdout).trim(),
            "Worker runtime validation failed"
        );
        assert_eq!(
            run(&session, "horizon-shared-checkout-contract=1", "1", false)
                .status
                .code(),
            Some(1)
        );
        assert!(!log.exists());
        assert!(
            run(&session, "horizon-shared-checkout-contract=1", "0", false)
                .status
                .success()
        );
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .starts_with("--shared\npanel\nshell\n")
        );
        session.worktree = "/workspace/agents/panel".into();
        assert!(run(&session, "", "0", false).status.success());
        assert!(fs::read_to_string(&log).unwrap().starts_with("panel\nshell\n"));
        fs::remove_file(&log).unwrap();
        for markers in [
            "",
            "horizon-tailnet-contract=1\n",
            "horizon-tailnet-contract=3\n",
            "horizon-tailnet-contract=1\nhorizon-tailnet-contract=3-suffix\n",
            "horizon-tailnet-contract=1\n horizon-tailnet-contract=3\n",
        ] {
            let refused = run(&session, markers, "0", true);
            assert_eq!(refused.status.code(), Some(3));
            assert!(
                !log.exists(),
                "an old selected-tailnet worker must not launch a session"
            );
        }
        let markers = "horizon-tailnet-contract=1\nhorizon-tailnet-contract=3\n";
        assert_eq!(run(&session, markers, "7", true).status.code(), Some(7));
        assert!(!log.exists());
        assert!(run(&session, markers, "0", true).status.success());
        assert!(fs::read_to_string(&log).unwrap().starts_with("panel\nshell\n"));
        session.worktree = super::super::siblings::shared_worktree(None);
        fs::remove_file(&log).unwrap();
        assert_eq!(run(&session, markers, "0", true).status.code(), Some(3));
        assert!(!log.exists());
        assert!(
            run(
                &session,
                &format!("{markers}horizon-shared-checkout-contract=1\n"),
                "0",
                true
            )
            .status
            .success()
        );
    }

    #[test]
    fn readiness_budget_interrupts_a_stalled_hostname_ssh_handshake() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let connection = Connection {
            host: "localhost".into(),
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
    fn dns_and_ip_destinations_keep_bound_identity_in_ssh_and_scp() {
        let root = tempfile::tempdir().unwrap();
        for host in ["worker.example.invalid", "192.0.2.1", "2001:db8::1"] {
            let worker: Worker = serde_json::from_value(serde_json::json!({
                "id":"worker1", "name":"fixture", "imageName":"fixture", "desiredStatus":"RUNNING",
                "sshHost":host, "portMappings":{"22":2222}
            }))
            .unwrap();
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "runpod_key_file":root.path().join("key"), "ssh_identity_file":root.path().join("identity"),
                "docker_config":root.path().join("docker"), "registry_pull_auth_id":null,
                "cpu_flavors":[], "gpu_types":[]
            }))
            .unwrap();
            let connection = Connection::new(&worker, &settings, root.path()).unwrap();
            assert_eq!(connection.host, host);
            let args = connection.args();
            assert!(args.contains(&format!("root@{host}")));
            assert!(args.contains(&"HostKeyAlias=horizon-cloud-worker1".into()));
            let scp = connection.upload_command(Path::new("source.pack"), "horizon-transfer.pack");
            let args: Vec<_> = scp.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
            let destination = if host.contains(':') {
                format!("[{host}]")
            } else {
                host.to_owned()
            };
            assert_eq!(
                args.last().unwrap(),
                &format!("root@{destination}:/workspace/horizon-transfer.pack")
            );
            assert!(args.contains(&"HostKeyAlias=horizon-cloud-worker1".into()));
            let pinned = connection.pinned_command("true");
            assert!(pinned.get_args().any(|arg| arg == "StrictHostKeyChecking=yes"));
        }
    }

    #[test]
    fn revisions_are_full_lowercase_commit_ids_as_the_worker_requires() {
        assert!(valid_revision(&"a".repeat(40)) && valid_revision(&"0".repeat(64)));
        for revision in ["A".repeat(40), "a".repeat(39), "g".repeat(40), String::new()] {
            assert!(!valid_revision(&revision), "{revision:?}");
        }
    }

    #[test]
    fn sibling_commands_and_uploads_stay_with_an_accepted_siblings_own_material() {
        let remote = SiblingRemote::new("native-lib").unwrap();
        assert_eq!(remote.stage, "horizon-worker-siblings stage native-lib");
        assert_eq!(
            remote
                .upload_path("/workspace/siblings/native-lib\n", "horizon-transfer.pack")
                .unwrap(),
            "siblings/native-lib/horizon-transfer.pack"
        );
        assert_eq!(
            remote
                .upload_path(
                    "/workspace/.horizon-tailnet/uploads/native-lib\n",
                    "horizon-transfer.pack"
                )
                .unwrap(),
            ".horizon-tailnet/uploads/native-lib/horizon-transfer.pack"
        );
        assert!(
            remote
                .upload_path("/workspace/.horizon-tailnet/uploads/other", "horizon-transfer.pack")
                .is_err()
        );
        assert_eq!(
            remote
                .upload_path("/workspace/siblings/native-lib", "horizon-source.tar")
                .unwrap(),
            "siblings/native-lib/horizon-source.tar"
        );
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
