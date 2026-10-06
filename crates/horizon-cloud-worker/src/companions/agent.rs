//! Companion access for agent sessions that run as the unprivileged agent account.
//!
//! The grant's private key, host-key pin and alias stay root-only under the live
//! directory. For an isolated worker, a root-owned copy that only the agent group
//! can read is published under the agent directory, with a system SSH include
//! that only the agent account matches. Root's SSH never applies it and agents
//! cannot change it. Only a grant with a committed connection record is published.
//! This is trusted shell access, not credential isolation: an agent can copy the
//! key, and a copy works until the target revokes the grant.
use super::{Busy, Runtime, files, path_text, ssh};
use horizon_cloud_protocol::companion::{Access, Response};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
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

    /// The keyless configuration that the agent check of an unfinished Connect
    /// uses. A grant ID has no `.`, so reconciliation never publishes this
    /// directory and removes one that an interruption left.
    pub(super) fn staged_probe(&self, grant: &str) -> PathBuf {
        self.agent.join(format!("{grant}.probe"))
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

    /// Makes the agent copy match the connected grants. A grant without a valid
    /// connection record, or that cannot be copied, loses its copied key, pin and
    /// alias; one failure does not keep a withdrawn grant usable. The first
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
                    Ok(Some((grant, config, files))) => {
                        combined.push_str(&config);
                        published.insert(grant, files);
                    }
                    Ok(None) => {}
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
    /// of the copied files. A grant whose Connect did not write a connection
    /// record for this alias is not published.
    fn publish_grant(&self, path: &Path) -> io::Result<Option<(String, String, BTreeSet<String>)>> {
        let grant = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|grant| horizon_cloud::valid_id(grant))
            .ok_or_else(|| io::Error::other("Invalid companion grant"))?;
        let config = std::fs::read_to_string(path)?;
        let record = match std::fs::read(self.key_directory(grant).join(CONNECTION)) {
            Ok(record) => record,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !committed(&config, &record) {
            return Ok(None);
        }
        let destination = self.agent.join(grant);
        let (rewritten, mut kept) = self.copy_grant(grant, &config, &destination)?;
        files::write_with(&destination.join(CONNECTION), &record, SHARED_FILE, self.agent_group())?;
        kept.insert(CONNECTION.to_owned());
        Ok(Some((grant.to_owned(), rewritten, kept)))
    }

    /// Copies the key and the pin that `config` names into `destination` and
    /// returns `config` rewritten to the copies and the names of the copies.
    fn copy_grant(&self, grant: &str, config: &str, destination: &Path) -> io::Result<(String, BTreeSet<String>)> {
        let group = self.agent_group();
        let source = self.key_directory(grant);
        let (rewritten, names) = rewrite_paths(&source, config, destination)?;
        files::shared_directory(destination, group)?;
        for name in &names {
            files::write_with(
                &destination.join(name),
                &std::fs::read(source.join(name))?,
                SHARED_FILE,
                group,
            )?;
        }
        Ok((rewritten, names))
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

    /// Resolves the candidate `config` as the agent account before Connect commits
    /// it, so Ready means that agent sessions can use the alias too. Root already
    /// probed the connection. The staged configuration names the eventual copies
    /// but holds no key or pin, so an interrupted Connect gives agents no key.
    /// `timeout` is what remains of the Connect budget after the root probe.
    pub(super) fn verify_agent_route(
        &self,
        grant: &str,
        config: &str,
        ssh_alias: &str,
        timeout: Duration,
    ) -> io::Result<()> {
        if self.workspace_launcher.is_none() || self.agent_account.is_none() {
            return Ok(());
        }
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Companion SSH access for agent sessions was not checked in time",
            ));
        }
        let staged = self.staged_probe(grant);
        let resolved = self.stage_route(grant, config, &staged).and_then(|probe| {
            let probe = path_text(&probe)?;
            self.resolve_as_agent(grant, &["-F", probe, ssh_alias], timeout)
        });
        let removed = files::remove_directory(&staged);
        resolved?;
        removed
    }

    /// Resolves the published alias as the agent account, without a connection,
    /// so Ready also covers the system include that agent sessions use.
    pub(super) fn verify_published_alias(&self, grant: &str, ssh_alias: &str, timeout: Duration) -> io::Result<()> {
        if self.workspace_launcher.is_none() || self.agent_account.is_none() {
            return Ok(());
        }
        self.resolve_as_agent(grant, &[ssh_alias], timeout)
    }

    /// Runs `ssh -G` with `arguments` as the agent account and requires the
    /// published key copy of `grant`. `ssh -G` reads no key and opens no connection.
    fn resolve_as_agent(&self, grant: &str, arguments: &[&str], timeout: Duration) -> io::Result<()> {
        let Some(launcher) = &self.workspace_launcher else {
            return Ok(());
        };
        let expected = format!("identityfile {}", path_text(&self.agent.join(grant).join("identity"))?);
        let failed = || io::Error::other("Companion SSH alias does not resolve for agent sessions");
        let resolved = ssh::checked_with_timeout(
            Command::new(launcher).args(["agent", "ssh", "-G"]).args(arguments),
            timeout,
        )
        .map_err(|error| io::Error::new(error.kind(), failed().to_string()))?;
        if resolved.lines().any(|line| line == expected) {
            Ok(())
        } else {
            Err(failed())
        }
    }

    /// Writes the candidate `config` of `grant` to `staged`, with the paths of
    /// the eventual copies, and returns its path. No key or pin is copied.
    fn stage_route(&self, grant: &str, config: &str, staged: &Path) -> io::Result<PathBuf> {
        let group = self.agent_group();
        let (rewritten, _) = rewrite_paths(&self.key_directory(grant), config, &self.agent.join(grant))?;
        files::shared_directory(&self.agent, group)?;
        files::remove_directory(staged)?;
        files::shared_directory(staged, group)?;
        let probe = staged.join("config");
        files::write_with(
            &probe,
            format!("{rewritten}{}", ssh::SYSTEM_INCLUDE).as_bytes(),
            SHARED_FILE,
            group,
        )?;
        Ok(probe)
    }

    /// Agent sessions cannot take the companion lock. A change during the probe
    /// to the published connection record, the combined configuration or the
    /// copied key and pin means that setup ran at the same time, so the probe
    /// reports busy, as a held lock does. A refresh can replace the route and
    /// the pin and keep the same record.
    pub(super) fn probe_published_access(
        &self,
        access: &Access,
        probe: impl FnOnce() -> io::Result<bool>,
    ) -> io::Result<bool> {
        if !horizon_cloud::valid_id(&access.grant) {
            return Err(io::Error::other("Invalid companion grant"));
        }
        let before = self.published_state(&access.grant)?;
        let record = before
            .get(&self.agent.join(&access.grant).join(CONNECTION))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Companion grant is not connected"))?;
        let response: Response = serde_json::from_slice(record)?;
        if response
            != (Response::Connected {
                ssh_alias: access.ssh_alias.clone(),
                worktree: access.worktree.clone(),
            })
        {
            return Ok(false);
        }
        let result = probe();
        if self.published_state(&access.grant).ok().as_ref() != Some(&before) {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, Busy));
        }
        result
    }

    /// The combined agent configuration and the published copies of `grant`.
    /// Files that a publication writes or removes at the same time are left out:
    /// a completed change also changes the configuration or a copy.
    fn published_state(&self, grant: &str) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
        let mut state = BTreeMap::new();
        let config = self.agent.join("config");
        state.insert(config.clone(), std::fs::read(config)?);
        for entry in std::fs::read_dir(self.agent.join(grant))? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "pending") {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => {
                    state.insert(path, bytes);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(state)
    }
}

/// Whether `record` is a `Connected` record for the alias that `config` names.
pub(super) fn committed(config: &str, record: &[u8]) -> bool {
    match serde_json::from_slice(record) {
        Ok(Response::Connected { ssh_alias, .. }) => config.lines().next() == Some(&format!("Host {ssh_alias}")),
        _ => false,
    }
}

/// Rewrites the key and pin paths under `source` that `config` names to the
/// same names under `destination`, and returns the result and those names.
fn rewrite_paths(source: &Path, config: &str, destination: &Path) -> io::Result<(String, BTreeSet<String>)> {
    let mut names = BTreeSet::new();
    let mut rewritten = String::new();
    for line in config.lines() {
        let setting = line.split_whitespace().collect::<Vec<_>>();
        match setting.as_slice() {
            [key, value]
                if key.eq_ignore_ascii_case("IdentityFile") || key.eq_ignore_ascii_case("UserKnownHostsFile") =>
            {
                let name = Path::new(value)
                    .strip_prefix(source)
                    .ok()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty() && !name.contains('/'))
                    .ok_or_else(|| io::Error::other("Companion config names a file outside its grant"))?;
                names.insert(name.to_owned());
                let _ = writeln!(rewritten, "  {key} {}", path_text(&destination.join(name))?);
            }
            _ => {
                rewritten.push_str(line);
                rewritten.push('\n');
            }
        }
    }
    Ok((rewritten, names))
}
