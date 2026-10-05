use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use horizon_app_provider::api::Quota;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::account::Account;
use crate::{Error, Result};

pub mod execution;
pub mod recovery;
mod store;
use store::Store;

const MAX_OPERATIONS: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Run,
    Upload,
    Session,
    Tunnel,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Allocating,
    Active,
    Releasing,
    Uncertain,
    Complete,
}

/// Host-only provider capacity evidence. IDs must come from fresh running-session metadata, never tool input.
pub struct Capacity {
    pub quota: Quota,
    running: BTreeSet<String>,
}
impl Capacity {
    /// # Errors
    /// Inconsistent quota/session observations hold admission rather than assuming overlap.
    pub fn observed(quota: Quota, running: BTreeSet<String>) -> Result<Self> {
        if running.len() as u64 > u64::from(quota.parallel_sessions_running)
            || running.iter().any(|id| !valid_reference(id))
        {
            return Err(Error::CapacityUnavailable);
        }
        Ok(Self { quota, running })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Operation {
    pub id: Uuid,
    pub kind: Kind,
    pub phase: Phase,
    pub created_seconds: u64,
    pub deadline_seconds: u64,
    pub pending_resources: usize,
    pub expired: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    #[serde(deserialize_with = "unique_records")]
    records: BTreeMap<Uuid, Record>,
}

fn unique_records<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<Uuid, Record>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<Uuid, Record>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique native operation records")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> std::result::Result<Self::Value, M::Error> {
            let mut records = BTreeMap::new();
            while let Some((id, record)) = map.next_entry()? {
                if records.insert(id, record).is_some() {
                    return Err(serde::de::Error::custom("duplicate native operation"));
                }
            }
            Ok(records)
        }
    }
    deserializer.deserialize_map(Unique)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    owner: Uuid,
    realm: String,
    root: PathBuf,
    kind: Kind,
    phase: Phase,
    created: u64,
    deadline: u64,
    slot: Option<Slot>,
    resources: Vec<Resource>,
}

#[derive(Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
enum Slot {
    Pending,
    Allocated,
}

// Never expose resource serialization through agent tools, logs or audit.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Resource {
    UploadIntent { digest: String },
    Upload { reference: String },
    AllocationIntent {},
    Session { reference: String },
}

/// One host-owned account namespace shared by all workspace actors and credential profiles.
/// Its private root is host configuration, never a project-contract or MCP parameter.
pub struct Journal {
    store: Store,
    realm: String,
}

impl Journal {
    /// # Errors
    /// The state directory must be private, owned by this OS user and free of symlink components.
    pub fn open(state: &Path, account: &Account) -> Result<Self> {
        let store = Store::open(&state.join(Account::capacity_namespace()))?;
        Ok(Self {
            store,
            realm: account.realm().to_owned(),
        })
    }

    /// # Errors
    /// Records intent durably before builds, uploads, tunnel spawn or remote allocation.
    pub fn start(&self, owner: Uuid, root: &Path, kind: Kind, lifetime: Duration) -> Result<Operation> {
        if owner.is_nil() || lifetime.is_zero() || lifetime > Duration::from_mins(30) {
            return Err(Error::OperationInvalid);
        }
        if root.canonicalize().map_err(|_| Error::OperationInvalid)? != root || !root.is_dir() {
            return Err(Error::OperationInvalid);
        }
        let root = root.to_owned();
        self.edit(|ledger| {
            if ledger.records.len() >= MAX_OPERATIONS {
                return Err(Error::JournalUnavailable);
            }
            let id = Uuid::new_v4();
            let created = now()?;
            let record = Record {
                owner,
                root,
                realm: self.realm.clone(),
                kind,
                phase: Phase::Preparing,
                created,
                deadline: created + lifetime.as_secs().max(1),
                slot: None,
                resources: Vec::new(),
            };
            let operation = snapshot(id, &record)?;
            ledger.records.insert(id, record);
            Ok(operation)
        })
    }

    /// # Errors
    /// Checks account capacity under the cross-process journal lock. Expired or uncertain slots remain reserved.
    pub fn reserve(&self, owner: Uuid, id: Uuid, capacity: impl FnOnce() -> Result<Capacity>) -> Result<()> {
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.kind != Kind::Session || record.slot.is_some() || record.phase != Phase::Preparing {
                return Err(Error::OperationInvalid);
            }
            if record.deadline <= now()? {
                return Err(Error::OperationExpired);
            }
            let pending = ledger
                .records
                .values()
                .filter(|record| record.slot == Some(Slot::Pending))
                .count();
            let capacity = capacity()?;
            let quota = capacity.quota;
            let maximum = quota.parallel_sessions_max_allowed.min(quota.team_parallel_sessions_max_allowed);
            let missing = ledger.records.values().filter(|record| record.slot == Some(Slot::Allocated))
                .filter(|record| !matches!(record.resources.as_slice(), [Resource::Session {reference}] if capacity.running.contains(reference))).count();
            let occupied = u64::from(quota.parallel_sessions_running.saturating_add(quota.queued_sessions))
                + missing as u64 + pending as u64;
            if occupied >= u64::from(maximum) {
                return Err(Error::CapacityUnavailable);
            }
            let record = self.record(ledger, owner, id)?;
            if record.deadline <= now()? { return Err(Error::OperationExpired); }
            record.slot = Some(Slot::Pending);
            record.phase = Phase::Allocating;
            record.resources.push(Resource::AllocationIntent {});
            Ok(())
        })
    }

    /// # Errors
    /// Host-only callback after allocation; private provider IDs never appear in operation snapshots.
    pub fn allocated(&self, owner: Uuid, id: Uuid, provider_session: &str) -> Result<()> {
        if !valid_reference(provider_session) {
            return Err(Error::OperationInvalid);
        }
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.slot != Some(Slot::Pending) || record.phase != Phase::Allocating {
                return Err(Error::OperationInvalid);
            }
            record
                .resources
                .retain(|resource| !matches!(resource, Resource::AllocationIntent {}));
            record.resources.push(Resource::Session {
                reference: provider_session.to_owned(),
            });
            record.slot = Some(Slot::Allocated);
            record.phase = Phase::Active;
            Ok(())
        })
    }

    /// # Errors
    /// Records the content digest before the provider upload request; the operation ID is its private custom ID.
    pub fn upload_intent(&self, owner: Uuid, id: Uuid, digest: &str) -> Result<()> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::OperationInvalid);
        }
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.kind != Kind::Upload || record.phase != Phase::Preparing || !record.resources.is_empty() {
                return Err(Error::OperationInvalid);
            }
            if record.deadline <= now()? {
                return Err(Error::OperationExpired);
            }
            record.resources.push(Resource::UploadIntent {
                digest: digest.to_owned(),
            });
            record.phase = Phase::Allocating;
            Ok(())
        })
    }

    /// # Errors
    /// Host-only callback after upload; the raw reference is retained solely in the private ledger.
    pub fn uploaded(&self, owner: Uuid, id: Uuid, reference: &str) -> Result<()> {
        if !valid_app(reference) {
            return Err(Error::OperationInvalid);
        }
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.kind != Kind::Upload
                || record.phase != Phase::Allocating
                || !matches!(record.resources.as_slice(), [Resource::UploadIntent { .. }])
            {
                return Err(Error::OperationInvalid);
            }
            record.resources.clear();
            record.resources.push(Resource::Upload {
                reference: reference.to_owned(),
            });
            record.phase = Phase::Active;
            Ok(())
        })
    }

    /// # Errors
    /// Cleanup receives only an exact privately recorded resource. Failure preserves identity and reservation.
    pub fn release_session(&self, owner: Uuid, id: Uuid, close: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.release(owner, id, Kind::Session, close)
    }

    /// # Errors
    /// Only the original credential realm and workspace may delete an owned uploaded app.
    pub fn release_upload(&self, owner: Uuid, id: Uuid, delete: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.release(owner, id, Kind::Upload, delete)
    }

    fn release(&self, owner: Uuid, id: Uuid, kind: Kind, cleanup: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        let reference = self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.kind != kind {
                return Err(Error::OperationInvalid);
            }
            if record.phase == Phase::Complete {
                return Ok(None);
            }
            if !matches!(record.phase, Phase::Active | Phase::Uncertain) {
                return Err(Error::OperationInvalid);
            }
            let reference = match record.resources.as_slice() {
                [Resource::Session { reference }] if kind == Kind::Session => reference,
                [Resource::Upload { reference }] if kind == Kind::Upload => reference,
                _ => return Err(Error::OperationInvalid),
            };
            let reference = Zeroizing::new(reference.clone());
            record.phase = Phase::Releasing;
            Ok(Some(reference))
        })?;
        let Some(reference) = reference else {
            return Ok(());
        };
        if let Err(error) = cleanup(&reference) {
            self.uncertain(owner, id)?;
            return Err(error);
        }
        self.confirm_released(owner, id)
    }

    /// # Errors
    /// Uncertain allocations keep their capacity reservation until provider reconciliation confirms cleanup.
    pub fn uncertain(&self, owner: Uuid, id: Uuid) -> Result<()> {
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.phase == Phase::Complete {
                return Err(Error::OperationInvalid);
            }
            record.phase = Phase::Uncertain;
            Ok(())
        })
    }

    /// # Errors
    /// Only a confirmed host cleanup may retire intent, resources and reserved capacity.
    pub fn confirm_released(&self, owner: Uuid, id: Uuid) -> Result<()> {
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            record.phase = Phase::Complete;
            record.resources.clear();
            record.slot = None;
            Ok(())
        })
    }

    /// # Errors
    /// Foreign owners and changed credential realms cannot access an operation.
    pub fn status(&self, owner: Uuid, id: Uuid) -> Result<Operation> {
        self.store
            .access(false, |ledger| snapshot(id, self.record(ledger, owner, id)?))
    }

    /// # Errors
    /// Retained records stay visible after restart, including expired and uncertain operations.
    pub fn pending(&self, owner: Uuid) -> Result<Vec<Operation>> {
        self.store.access(false, |ledger| {
            ledger
                .records
                .iter()
                .filter(|(_, record)| {
                    record.owner == owner && record.realm == self.realm && record.phase != Phase::Complete
                })
                .map(|(id, record)| snapshot(*id, record))
                .collect()
        })
    }

    /// # Errors
    /// Only confirmed complete records can be removed from the bounded journal.
    pub fn retire(&self, owner: Uuid, id: Uuid) -> Result<()> {
        self.edit(|ledger| {
            let record = self.record(ledger, owner, id)?;
            if record.phase != Phase::Complete || record.slot.is_some() || !record.resources.is_empty() {
                return Err(Error::OperationInvalid);
            }
            ledger.records.remove(&id);
            Ok(())
        })
    }

    fn record<'a>(&self, ledger: &'a mut Ledger, owner: Uuid, id: Uuid) -> Result<&'a mut Record> {
        let record = ledger.records.get_mut(&id).ok_or(Error::OwnershipRefused)?;
        if record.owner != owner || record.realm != self.realm {
            return Err(Error::OwnershipRefused);
        }
        Ok(record)
    }

    fn edit<T>(&self, operation: impl FnOnce(&mut Ledger) -> Result<T>) -> Result<T> {
        self.store.access(true, operation)
    }
}

fn snapshot(id: Uuid, record: &Record) -> Result<Operation> {
    Ok(Operation {
        id,
        kind: record.kind,
        phase: record.phase,
        created_seconds: record.created,
        deadline_seconds: record.deadline,
        pending_resources: record.resources.len(),
        expired: record.deadline <= now()?,
    })
}

fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| Error::JournalUnavailable)
}

fn valid_app(value: &str) -> bool {
    value
        .strip_prefix("bs://")
        .is_some_and(|id| (16..=128).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn valid_reference(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn validate(ledger: &Ledger) -> Result<()> {
    if ledger.version != 1 || ledger.records.len() > MAX_OPERATIONS {
        return Err(Error::JournalInvalid);
    }
    let mut references = BTreeSet::new();
    for (id, record) in &ledger.records {
        if id.is_nil()
            || record.owner.is_nil()
            || record.realm.len() != 64
            || !record.realm.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !record.root.is_absolute()
            || record.deadline < record.created
            || record.deadline - record.created > 1800
            || record.resources.len() > 128
            || record.phase == Phase::Complete && (record.slot.is_some() || !record.resources.is_empty())
        {
            return Err(Error::JournalInvalid);
        }
        let empty = record.slot.is_none() && record.resources.is_empty();
        let pending = record.kind == Kind::Session
            && record.slot == Some(Slot::Pending)
            && matches!(record.resources.as_slice(), [Resource::AllocationIntent {}])
            || record.kind == Kind::Upload
                && record.slot.is_none()
                && matches!(record.resources.as_slice(), [Resource::UploadIntent { .. }]);
        let active = record.kind == Kind::Session
            && record.slot == Some(Slot::Allocated)
            && matches!(record.resources.as_slice(), [Resource::Session { .. }])
            || record.kind == Kind::Upload
                && record.slot.is_none()
                && matches!(record.resources.as_slice(), [Resource::Upload { .. }]);
        let valid = match record.phase {
            Phase::Preparing | Phase::Complete => empty,
            Phase::Allocating => pending,
            Phase::Active | Phase::Releasing => active,
            Phase::Uncertain => empty || pending || active,
        };
        if !valid {
            return Err(Error::JournalInvalid);
        }
        for resource in &record.resources {
            match resource {
                Resource::Session { reference }
                    if !valid_reference(reference) || !references.insert(reference.as_str()) =>
                {
                    return Err(Error::JournalInvalid);
                }
                Resource::Upload { reference } if !valid_app(reference) || !references.insert(reference.as_str()) => {
                    return Err(Error::JournalInvalid);
                }
                Resource::UploadIntent { digest }
                    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
                {
                    return Err(Error::JournalInvalid);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
