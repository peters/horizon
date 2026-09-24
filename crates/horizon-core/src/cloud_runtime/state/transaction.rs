//! Paired legacy journals under allocation-before-project locks.
use super::{Error, Result};
use crate::cloud_runtime::allocation::{AllocationId, ControllerId, ProjectIdentity, legacy::Records};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const TRANSACTION: &str = "transaction.json";
const ALLOCATION: &str = "allocation.json";
const PROJECT: &str = "deployment.json";

pub(super) struct LockedPair {
    project_root: PathBuf,
    allocation_root: PathBuf,
    identity: AllocationId,
    project: ProjectIdentity,
    controller: ControllerId,
    _allocation_lock: OperationLock,
    _project_lock: OperationLock,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    version: u32,
    generation: u64,
    owner: PathBuf,
    allocation: AllocationId,
    phase: Phase,
    previous: Snapshot,
    next: Payload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Pending,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    allocation: Option<[u8; 32]>,
    project: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    allocation: String,
    project: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    Intent,
    Allocation,
    Project,
    Complete,
}

impl LockedPair {
    pub(super) fn acquire(
        project_root: &Path,
        identity: AllocationId,
        project: ProjectIdentity,
        controller: ControllerId,
    ) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
        let project_root = project_root.canonicalize()?;
        let parent = project_root
            .parent()
            .ok_or(Error::Invalid("Missing cloud state parent"))?;
        let allocations = parent.join(".allocations");
        create_directory(&allocations)?;
        let allocation_root = allocations.join(identity.to_string());
        create_directory(&allocation_root)?;
        let allocation_lock = lock_file(&allocation_root)?;
        let project_lock = lock_file(&project_root)?;
        Ok(Self {
            project_root,
            allocation_root,
            identity,
            project,
            controller,
            _allocation_lock: allocation_lock,
            _project_lock: project_lock,
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.project_root
    }

    pub(super) fn load(&mut self) -> Result<Records> {
        let mut transaction = self.read_transaction()?;
        if transaction.phase == Phase::Pending {
            self.finish(&mut transaction, &mut |_| Ok(()))?;
        }
        self.verify_complete(&transaction)?;
        decode(&transaction.next)
    }

    pub(super) fn save(&mut self, records: &Records) -> Result<()> {
        self.save_with(records, &mut |_| Ok(()))
    }

    pub(super) fn save_with(
        &mut self,
        records: &Records,
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        let current = self.read_transaction()?;
        self.verify_complete(&current)?;
        let previous = decode(&current.next)?;
        if previous.identity() != records.identity()
            || previous.controller_id() != records.controller_id()
            || records.allocation_id() != self.identity
        {
            return Err(Error::Invalid("A journal update cannot change project ownership"));
        }
        let generation = current
            .generation
            .checked_add(1)
            .ok_or(Error::Invalid("Journal generation exhausted"))?;
        let mut next = Transaction {
            version: 1,
            generation,
            owner: self.project_root.clone(),
            allocation: self.identity,
            phase: Phase::Pending,
            previous: Snapshot {
                allocation: Some(hash(current.next.allocation.as_bytes())),
                project: Some(hash(current.next.project.as_bytes())),
            },
            next: encode(records)?,
        };
        self.write_transaction(&next)?;
        checkpoint(Boundary::Intent)?;
        self.finish(&mut next, checkpoint)
    }

    // Migration must establish and verify its old-reader barrier before this call.
    pub(super) fn initialize(
        &mut self,
        records: &Records,
        marker: &[u8],
        checkpoint: &mut impl FnMut(Boundary) -> Result<()>,
    ) -> Result<()> {
        if records.allocation_id() != self.identity {
            return Err(Error::Invalid("Migration allocation differs from its lock"));
        }
        if read_optional(&self.allocation_root.join(TRANSACTION))?.is_some() {
            let transaction = self.read_transaction()?;
            if transaction.next != encode(records)? {
                return Err(Error::Invalid("Existing allocation differs from migration intent"));
            }
            self.load()?;
            return Ok(());
        }
        if read_optional(&self.allocation_root.join(ALLOCATION))?.is_some()
            || read_optional(&self.project_root.join(PROJECT))?.as_deref() != Some(marker)
        {
            return Err(Error::Invalid("Migration barrier or allocation state changed"));
        }
        let mut transaction = Transaction {
            version: 1,
            generation: 0,
            owner: self.project_root.clone(),
            allocation: self.identity,
            phase: Phase::Pending,
            previous: Snapshot {
                allocation: None,
                project: Some(hash(marker)),
            },
            next: encode(records)?,
        };
        self.write_transaction(&transaction)?;
        checkpoint(Boundary::Intent)?;
        self.finish(&mut transaction, checkpoint)
    }

    pub(super) fn is_uninitialized(&self) -> Result<bool> {
        Ok(read_optional(&self.allocation_root.join(TRANSACTION))?.is_none()
            && read_optional(&self.allocation_root.join(ALLOCATION))?.is_none())
    }

    fn read_transaction(&self) -> Result<Transaction> {
        read_transaction(
            &self.project_root,
            &self.allocation_root,
            self.identity,
            &self.project,
            self.controller,
        )
    }

    fn write_transaction(&self, transaction: &Transaction) -> Result<()> {
        write_atomic(
            &self.allocation_root,
            TRANSACTION,
            &serde_json::to_vec_pretty(transaction).map_err(|_| Error::Json)?,
        )
    }

    fn verify_complete(&self, transaction: &Transaction) -> Result<()> {
        if transaction.phase != Phase::Complete
            || read_optional(&self.allocation_root.join(ALLOCATION))?.as_deref()
                != Some(transaction.next.allocation.as_bytes())
            || read_optional(&self.project_root.join(PROJECT))?.as_deref() != Some(transaction.next.project.as_bytes())
        {
            return Err(Error::Invalid(
                "Allocation and project journal commit is incomplete or changed",
            ));
        }
        Ok(())
    }

    fn finish(&self, transaction: &mut Transaction, checkpoint: &mut impl FnMut(Boundary) -> Result<()>) -> Result<()> {
        let allocation = read_optional(&self.allocation_root.join(ALLOCATION))?;
        let project = read_optional(&self.project_root.join(PROJECT))?;
        if !matches_boundary(
            allocation.as_deref(),
            transaction.previous.allocation.as_ref(),
            transaction.next.allocation.as_bytes(),
        ) || !matches_boundary(
            project.as_deref(),
            transaction.previous.project.as_ref(),
            transaction.next.project.as_bytes(),
        ) {
            return Err(Error::Invalid("Pending transaction conflicts with persisted state"));
        }
        write_atomic(
            &self.allocation_root,
            ALLOCATION,
            transaction.next.allocation.as_bytes(),
        )?;
        checkpoint(Boundary::Allocation)?;
        write_atomic(&self.project_root, PROJECT, transaction.next.project.as_bytes())?;
        checkpoint(Boundary::Project)?;
        transaction.phase = Phase::Complete;
        self.write_transaction(transaction)?;
        checkpoint(Boundary::Complete)?;
        self.verify_complete(transaction)
    }
}

// The caller holds the parent migration lock. All migrated updates retain that
// lock, so inspecting a neighbor requires no allocation-after-project lock.
pub(super) fn inspect(
    project_root: &Path,
    identity: AllocationId,
    project: &ProjectIdentity,
    controller: ControllerId,
) -> Result<Records> {
    let allocation_root = project_root
        .parent()
        .ok_or(Error::Invalid("Missing cloud parent"))?
        .join(".allocations")
        .join(identity.to_string());
    let transaction = read_transaction(project_root, &allocation_root, identity, project, controller)?;
    let records = decode(&transaction.next)?;
    let allocation = read_optional(&allocation_root.join(ALLOCATION))?;
    let current_project = read_optional(&project_root.join(PROJECT))?;
    let matches = match transaction.phase {
        Phase::Complete => {
            allocation.as_deref() == Some(transaction.next.allocation.as_bytes())
                && current_project.as_deref() == Some(transaction.next.project.as_bytes())
        }
        Phase::Pending => {
            matches_boundary(
                allocation.as_deref(),
                transaction.previous.allocation.as_ref(),
                transaction.next.allocation.as_bytes(),
            ) && matches_boundary(
                current_project.as_deref(),
                transaction.previous.project.as_ref(),
                transaction.next.project.as_bytes(),
            )
        }
    };
    if !matches {
        return Err(Error::Invalid(
            "Neighboring allocation transaction conflicts with its files",
        ));
    }
    Ok(records)
}

// The registry is itself an ownership fence when a project directory is missing.
pub(super) fn registered_records(parent: &Path) -> Result<Vec<Records>> {
    let registry = parent.join(".allocations");
    let mut records = Vec::new();
    if !registry.try_exists()? {
        return Ok(records);
    }
    for entry in fs::read_dir(&registry)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            return Err(Error::Invalid("Unexpected allocation registry entry"));
        }
        let root = entry.path();
        let Some(bytes) = read_optional(&root.join(TRANSACTION))? else {
            if read_optional(&root.join(ALLOCATION))?.is_some() {
                return Err(Error::Invalid("Allocation lost its transaction"));
            }
            continue;
        };
        let transaction: Transaction = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        let next = decode(&transaction.next)?;
        if entry.file_name().to_str() != Some(next.allocation_id().to_string().as_str()) {
            return Err(Error::Invalid("Allocation directory identity differs"));
        }
        let project_root = parent.join(next.identity().cloud_id());
        records.push(inspect(
            &project_root,
            next.allocation_id(),
            next.identity(),
            next.controller_id(),
        )?);
    }
    Ok(records)
}

fn read_transaction(
    project_root: &Path,
    allocation_root: &Path,
    identity: AllocationId,
    project: &ProjectIdentity,
    controller: ControllerId,
) -> Result<Transaction> {
    let bytes =
        read_optional(&allocation_root.join(TRANSACTION))?.ok_or(Error::Invalid("Missing allocation transaction"))?;
    let transaction: Transaction = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    let records = decode(&transaction.next)?;
    if transaction.version != 1
        || transaction.owner != project_root
        || transaction.allocation != identity
        || records.allocation_id() != identity
        || records.identity() != project
        || records.controller_id() != controller
        || transaction.previous.project.is_none()
        || (transaction.previous.allocation.is_none() != (transaction.generation == 0))
    {
        return Err(Error::Invalid(
            "Allocation journal ownership, generation or version differs",
        ));
    }
    Ok(transaction)
}

fn encode(records: &Records) -> Result<Payload> {
    Ok(Payload {
        allocation: String::from_utf8(records.allocation_bytes().map_err(|_| Error::Json)?).map_err(|_| Error::Json)?,
        project: String::from_utf8(records.project_bytes().map_err(|_| Error::Json)?).map_err(|_| Error::Json)?,
    })
}

fn decode(payload: &Payload) -> Result<Records> {
    Records::decode(payload.allocation.as_bytes(), payload.project.as_bytes()).map_err(|_| Error::Json)
}

fn matches_boundary(actual: Option<&[u8]>, previous: Option<&[u8; 32]>, next: &[u8]) -> bool {
    actual == Some(next) || actual.map(hash).as_ref() == previous
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(Error::Invalid("Journal is not a plain file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    Ok(Some(fs::read(path)?))
}

pub(super) fn write_atomic(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(root.join(name)).map_err(|error| error.error)?;
    sync_directory(root)
}

fn create_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(Error::Invalid("Allocation directory is not a plain directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return create_directory(path),
            Err(error) => return Err(error.into()),
        },
        Err(error) => return Err(error.into()),
    }
    // A prior attempt may have crashed between creation and parent synchronization.
    sync_directory(path.parent().ok_or(Error::Invalid("Missing allocation parent"))?)?;
    sync_directory(path)
}

pub(super) struct OperationLock(File);

impl Drop for OperationLock {
    fn drop(&mut self) {
        // Explicit unlock also releases a descriptor briefly inherited by a
        // concurrently spawning child; closing only this descriptor would not.
        let _ = self.0.unlock();
    }
}

pub(super) fn lock_file(root: &Path) -> Result<OperationLock> {
    lock_named(root, "operation.lock")
}

pub(super) fn lock_named(root: &Path, name: &str) -> Result<OperationLock> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(Error::Invalid("Operation lock is not a plain file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(|_| Error::Busy)?;
    Ok(OperationLock(file))
}

fn sync_directory(root: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}
