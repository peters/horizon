use super::{Runtime, files, path_text};
use base64::Engine as _;
use horizon_cloud_protocol::companion::Response;
use std::{
    fmt::Write as _,
    io::{self, Read},
    net::IpAddr,
    process::{Command, Stdio},
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
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child
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
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "Companion command timed out; reconcile before retrying",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = receiver
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| io::Error::other("Companion output incomplete"))??;
    if !status.success() || output.len() > 64 * 1024 {
        return Err(io::Error::other("Companion command failed"));
    }
    String::from_utf8(output).map_err(|_| io::Error::other("Invalid companion output"))
}

impl Runtime {
    pub(super) fn connect(
        &self,
        grant: &str,
        alias: &str,
        host: IpAddr,
        port: u16,
        host_key: &str,
    ) -> io::Result<Response> {
        if port == 0 || alias.len() > 64 || !horizon_cloud::valid_id(alias) {
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
        let config = format!(
            "Host {ssh_alias}\n  HostName {host}\n  Port {port}\n  User root\n  IdentityFile {}\n  IdentitiesOnly yes\n  StrictHostKeyChecking yes\n  HostKeyAlias {binding}\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n  BatchMode yes\n  ConnectTimeout 10\n  ServerAliveInterval 5\n  ServerAliveCountMax 2\n  ForwardAgent no\n  ControlMaster no\n  ControlPath none\n",
            path_text(&identity)?,
            path_text(&known_hosts)?,
        );
        // Probe an isolated config before exposing the alias to interactive agents.
        let probe = directory.join("probe");
        files::write(&probe, config.as_bytes())?;
        let worktree = self.worktree(grant);
        let command = format!("git -C {} rev-parse --is-inside-work-tree", path_text(&worktree)?);
        let observed = checked(Command::new("ssh").arg("-F").arg(&probe).arg(&ssh_alias).arg(command))?;
        if observed.trim() != "true" {
            return Err(io::Error::other("Companion worktree readiness failed"));
        }
        files::write(&directory.join("config"), config.as_bytes())?;
        self.update_config()?;
        let response = Response::Connected {
            ssh_alias,
            worktree: path_text(&worktree)?.into(),
        };
        files::write(&directory.join("connection.json"), &serde_json::to_vec(&response)?)?;
        Ok(response)
    }

    fn require_available_alias(&self, grant: &str, alias: &str) -> io::Result<()> {
        for entry in std::fs::read_dir(self.live.join("companions"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path() != self.key_directory(grant) {
                let path = entry.path().join("config");
                if path.exists() && std::fs::read_to_string(path)?.lines().next() == Some(&format!("Host {alias}")) {
                    return Err(io::Error::other("Companion SSH alias already bound"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn update_config(&self) -> io::Result<()> {
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
        let mut configs = Vec::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("config").is_file() {
                configs.push(entry.path().join("config"));
            }
        }
        configs.sort();
        let mut combined = String::new();
        let mut aliases = std::collections::BTreeSet::new();
        for path in configs {
            let config = std::fs::read_to_string(path)?;
            let alias = config
                .lines()
                .next()
                .ok_or_else(|| io::Error::other("Invalid companion config"))?;
            if !aliases.insert(alias.to_owned()) {
                return Err(io::Error::other("Companion SSH alias already bound"));
            }
            combined.push_str(&config);
        }
        // End the last Host block before control returns to the user's original config.
        combined.push_str("Host *\n");
        files::write(&root.join("config"), combined.as_bytes())
    }
}
