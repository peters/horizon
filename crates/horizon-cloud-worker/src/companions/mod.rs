//! Owner-invoked grant setup over SSH. Private keys never leave the source worker.
mod agent;
mod files;
mod ssh;
// Worker grant fixtures use Unix paths and the OpenSSH tools shipped in Linux images.
#[cfg(all(test, unix))]
mod tests;

use horizon_cloud_protocol::companion::{Access, Catalog, Request, Response};
use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

struct Runtime {
    workspace: PathBuf,
    live: PathBuf,
    ssh_home: PathBuf,
    source_helper: PathBuf,
    workspace_launcher: Option<PathBuf>,
    /// Root-owned copies that the agent group can read: catalog, alias, key and pin.
    agent: PathBuf,
    /// The unprivileged account of an isolated worker. Without one, agents run as root.
    agent_account: Option<agent::AgentAccount>,
    /// The system SSH include that selects the agent copy for the agent account.
    system_include: PathBuf,
    /// Only root can take the companion lock and read the private grant directories.
    privileged: bool,
    /// How long an inspection waits for companion setup that holds the lock, as
    /// the owning Horizon's own refresh does for a few seconds.
    probe_wait: Duration,
}

/// The companion lock was held throughout; nothing about the target is known.
#[derive(Debug)]
pub(crate) struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Companion setup busy; retry")
    }
}

impl std::error::Error for Busy {}

/// Whether `error` is only the companion lock being held, never a probe failure.
pub(crate) fn is_busy(error: &io::Error) -> bool {
    matches!(error.get_ref(), Some(inner) if inner.is::<Busy>())
}

pub(super) fn run() -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().take(64 * 1024 + 1).read_to_end(&mut input)?;
    if input.len() > 64 * 1024 {
        return Err(io::Error::other("Companion request too large"));
    }
    let request: Request = serde_json::from_slice(&input).map_err(|_| io::Error::other("Invalid companion request"))?;
    let response = runtime().apply(&request)?;
    serde_json::to_writer(io::stdout().lock(), &response)?;
    io::stdout().write_all(b"\n")
}

/// Workspace mutations stay separate from privileged SSH grant publication.
pub(super) fn workspace() -> io::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(2).collect();
    match arguments.as_slice() {
        [action, grant, revision] if action == "prepare" && horizon_cloud::valid_id(grant) => {
            runtime().prepare_worktree(grant, revision)
        }
        [action, grant] if action == "remove" && horizon_cloud::valid_id(grant) => {
            runtime().remove_clean_worktree(grant)
        }
        _ => Err(io::Error::other("Invalid companion workspace request")),
    }
}

/// Agent sessions read the discovery catalog here; see [`Runtime::agent`].
pub(crate) const PUBLISHED: &str = "/run/horizon-companions";

fn runtime() -> Runtime {
    let isolated = Path::new("/run/horizon-tailnet/agent-isolation").exists();
    Runtime {
        workspace: "/workspace".into(),
        live: "/run/sshd".into(),
        // OpenSSH looks up the login's home in passwd, not the agent's HOME override.
        ssh_home: "/root/.ssh".into(),
        source_helper: "/usr/local/bin/horizon-worker-source".into(),
        workspace_launcher: isolated.then(|| "/usr/local/bin/horizon-worker-tailnet".into()),
        agent: PUBLISHED.into(),
        // The isolation launcher runs sessions with exactly this account and group.
        agent_account: isolated.then(|| agent::AgentAccount {
            name: "horizon-agent".into(),
            group: 10001,
        }),
        system_include: "/etc/ssh/ssh_config.d/horizon-companions.conf".into(),
        #[cfg(unix)]
        privileged: rustix::process::geteuid().is_root(),
        #[cfg(not(unix))]
        privileged: true,
        probe_wait: Duration::from_secs(10),
    }
}

pub(crate) fn publish_catalog(catalog: &Catalog) -> io::Result<()> {
    catalog.validate().map_err(io::Error::other)?;
    let runtime = runtime();
    runtime.with_lock(|| runtime.publish_catalog_locked(&serde_json::to_vec(catalog)?))
}

pub(crate) fn probe_access(access: &Access) -> io::Result<bool> {
    runtime().probe_access(access, || {
        let output = ssh::checked(
            std::process::Command::new("ssh")
                .arg(&access.ssh_alias)
                .arg(format!("git -C {} rev-parse --is-inside-work-tree", access.worktree)),
        )?;
        Ok(output.trim() == "true")
    })
}

impl Runtime {
    /// Probes under the companion lock, waiting up to `probe_wait` for setup that holds
    /// it. A lock that stays held fails with `WouldBlock`, which says nothing about SSH.
    fn probe_access(&self, access: &Access, probe: impl FnOnce() -> io::Result<bool>) -> io::Result<bool> {
        if !self.privileged {
            return self.probe_published_access(access, probe);
        }
        self.with_lock_within(self.probe_wait, || {
            let directory = self.key_directory(&access.grant);
            let response: Response = serde_json::from_slice(&std::fs::read(directory.join("connection.json"))?)?;
            if response
                != (Response::Connected {
                    ssh_alias: access.ssh_alias.clone(),
                    worktree: access.worktree.clone(),
                })
            {
                return Ok(false);
            }
            probe()
        })
    }

    fn apply(&self, request: &Request) -> io::Result<Response> {
        if !horizon_cloud::valid_id(request.grant()) {
            return Err(io::Error::other("Invalid companion grant"));
        }
        self.with_lock(|| self.apply_locked(request))
    }

    fn with_lock<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        self.with_lock_within(Duration::ZERO, operation)
    }

    fn with_lock_within<T>(&self, wait: Duration, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        std::fs::create_dir_all(&self.live)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.live.join("companion.lock"))?;
        let deadline = std::time::Instant::now() + wait;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                // Only contention is waited out; a locking failure is reported as itself.
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(io::Error::new(io::ErrorKind::WouldBlock, Busy)),
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
        }
        let result = operation();
        let unlock = lock.unlock();
        let response = result?;
        unlock?;
        Ok(response)
    }

    fn apply_locked(&self, request: &Request) -> io::Result<Response> {
        let grant = request.grant();
        match request {
            Request::Identity { .. } => self.identity(grant),
            Request::Authorize {
                public_key, revision, ..
            } => self.authorize(grant, public_key, revision),
            Request::Connect {
                alias,
                host,
                port,
                host_key,
                ..
            } => self.connect(grant, alias, host, *port, host_key),
            Request::Revoke { .. } => {
                self.authorized_key(grant, None)?;
                // The key is gone, which is what revocation needs; a worktree that
                // cannot be checked is kept rather than failing the revocation.
                let _ = self.workspace_action("remove", grant, None);
                Ok(Response::Revoked)
            }
            Request::Forget { .. } => self.forget(grant),
            Request::Disconnect { .. } => {
                self.withdraw(grant)?;
                Ok(Response::Disconnected)
            }
        }
    }

    /// Removes the grant's worktree and its prepared record when nothing in it would
    /// be lost: no changes and no untracked or ignored files. A dirty worktree is kept.
    fn remove_clean_worktree(&self, grant: &str) -> io::Result<()> {
        let worktree = self.worktree(grant);
        let prepared = self.workspace.join("companions/prepared").join(grant);
        if worktree.exists() {
            let status = ssh::checked(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&worktree)
                    .args(["status", "--porcelain", "--ignored"])
                    .env("HOME", self.workspace.join("home")),
            )?;
            if !status.trim().is_empty() {
                return Ok(());
            }
            ssh::checked(
                std::process::Command::new("git")
                    .arg(format!("--git-dir={}", self.workspace.join("repository.git").display()))
                    .args(["worktree", "remove"])
                    .arg(&worktree)
                    .env("HOME", self.workspace.join("home")),
            )?;
        }
        files::remove(&prepared)
    }

    /// Removes the grant's alias for root and agent sessions. The identity stays
    /// until the caller confirms target revocation.
    fn withdraw(&self, grant: &str) -> io::Result<()> {
        // The agent copy goes first, so a failed reconciliation cannot keep it.
        files::remove_directory(&self.agent.join(grant))?;
        let directory = self.key_directory(grant);
        if directory.exists() {
            files::remove(&directory.join("config"))?;
            files::remove(&directory.join("connection.json"))?;
        }
        self.update_config()
    }

    /// Drops a disconnected grant's key directory once the target revoked the key.
    fn forget(&self, grant: &str) -> io::Result<Response> {
        let directory = self.key_directory(grant);
        if directory.join("config").exists() || directory.join("connection.json").exists() {
            return Err(io::Error::other("Disconnect the companion before forgetting its key"));
        }
        // Disconnect already withdrew the agent copy; this also clears an interrupted one.
        files::remove_directory(&self.agent.join(grant))?;
        files::remove_directory(&directory)?;
        Ok(Response::Forgotten)
    }

    fn key_directory(&self, grant: &str) -> PathBuf {
        self.live.join("companions").join(grant)
    }

    fn worktree(&self, grant: &str) -> PathBuf {
        self.workspace.join("companions/worktrees").join(grant)
    }

    fn authorized_key(&self, grant: &str, key: Option<&str>) -> io::Result<()> {
        let path = self.live.join("horizon-authorized-keys");
        let original = std::fs::read_to_string(&path)?;
        let marker = format!("horizon-companion:{grant}");
        let prefix = format!("restrict,pty,command=\"/usr/local/bin/horizon-worker-run {grant} companion\" ");
        let mut lines: Vec<_> = original
            .lines()
            .filter(|line| !(line.starts_with(&prefix) && line.split_whitespace().last() == Some(&marker)))
            .collect();
        let added = key.map(|key| format!("{prefix}{key} {marker}"));
        if let Some(added) = &added {
            lines.push(added);
        }
        files::write(&path, format!("{}\n", lines.join("\n")).as_bytes())
    }

    fn identity(&self, grant: &str) -> io::Result<Response> {
        let directory = self.key_directory(grant);
        files::directory(&directory)?;
        let key = directory.join("identity");
        if !key.exists() {
            ssh::checked(
                std::process::Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
                    .arg(&key),
            )?;
        }
        let public_key = ssh::checked(std::process::Command::new("ssh-keygen").args(["-y", "-f"]).arg(&key))?;
        Ok(Response::Identity {
            public_key: ssh::public_key(&public_key)?,
        })
    }

    fn prepare_worktree(&self, grant: &str, revision: &str) -> io::Result<()> {
        if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::other("Invalid companion source revision"));
        }
        let worktree = self.worktree(grant);
        let prepared = self.workspace.join("companions/prepared").join(grant);
        let parent = worktree.parent().ok_or_else(|| io::Error::other("Invalid worktree"))?;
        files::directory(parent)?;
        if !worktree.exists() {
            let checkout_timeout = Duration::from_secs(300);
            ssh::checked_with_timeout(
                std::process::Command::new("git")
                    .arg(format!("--git-dir={}", self.workspace.join("repository.git").display()))
                    .args(["worktree", "add", "--detach"])
                    .arg(&worktree)
                    .arg(revision)
                    .env("HOME", self.workspace.join("home")),
                checkout_timeout,
            )?;
            ssh::checked_with_timeout(
                std::process::Command::new(&self.source_helper)
                    .arg("checkout")
                    .arg(&worktree)
                    .env("HOME", self.workspace.join("home")),
                checkout_timeout,
            )?;
            files::directory(&self.workspace.join("companions/prepared"))?;
            files::write(&prepared, revision.as_bytes())?;
        } else if std::fs::read_to_string(&prepared).ok().as_deref() != Some(revision) {
            return Err(io::Error::other(
                "Companion checkout is incomplete or its initial revision changed; existing work was preserved",
            ));
        }
        // Retrying a grant never resets existing work, including a dirty worktree.
        ssh::checked(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&worktree)
                .args(["rev-parse", "--show-toplevel"]),
        )?;
        Ok(())
    }

    fn workspace_action(&self, action: &str, grant: &str, revision: Option<&str>) -> io::Result<()> {
        if let Some(launcher) = &self.workspace_launcher {
            let mut command = std::process::Command::new(launcher);
            command
                .args(["agent"])
                .arg(std::env::current_exe()?)
                .args(["companion-workspace", action, grant]);
            if let Some(revision) = revision {
                command.arg(revision);
            }
            ssh::checked_with_timeout(&mut command, Duration::from_secs(300))?;
            Ok(())
        } else if let Some(revision) = revision {
            self.prepare_worktree(grant, revision)
        } else {
            self.remove_clean_worktree(grant)
        }
    }

    fn authorize(&self, grant: &str, public_key: &str, revision: &str) -> io::Result<Response> {
        let public_key = ssh::public_key(public_key)?;
        self.workspace_action("prepare", grant, Some(revision))?;
        let worktree = self.worktree(grant);
        let host_key = ssh::checked(
            std::process::Command::new("ssh-keygen")
                .args(["-y", "-f"])
                .arg(self.live.join("horizon-host-keys/ed25519")),
        )?;
        let host_key = host_key.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        self.authorized_key(grant, Some(&public_key))?;
        Ok(Response::Authorized {
            host_key: ssh::public_key(&host_key)?,
            worktree: worktree.to_string_lossy().into_owned(),
        })
    }
}

fn path_text(path: &Path) -> io::Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid companion path"))?;
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || b"/._-".contains(&byte)))
    {
        return Err(io::Error::other("Unsafe companion path"));
    }
    Ok(value)
}
