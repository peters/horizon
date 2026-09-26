//! Owner-invoked grant setup over SSH. Private keys never leave the source worker.
pub(crate) mod files;
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

fn runtime() -> Runtime {
    Runtime {
        workspace: "/workspace".into(),
        live: "/run/sshd".into(),
        // OpenSSH looks up the login's home in passwd, not the agent's HOME override.
        ssh_home: "/root/.ssh".into(),
        source_helper: "/usr/local/bin/horizon-worker-source".into(),
    }
}

pub(crate) fn publish_catalog(catalog: &Catalog) -> io::Result<()> {
    catalog.validate().map_err(io::Error::other)?;
    let runtime = runtime();
    runtime.with_lock(|| {
        let root = runtime.live.join("companions");
        files::directory(&root)?;
        files::write(&root.join("catalog.json"), &serde_json::to_vec(catalog)?)
    })
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
    fn probe_access(&self, access: &Access, probe: impl FnOnce() -> io::Result<bool>) -> io::Result<bool> {
        self.with_lock(|| {
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
        std::fs::create_dir_all(&self.live)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.live.join("companion.lock"))?;
        lock.try_lock()
            .map_err(|_| io::Error::other("Companion setup busy; retry"))?;
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
            } => self.connect(grant, alias, *host, *port, host_key),
            Request::Revoke { .. } => {
                self.authorized_key(grant, None)?;
                Ok(Response::Revoked)
            }
            Request::Disconnect { .. } => {
                let directory = self.key_directory(grant);
                if directory.exists() {
                    // Keep the identity until target revocation is confirmed by the caller.
                    files::remove(&directory.join("config"))?;
                    files::remove(&directory.join("connection.json"))?;
                }
                self.update_config()?;
                Ok(Response::Disconnected)
            }
        }
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

    fn authorize(&self, grant: &str, public_key: &str, revision: &str) -> io::Result<Response> {
        let public_key = ssh::public_key(public_key)?;
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
