//! Companion access for agent sessions that run as the unprivileged agent account.
//!
//! The grant's private key, host-key pin and alias stay root-only under the live
//! directory. For an isolated worker, a root-owned copy that only the agent group
//! can read is published under the agent directory, with a system SSH include
//! that only the agent account matches. Root's SSH never applies it and agents
//! cannot change it. This is trusted shell access, not credential isolation: an
//! agent can copy the key, and a copy works until the target revokes the grant.
use super::{Busy, Runtime, files, path_text, ssh};
use horizon_cloud_protocol::companion::{Access, Response};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    io,
    path::Path,
    process::Command,
};

/// The account that runs agent and shell sessions on an isolated worker.
pub(super) struct AgentAccount {
    pub(super) name: String,
    pub(super) group: u32,
}

const SHARED_FILE: u32 = 0o640;
const SYSTEM_FILE: u32 = 0o644;
const CONNECTION: &str = "connection.json";

impl Runtime {
    fn agent_group(&self) -> Option<u32> {
        self.agent_account.as_ref().map(|account| account.group)
    }

    /// Refreshes the agent copies, for a worker that became isolated later, and then
    /// publishes the discovery catalog where agent sessions can read it. If the
    /// copies cannot be reconciled, no catalog is left to claim agent-usable access.
    pub(super) fn publish_catalog_locked(&self, bytes: &[u8]) -> io::Result<()> {
        files::shared_directory(&self.agent, self.agent_group())?;
        let catalog = self.agent.join("catalog.json");
        if let Err(error) = self.publish_agent_access() {
            files::remove(&catalog)?;
            return Err(error);
        }
        if let Err(error) = files::write_with(&catalog, bytes, SHARED_FILE, self.agent_group()) {
            // The previous catalog can name access that reconciliation just withdrew.
            files::remove(&catalog)?;
            return Err(error);
        }
        // Older helpers kept the catalog in the root-only grant directory.
        files::remove(&self.live.join("companions/catalog.json"))
    }

    /// Makes the agent copy match the published root configuration. A grant that
    /// is no longer connected, or that cannot be copied, loses its copied key, pin
    /// and alias; one failure does not keep a withdrawn grant usable. The first
    /// failure is reported after the reconciliation.
    pub(super) fn publish_agent_access(&self) -> io::Result<()> {
        let group = self.agent_group();
        files::shared_directory(&self.agent, group)?;
        let mut first_error = None;
        let mut published = BTreeMap::new();
        if let Some(account) = &self.agent_account {
            let mut combined = String::new();
            for path in self.grant_configs()? {
                match self.publish_grant(&path) {
                    Ok((grant, config, files)) => {
                        combined.push_str(&config);
                        published.insert(grant, files);
                    }
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            // End the last Host block before the system configuration continues.
            combined.push_str("Host *\n");
            if let Err(error) = files::write_with(&self.agent.join("config"), combined.as_bytes(), SHARED_FILE, group) {
                // A stale combined file must not keep naming withdrawn grants.
                files::remove(&self.agent.join("config"))?;
                first_error.get_or_insert(error);
            }
            if let Err(error) = self.install_system_include(account) {
                first_error.get_or_insert(error);
            }
        } else {
            files::remove(&self.agent.join("config"))?;
        }
        // Withdraw copies only after the combined configuration stopped naming them.
        for entry in std::fs::read_dir(&self.agent)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            match published.get(entry.file_name().to_string_lossy().as_ref()) {
                None => files::remove_directory(&entry.path())?,
                // A replaced host-key pin stays until the new configuration replaced it.
                Some(kept) => {
                    for file in std::fs::read_dir(entry.path())? {
                        let file = file?;
                        if !kept.contains(file.file_name().to_string_lossy().as_ref()) {
                            files::remove(&file.path())?;
                        }
                    }
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Copies the files that one grant's configuration at `path` names and returns
    /// the grant, that configuration rewritten to the copied paths, and the names
    /// of the copied files.
    fn publish_grant(&self, path: &Path) -> io::Result<(String, String, BTreeSet<String>)> {
        let grant = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|grant| horizon_cloud::valid_id(grant))
            .ok_or_else(|| io::Error::other("Invalid companion grant"))?;
        let config = std::fs::read_to_string(path)?;
        let group = self.agent_group();
        let source = self.key_directory(grant);
        let destination = self.agent.join(grant);
        files::shared_directory(&destination, group)?;
        let mut kept = BTreeSet::from([CONNECTION.to_owned()]);
        let mut rewritten = String::new();
        for line in config.lines() {
            let setting = line.split_whitespace().collect::<Vec<_>>();
            match setting.as_slice() {
                [key, value]
                    if key.eq_ignore_ascii_case("IdentityFile") || key.eq_ignore_ascii_case("UserKnownHostsFile") =>
                {
                    let name = Path::new(value)
                        .strip_prefix(&source)
                        .ok()
                        .and_then(|name| name.to_str())
                        .filter(|name| !name.is_empty() && !name.contains('/'))
                        .ok_or_else(|| io::Error::other("Companion config names a file outside its grant"))?;
                    let copy = destination.join(name);
                    files::write_with(&copy, &std::fs::read(source.join(name))?, SHARED_FILE, group)?;
                    kept.insert(name.to_owned());
                    let _ = writeln!(rewritten, "  {key} {}", path_text(&copy)?);
                }
                _ => {
                    rewritten.push_str(line);
                    rewritten.push('\n');
                }
            }
        }
        match std::fs::read(source.join(CONNECTION)) {
            Ok(bytes) => files::write_with(&destination.join(CONNECTION), &bytes, SHARED_FILE, group)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => files::remove(&destination.join(CONNECTION))?,
            Err(error) => return Err(error),
        }
        Ok((grant.to_owned(), rewritten, kept))
    }

    /// OpenSSH reads the user file from the passwd home, which the agent account
    /// does not have, so a system include selects the agent copy for that account only.
    fn install_system_include(&self, account: &AgentAccount) -> io::Result<()> {
        let content = format!(
            "# Companion SSH aliases for agent sessions, maintained by horizon-cloud-worker.\nMatch localuser {}\n  Include {}/config\n",
            account.name,
            path_text(&self.agent)?
        );
        if std::fs::read_to_string(&self.system_include).ok().as_deref() == Some(content.as_str()) {
            return Ok(());
        }
        if let Some(parent) = self.system_include.parent() {
            std::fs::create_dir_all(parent)?;
        }
        files::write_with(&self.system_include, content.as_bytes(), SYSTEM_FILE, None)
    }

    /// Runs the readiness command as the agent account through the isolation
    /// launcher, so Ready means that agent sessions can use the alias too.
    pub(super) fn verify_agent_access(&self, ssh_alias: &str, command: &str) -> io::Result<()> {
        let (Some(launcher), Some(_)) = (&self.workspace_launcher, &self.agent_account) else {
            return Ok(());
        };
        let observed = ssh::checked(Command::new(launcher).args(["agent", "ssh", ssh_alias, command]))
            .map_err(|error| io::Error::new(error.kind(), "Companion SSH access failed for agent sessions"))?;
        if observed.trim() == "true" {
            Ok(())
        } else {
            Err(io::Error::other("Companion SSH access failed for agent sessions"))
        }
    }

    /// Agent sessions cannot take the companion lock. A connection record that
    /// changes during the probe means setup ran at the same time, so the probe
    /// reports busy, as a held lock does.
    pub(super) fn probe_published_access(
        &self,
        access: &Access,
        probe: impl FnOnce() -> io::Result<bool>,
    ) -> io::Result<bool> {
        if !horizon_cloud::valid_id(&access.grant) {
            return Err(io::Error::other("Invalid companion grant"));
        }
        let record = self.agent.join(&access.grant).join(CONNECTION);
        let before = std::fs::read(&record)?;
        let response: Response = serde_json::from_slice(&before)?;
        if response
            != (Response::Connected {
                ssh_alias: access.ssh_alias.clone(),
                worktree: access.worktree.clone(),
            })
        {
            return Ok(false);
        }
        let result = probe();
        if std::fs::read(&record).ok().as_deref() != Some(before.as_slice()) {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, Busy));
        }
        result
    }
}
