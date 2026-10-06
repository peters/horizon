use super::{Runtime, files, path_text};
use base64::Engine as _;
use horizon_cloud_protocol::companion::Response;
use std::{
    fmt::Write as _,
    io::{self, Read},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) fn public_key(value: &str) -> io::Result<String> {
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.len() != 2 || fields[0] != "ssh-ed25519" {
        return Err(io::Error::other(
            "Expected an Ed25519 public key without options or comment",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(fields[1])
        .map_err(|_| io::Error::other("Invalid public key"))?;
    if bytes.len() != 51 || !bytes.starts_with(b"\0\0\0\x0bssh-ed25519\0\0\0\x20") {
        return Err(io::Error::other("Invalid Ed25519 public key"));
    }
    Ok(format!("ssh-ed25519 {}", fields[1]))
}

/// All invoked commands are noninteractive, local operations or bounded SSH probes.
pub(super) fn checked(command: &mut Command) -> io::Result<String> {
    checked_with_timeout(command, Duration::from_secs(30))
}

pub(super) fn checked_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = CommandGuard(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let output = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Missing command output"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.take(64 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            return Err(io::Error::other(
                "Companion command timed out; reconcile before retrying",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = receiver
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| io::Error::other("Companion output incomplete"))??;
    if !status.success() || output.len() > 64 * 1024 {
        return Err(io::Error::other("Companion command failed"));
    }
    String::from_utf8(output).map_err(|_| io::Error::other("Invalid companion output"))
}

/// Ends a probe configuration that `-F` selects, with the system file that `-F` skips.
pub(super) const SYSTEM_INCLUDE: &str = "Host *\nInclude /etc/ssh/ssh_config\n";

/// The root readiness probe and the agent checks together. The controller allows a
/// Connect request 45 seconds, which also covers its own SSH round trip to the source.
const CONNECT_PROBES: Duration = Duration::from_secs(35);

/// The published SSH configuration of one grant's alias.
pub(super) fn alias_config(
    ssh_alias: &str,
    host: &horizon_cloud::SshHost,
    port: u16,
    binding: &str,
    identity: &std::path::Path,
    known_hosts: &std::path::Path,
) -> io::Result<String> {
    Ok(format!(
        "Host {ssh_alias}\n  HostName {host}\n  Port {port}\n  User root\n  IdentityFile {}\n  IdentitiesOnly yes\n  StrictHostKeyChecking yes\n  HostKeyAlias {binding}\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n  BatchMode yes\n  RequestTTY auto\n  ConnectTimeout 10\n  ServerAliveInterval 5\n  ServerAliveCountMax 2\n  ForwardAgent no\n  ControlMaster no\n  ControlPath none\n",
        path_text(identity)?,
        path_text(known_hosts)?,
    ))
}

struct CommandGuard(Child);
impl Drop for CommandGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = rustix::process::Pid::from_raw(self.0.id().cast_signed()) {
            let _ = rustix::process::kill_process_group(id, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Runtime {
    pub(super) fn connect(
        &self,
        grant: &str,
        alias: &str,
        host: &horizon_cloud::SshHost,
        port: u16,
        host_key: &str,
    ) -> io::Result<Response> {
        if port == 0 || !horizon_cloud::companions::valid_alias(alias) {
            return Err(io::Error::other("Invalid companion address or alias"));
        }
        let host_key = public_key(host_key)?;
        let directory = self.key_directory(grant);
        let identity = directory.join("identity");
        if !identity.is_file() {
            return Err(io::Error::other("Prepare the source identity before connecting"));
        }
        let ssh_alias = format!("companion-{alias}");
        self.require_available_alias(grant, &ssh_alias)?;
        let binding = format!("horizon-companion-{grant}");
        // A failed probe must not replace the trust pin used by a published connection.
        let key_id = host_key.bytes().fold(String::new(), |mut encoded, byte| {
            let _ = write!(encoded, "{byte:02x}");
            encoded
        });
        let known_hosts = directory.join(format!("known_hosts-{key_id}"));
        files::write(&known_hosts, format!("{binding} {host_key}\n").as_bytes())?;
        let config = alias_config(&ssh_alias, host, port, &binding, &identity, &known_hosts)?;
        // Match the eventual user and system configuration before publishing this alias.
        let probe = directory.join("probe");
        files::write(
            &probe,
            self.preview_config(&directory.join("config"), &config)?.as_bytes(),
        )?;
        let worktree = self.worktree(grant);
        let command = format!("git -C {} rev-parse --is-inside-work-tree", path_text(&worktree)?);
        // The root probe and the agent checks share one budget inside the Connect deadline.
        let deadline = Instant::now() + CONNECT_PROBES;
        let observed = checked(Command::new("ssh").arg("-F").arg(&probe).arg(&ssh_alias).arg(&command))?;
        if observed.trim() != "true" {
            return Err(io::Error::other("Companion worktree readiness failed"));
        }
        let response = Response::Connected {
            ssh_alias: ssh_alias.clone(),
            worktree: path_text(&worktree)?.into(),
        };
        self.commit_connection(grant, &config, &response, deadline)?;
        Ok(response)
    }

    /// Publishes a probed connection. Agents get the alias and the key only after
    /// the connection record is written: their copies require that record, and
    /// their check before it reads a keyless staged `config`. A new grant gets its
    /// key copy last, after every check passed. A refresh of a connected grant
    /// keeps the published alias and key until the new record replaces it. If the
    /// refresh fails, the previous connection is restored; a new grant is withdrawn.
    pub(super) fn commit_connection(
        &self,
        grant: &str,
        config: &str,
        response: &Response,
        deadline: Instant,
    ) -> io::Result<()> {
        let Response::Connected { ssh_alias, .. } = response else {
            return Err(io::Error::other("Invalid companion connection"));
        };
        let directory = self.key_directory(grant);
        let previous = self.committed_connection(grant)?;
        let remaining = || deadline.saturating_duration_since(Instant::now());
        // A new grant never gave agents its key, so a failed check must not either.
        let keyless = previous.is_none().then_some(grant);
        let committed = self
            .verify_agent_route(grant, config, ssh_alias, remaining())
            .and_then(|()| files::write(&directory.join("config"), config.as_bytes()))
            .and_then(|()| files::write(&directory.join("connection.json"), &serde_json::to_vec(response)?))
            .and_then(|()| self.publish_config(keyless))
            .and_then(|()| self.verify_published_alias(grant, ssh_alias, remaining()))
            .and_then(|()| keyless.map_or(Ok(()), |grant| self.publish_agent_key(grant)));
        if let Err(error) = committed {
            // The original error explains the refusal; the recovery is best effort.
            let restored = previous.map_or(Err(io::ErrorKind::NotFound.into()), |(config, record)| {
                self.restore_connection(grant, &config, &record)
            });
            if restored.is_err() {
                let _ = self.withdraw(grant);
            }
            return Err(error);
        }
        Ok(())
    }

    /// The configuration and the record of a committed connection of `grant`.
    fn committed_connection(&self, grant: &str) -> io::Result<Option<(String, Vec<u8>)>> {
        let directory = self.key_directory(grant);
        let read = |name: &str| match std::fs::read(directory.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
        let (Some(config), Some(record)) = (read("config")?, read("connection.json")?) else {
            return Ok(None);
        };
        let config = String::from_utf8(config).map_err(|_| io::Error::other("Invalid companion config"))?;
        Ok(super::agent::committed(&config, &record).then_some((config, record)))
    }

    fn restore_connection(&self, grant: &str, config: &str, record: &[u8]) -> io::Result<()> {
        let directory = self.key_directory(grant);
        files::write(&directory.join("config"), config.as_bytes())?;
        files::write(&directory.join("connection.json"), record)?;
        self.update_config()
    }

    fn require_available_alias(&self, grant: &str, alias: &str) -> io::Result<()> {
        for entry in std::fs::read_dir(self.live.join("companions"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path() != self.key_directory(grant) {
                let path = entry.path().join("config");
                if path.exists()
                    && std::fs::read_to_string(path)?
                        .lines()
                        .next()
                        .is_some_and(|line| line.eq_ignore_ascii_case(&format!("Host {alias}")))
                {
                    return Err(io::Error::other("Companion SSH alias already bound"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn update_config(&self) -> io::Result<()> {
        self.publish_config(None)
    }

    /// Updates the root configuration and the agent copies; the `keyless` grant
    /// gets no key copy.
    fn publish_config(&self, keyless: Option<&str>) -> io::Result<()> {
        files::directory(&self.ssh_home)?;
        let root = self.live.join("companions");
        files::directory(&root)?;
        let include = format!("Include {}/config\n", path_text(&root)?);
        let path = self.ssh_home.join("config");
        let original = match std::fs::read_to_string(&path) {
            Ok(original) => original,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        if !original.starts_with(&include) {
            files::write(&path, format!("{include}{original}").as_bytes())?;
        }
        files::write(&root.join("config"), self.combined_config(None)?.as_bytes())?;
        self.publish_agent_access(keyless)
    }

    pub(super) fn preview_config(&self, path: &std::path::Path, candidate: &str) -> io::Result<String> {
        let root = self.live.join("companions");
        let include = format!("Include {}/config\n", path_text(&root)?);
        let original = match std::fs::read_to_string(self.ssh_home.join("config")) {
            Ok(original) => original,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let original = original.strip_prefix(&include).unwrap_or(&original);
        // -F suppresses the system file, so include it after the user file as ordinary SSH does.
        Ok(format!(
            "{}{}\n{SYSTEM_INCLUDE}",
            self.combined_config(Some((path, candidate)))?,
            original
        ))
    }

    /// The published configuration of each connected grant.
    pub(super) fn grant_configs(&self) -> io::Result<Vec<std::path::PathBuf>> {
        let mut configs = Vec::new();
        let entries = match std::fs::read_dir(self.live.join("companions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(configs),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("config").is_file() {
                configs.push(entry.path().join("config"));
            }
        }
        configs.sort();
        Ok(configs)
    }

    fn combined_config(&self, candidate: Option<(&std::path::Path, &str)>) -> io::Result<String> {
        let mut configs = self.grant_configs()?;
        if let Some((path, _)) = candidate
            && !configs.iter().any(|existing| existing == path)
        {
            configs.push(path.to_owned());
        }
        configs.sort();
        let mut combined = String::new();
        let mut aliases = std::collections::BTreeSet::new();
        for path in configs {
            let config = match candidate {
                Some((replacement, value)) if replacement == path => value.to_owned(),
                _ => std::fs::read_to_string(path)?,
            };
            let alias = config
                .lines()
                .next()
                .ok_or_else(|| io::Error::other("Invalid companion config"))?;
            if !aliases.insert(alias.to_ascii_lowercase()) {
                return Err(io::Error::other("Companion SSH alias already bound"));
            }
            combined.push_str(&config);
        }
        // End the last Host block before control returns to the user's original config.
        combined.push_str("Host *\n");
        Ok(combined)
    }
}
