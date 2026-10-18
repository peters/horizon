//! Remote holds of this host on disk, and the answers to
//! `browser_remote_allocations`.
//!
//! Each host keeps its holds in its own folder and locks the folder's owner
//! file for as long as it runs. The operating system frees the lock when the
//! process ends, also after a kill, so the next host that starts on the same
//! runtime root takes over the holds of a host that did not release them and
//! asks the original provider about each exact session.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use horizon_browser::remote::{DeviceRequirement, RemoteTargetProfile};
use horizon_browser::{RemoteAllocation, RemoteAuthorizationHeader, RemoteRecoveryStatus, RemoteSessionRequest};
use horizon_browser_control::manifest::recovery::{self, RemoteAllocationSummary};
use horizon_browser_control::remote_slots::quota_key;
use serde::{Deserialize, Serialize};

use super::config::HostConfig;

/// Workspace id of the one workspace a standalone host has.
pub const WORKSPACE: &str = "standalone";
const OWNER_LOCK: &str = "owner.lock";
const HOLD: &str = "hold";
const JOURNAL: &str = "journal";
/// Released holds stay listed for the agent, up to this count.
const RELEASED_KEPT: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    provider: String,
    reference: String,
}

struct Held {
    id: String,
    provider: String,
    allocation: RemoteAllocation,
    /// Taken over from an earlier host rather than started by this one.
    adopted: bool,
    /// The order in which this host saw the release, for keeping the newest ones.
    released: Option<u64>,
}

pub struct Holds {
    own: PathBuf,
    _owner: File,
    host: String,
    actor: String,
    held: BTreeMap<String, Held>,
    releases: u64,
    /// Holds of an earlier host that could not be restored: no exact session
    /// identity, or the provider or its credentials are gone.
    orphans: BTreeMap<String, Record>,
}

impl Holds {
    /// Lock this host's folder under `root` and take over the holds of hosts
    /// under `root` that are no longer running.
    ///
    /// # Errors
    /// The folder cannot be created or locked.
    pub fn open(root: &Path, host: &str, actor: &str, config: &HostConfig) -> io::Result<Self> {
        let name = horizon_browser_control::paths::safe_local_id(host);
        let own = root.join(&name);
        // The folder gets its name only after its lock is held, so another host never takes
        // over a folder that is still starting.
        // Windows does not rename a folder with an open file, so there the folder starts
        // under its own name.
        let starting = if cfg!(unix) {
            root.join(format!(".{name}"))
        } else {
            own.clone()
        };
        create_private_dir(&starting)?;
        let owner = open_private(&starting.join(OWNER_LOCK))?;
        owner
            .try_lock()
            .map_err(|_| io::Error::other("remote hold folder is in use"))?;
        if starting != own {
            fs::rename(&starting, &own)?;
        }
        let mut holds = Self {
            own,
            _owner: owner,
            host: host.to_owned(),
            actor: actor.to_owned(),
            held: BTreeMap::new(),
            releases: 0,
            orphans: BTreeMap::new(),
        };
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let starting = path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
            if path != holds.own && !starting && path.is_dir() {
                holds.adopt(&path, config);
            }
        }
        holds.adopt_files(config);
        Ok(holds)
    }

    /// Record a hold before its driver starts, so a crash cannot lose it.
    ///
    /// # Errors
    /// The hold could not be saved; nothing was started.
    pub fn insert(&mut self, id: &str, request: &RemoteSessionRequest) -> io::Result<()> {
        let record = Record {
            provider: request.provider.clone(),
            reference: request.recovery.reference().to_owned(),
        };
        let hold = self.own.join(format!("{id}.{HOLD}"));
        let saved = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let file = options.open(&hold)?;
            serde_json::to_writer(&file, &record).map_err(io::Error::other)?;
            file.sync_all()?;
            request
                .recovery
                .retain_journal(&self.own.join(format!("{id}.{JOURNAL}")), request)
        })();
        if let Err(error) = saved {
            let _ = fs::remove_file(&hold);
            return Err(error);
        }
        request.recovery.record_admission(&self.host, &self.actor, WORKSPACE);
        self.held.insert(
            record.reference,
            Held {
                id: id.to_owned(),
                provider: record.provider,
                allocation: request.recovery.clone(),
                adopted: false,
                released: None,
            },
        );
        Ok(())
    }

    /// Forget a hold whose driver never started.
    pub fn cancel(&mut self, id: &str) {
        for held in self.held.values().filter(|held| held.id == id) {
            held.allocation.cancel_before_launch();
        }
        self.prune();
    }

    /// Whether any hold is not released.
    pub fn any_unreleased(&self) -> bool {
        !self.orphans.is_empty() || self.held.values().any(|held| !held.allocation.is_released())
    }

    /// Check at the provider each exact session of `ids` whose release the
    /// driver could not confirm. The check only reads; it never ends a session.
    pub fn check(&self, ids: &[String]) {
        for held in self.held.values().filter(|held| ids.contains(&held.id)) {
            if !held.allocation.is_released() {
                held.allocation.reconcile();
            }
        }
    }

    /// Whether a check at the provider is still running.
    pub fn checking(&self) -> bool {
        self.held
            .values()
            .any(|held| held.allocation.status() == RemoteRecoveryStatus::Reconciling)
    }

    /// The unreleased holds taken over from an earlier host, with their provider.
    pub fn adopted(&self) -> Vec<(String, RemoteAllocation)> {
        self.held
            .values()
            .filter(|held| held.adopted && !held.allocation.is_released())
            .map(|held| (held.provider.clone(), held.allocation.clone()))
            .collect()
    }

    /// Whether a session that this host started is not released.
    pub fn own_unreleased(&self) -> bool {
        self.held
            .values()
            .any(|held| !held.adopted && !held.allocation.is_released())
    }

    /// Remove the files of released holds and keep the newest released ones listed.
    pub fn prune(&mut self) {
        for held in self.held.values_mut() {
            if held.released.is_none() && held.allocation.is_released() {
                remove_hold(&self.own, &held.id);
                self.releases += 1;
                held.released = Some(self.releases);
            }
        }
        let mut released: Vec<_> = self
            .held
            .iter()
            .filter_map(|(reference, held)| Some((held.released?, reference.clone())))
            .collect();
        released.sort_unstable();
        let excess = released.len().saturating_sub(RELEASED_KEPT);
        for (_, reference) in released.drain(..excess) {
            self.held.remove(&reference);
        }
    }

    /// Answer the `browser_remote_allocations` requests of this host.
    pub fn answer(&mut self) {
        let Ok(requests) = recovery::claim_recovery_requests(&self.host) else {
            return;
        };
        for request in requests {
            let mine = request.actor == self.actor;
            let mut error = (!mine).then(|| "Allocation is not available to this agent".to_owned());
            if mine
                && let Some(reference) = &request.reference
                && !self
                    .held
                    .get(reference)
                    .is_some_and(|held| held.allocation.reconcile_for(&self.host, &self.actor, WORKSPACE, true))
            {
                error = Some("Allocation is not available to this agent".into());
            }
            let summaries = if mine { self.summaries() } else { Vec::new() };
            let _ = recovery::complete_recovery(&request.result(summaries, error));
        }
        self.prune();
    }

    fn summaries(&self) -> Vec<RemoteAllocationSummary> {
        let mut summaries: Vec<_> = self
            .held
            .iter()
            .filter_map(|(reference, held)| {
                let status = held.allocation.status_for(&self.host, &self.actor, WORKSPACE, true)?;
                Some(RemoteAllocationSummary {
                    reference: reference.clone(),
                    provider: held.provider.clone(),
                    status,
                    message: status.message().into(),
                })
            })
            .collect();
        summaries.extend(self.orphans.values().map(|record| {
            RemoteAllocationSummary {
                reference: record.reference.clone(),
                provider: record.provider.clone(),
                status: RemoteRecoveryStatus::IdentityUnavailable,
                message: "An earlier standalone host ended before it could release this session, and the session \
                      cannot be checked now. Verify it at the provider; the provider ends idle sessions itself."
                    .into(),
            }
        }));
        summaries
    }

    /// Move the holds of the stopped host in `dir` into this host's folder.
    fn adopt(&mut self, dir: &Path, config: &HostConfig) {
        let Ok(owner) = open_private(&dir.join(OWNER_LOCK)) else {
            return;
        };
        // A running host holds its lock.
        if owner.try_lock().is_err() {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == HOLD || extension == JOURNAL)
                && let Some(name) = path.file_name()
            {
                let _ = fs::rename(&path, self.own.join(name));
            }
        }
        let _ = fs::remove_file(dir.join(OWNER_LOCK));
        drop(owner);
        let _ = fs::remove_dir(dir);
        self.adopt_files(config);
    }

    /// Restore the holds in this host's folder that no live driver owns.
    fn adopt_files(&mut self, config: &HostConfig) {
        let Ok(entries) = fs::read_dir(&self.own) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != HOLD) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned) else {
                continue;
            };
            if self.held.values().any(|held| held.id == id) || self.orphans.contains_key(&id) {
                continue;
            }
            let Some(record) = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
            else {
                continue;
            };
            let journal = self.own.join(format!("{id}.{JOURNAL}"));
            // The journal is saved before a driver gets the hold, so without it no session
            // was ever requested.
            if !journal.exists() {
                remove_hold(&self.own, &id);
                continue;
            }
            match restore(&journal, &record, config) {
                Some(allocation) if allocation.reference() == record.reference => {
                    allocation.record_admission(&self.host, &self.actor, WORKSPACE);
                    allocation.reconcile();
                    self.held.insert(
                        record.reference,
                        Held {
                            id,
                            provider: record.provider,
                            allocation,
                            adopted: true,
                            released: None,
                        },
                    );
                }
                _ => {
                    self.orphans.insert(id, record);
                }
            }
        }
    }
}

impl Drop for Holds {
    /// A host that released everything leaves no folder behind.
    fn drop(&mut self) {
        if !self.any_unreleased() {
            self.prune();
            let _ = fs::remove_file(self.own.join(OWNER_LOCK));
            let _ = fs::remove_dir(&self.own);
        }
    }
}

/// The retired allocation in `journal`, bound to its original provider so its
/// exact session can be checked.
fn restore(journal: &Path, record: &Record, config: &HostConfig) -> Option<RemoteAllocation> {
    if let Ok(Some(released)) = RemoteAllocation::restore_released_journal(journal) {
        return Some(released);
    }
    let provider = config.provider(&record.provider).ok()?;
    let authorization = config
        .authorization(&record.provider)
        .ok()?
        .map(|header| RemoteAuthorizationHeader::new(header.as_str().to_owned()).map(Arc::new))
        .transpose()
        .ok()?;
    // Recovery uses only the provider's transport, never these target fields.
    let target = RemoteTargetProfile {
        provider: record.provider.clone(),
        browser_name: "chrome".into(),
        platform_name: "recovery".into(),
        device: DeviceRequirement::default(),
        orientation: None,
        capability_extensions: BTreeMap::new(),
    };
    let request = horizon_browser::remote_config::configured_remote_request(
        provider,
        &target,
        "recovery",
        authorization,
        quota_key(provider),
    )
    .ok()?;
    RemoteAllocation::restore_journal(journal, &request).ok()
}

fn remove_hold(dir: &Path, id: &str) {
    let _ = fs::remove_file(dir.join(format!("{id}.{JOURNAL}")));
    let _ = fs::remove_file(dir.join(format!("{id}.{HOLD}")));
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}
