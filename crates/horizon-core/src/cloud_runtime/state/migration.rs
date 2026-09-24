//! Opt-in local migration. Runtime callers must separately establish provider authority.
mod companions;
#[cfg(all(test, unix))]
mod tests;

use super::{
    Error, Result,
    transaction::{self, LockedPair},
};
use crate::cloud_runtime::allocation::{AllocationId, ControllerId, ProjectId, ProjectIdentity, legacy::Records};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const INTENT: &str = "migration.json";
const DEPLOYMENT: &str = "deployment.json";
const BACKUP: &str = "legacy-deployment.backup.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    root: PathBuf,
    identity: ProjectIdentity,
    allocation: AllocationId,
    controller: ControllerId,
    complete: bool,
    legacy: Vec<u8>,
    companions: BTreeMap<String, Vec<u8>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Barrier {
    version: u32,
    migrating: Intent,
}

/// Holds the allocation lock before the original project lock. It grants no
/// permission to mutate a provider or use credentials. Legacy runtime paths stay put.
pub struct MigratedStore {
    pair: LockedPair,
    _registry_lock: transaction::OperationLock,
    retained_companions: BTreeMap<String, Vec<u8>>,
}

impl MigratedStore {
    /// Convert or reconcile a dedicated deployment using authenticated saved-session
    /// context and a durably assigned local controller identity. No remote I/O occurs.
    /// Ordinary deployment entry points do not invoke this opt-in API yet.
    ///
    /// # Errors
    /// Rejects conflicting identity, corrupt/missing fences and unsupported hosts.
    pub fn migrate(root: &Path, session: &str, workspace: &str, controller: ControllerId) -> Result<Self> {
        migrate_with(root, session, workspace, controller, &mut |_| Ok(()))
    }

    /// # Errors
    /// Rejects missing, changed or conflicting journal projections; reconciles only
    /// a durable transaction whose files match its exact old or intended new bytes.
    pub fn load(&mut self) -> Result<Records> {
        let records = self.pair.load()?;
        companions::validate_retained(self.root(), &records.deployment(), &self.retained_companions)?;
        Ok(records)
    }

    /// # Errors
    /// Refuses ownership changes and incomplete transactions. Provider verification
    /// and credential pinning remain the responsibility of the future runtime adapter.
    pub fn save(&mut self, records: &Records) -> Result<()> {
        let previous = self.load()?;
        companions::validate_update(self.root(), &previous.deployment(), &records.deployment())?;
        self.pair.save(records)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.pair.root()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    Discovered,
    Refreshed,
    Barrier,
    Backup,
    Journal(transaction::Boundary),
    Complete,
}

fn migrate_with(
    root: &Path,
    session: &str,
    workspace: &str,
    controller: ControllerId,
    checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
) -> Result<MigratedStore> {
    crate::session_store::require_directory_durability()?;
    let root = root.canonicalize()?;
    let cloud = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::Invalid("Invalid cloud directory"))?;
    let discovered = discover(&root, session, workspace, cloud, controller)?;
    checkpoint(Boundary::Discovered)?;
    let parent = root.parent().ok_or(Error::Invalid("Missing cloud parent"))?;
    let registry_lock = transaction::lock_named(parent, ".migration.lock")?;
    let mut pair = LockedPair::acquire(&root, discovered.allocation, discovered.identity.clone(), controller)?;
    let mut intent = read_intent(&root)?.ok_or(Error::Invalid("Missing migration intent"))?;
    validate_context(&intent, &root, session, workspace, cloud, controller)?;
    if intent.allocation != discovered.allocation || intent.identity != discovered.identity {
        return Err(Error::Invalid("Migration identity changed during lock acquisition"));
    }
    let retained_companions = intent.companions.clone();
    if intent.complete {
        let records = pair.load()?;
        verify_records(&intent, &records)?;
        companions::validate_retained(&root, &records.deployment(), &retained_companions)?;
        return Ok(MigratedStore {
            pair,
            _registry_lock: registry_lock,
            retained_companions: intent.companions.clone(),
        });
    }
    let current = required(&root.join(DEPLOYMENT))?;
    if Records::from_legacy(&current, intent.identity.clone(), intent.allocation, controller).is_ok() {
        if !pair.is_uninitialized()? {
            return Err(Error::Invalid("Legacy state reappeared after allocation publication"));
        }
        // An old controller may have updated v1 after discovery. Refresh *all* state
        // under the old lock before installing the old-reader rejection barrier.
        refresh(&mut intent)?;
        companions::reject_duplicate_workers(&root, &records(&intent)?.deployment())?;
        write_intent(&intent)?;
        checkpoint(Boundary::Refreshed)?;
        transaction::write_atomic(&root, DEPLOYMENT, &barrier(&intent)?)?;
        checkpoint(Boundary::Barrier)?;
    } else {
        let marker = barrier(&intent)?;
        let project = records(&intent)?.project_bytes().map_err(|_| Error::Json)?;
        if current != marker && current != project {
            return Err(Error::Invalid("Migration deployment differs from its intent"));
        }
    }
    let original = records(&intent)?;
    if companions::capture(&root, &original.deployment())? != intent.companions {
        return Err(Error::Invalid("Migration companion journals changed after the barrier"));
    }
    match transaction::read_optional(&root.join(BACKUP))? {
        Some(backup) if backup != intent.legacy => return Err(Error::Invalid("Legacy diagnostic backup differs")),
        Some(_) => {}
        None => transaction::write_atomic(&root, BACKUP, &intent.legacy)?,
    }
    checkpoint(Boundary::Backup)?;
    pair.initialize(&original, &barrier(&intent)?, &mut |at| {
        checkpoint(Boundary::Journal(at))
    })?;
    intent.complete = true;
    write_intent(&intent)?;
    checkpoint(Boundary::Complete)?;
    Ok(MigratedStore {
        pair,
        _registry_lock: registry_lock,
        retained_companions: intent.companions.clone(),
    })
}

fn discover(root: &Path, session: &str, workspace: &str, cloud: &str, controller: ControllerId) -> Result<Intent> {
    // Release the old lock before acquiring allocation then project locks.
    let _project_lock = transaction::lock_file(root)?;
    if let Some(intent) = read_intent(root)? {
        validate_context(&intent, root, session, workspace, cloud, controller)?;
        return Ok(intent);
    }
    let identity = ProjectIdentity::new(ProjectId::generate(), session.into(), workspace.into(), cloud.into())
        .map_err(|_| Error::Invalid("Invalid migration membership"))?;
    let mut intent = Intent {
        version: 1,
        root: root.into(),
        identity,
        allocation: AllocationId::generate(),
        controller,
        complete: false,
        legacy: Vec::new(),
        companions: BTreeMap::new(),
    };
    refresh(&mut intent)?;
    write_intent(&intent)?;
    Ok(intent)
}

fn validate_context(
    intent: &Intent,
    root: &Path,
    session: &str,
    workspace: &str,
    cloud: &str,
    controller: ControllerId,
) -> Result<()> {
    if intent.version != 1
        || intent.root != root
        || intent.controller != controller
        || !intent.identity.belongs_to(session, workspace, cloud)
    {
        return Err(Error::Invalid("Migration context or version differs"));
    }
    records(intent)?;
    Ok(())
}

fn verify_records(intent: &Intent, records: &Records) -> Result<()> {
    if records.allocation_id() != intent.allocation
        || records.identity() != &intent.identity
        || records.controller_id() != intent.controller
    {
        return Err(Error::Invalid("Migrated journal ownership differs"));
    }
    Ok(())
}

fn refresh(intent: &mut Intent) -> Result<()> {
    intent.legacy = required(&intent.root.join(DEPLOYMENT))?;
    let records = records(intent)?;
    intent.companions = companions::capture(&intent.root, &records.deployment())?;
    Ok(())
}

fn records(intent: &Intent) -> Result<Records> {
    Records::from_legacy(
        &intent.legacy,
        intent.identity.clone(),
        intent.allocation,
        intent.controller,
    )
    .map_err(|_| Error::Json)
}

fn barrier(intent: &Intent) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(&Barrier {
        version: 2,
        migrating: intent.clone(),
    })
    .map_err(|_| Error::Json)
}

fn required(path: &Path) -> Result<Vec<u8>> {
    transaction::read_optional(path)?.ok_or(Error::Invalid("Missing migration journal"))
}

fn read_intent(root: &Path) -> Result<Option<Intent>> {
    let Some(bytes) = transaction::read_optional(&root.join(INTENT))? else {
        return Ok(None);
    };
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if intent.version != 1 || intent.root != root {
        return Err(Error::Invalid("Migration journal location or version differs"));
    }
    Ok(Some(intent))
}

fn write_intent(intent: &Intent) -> Result<()> {
    transaction::write_atomic(
        &intent.root,
        INTENT,
        &serde_json::to_vec_pretty(intent).map_err(|_| Error::Json)?,
    )
}
