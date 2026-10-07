//! One controller owns uploads and device lanes for MCP, CLI, recipes and live views.
use crate::{Error, Result, local::Local};
use horizon_app_provider::{api::UploadedApp, artifact::Artifact};
use horizon_app_runtime::{
    account::Account,
    journal::{Capacity, Kind, Phase, execution::Workspace},
};
use horizon_app_testing::{
    catalog::{Device, ResolvedDevice, resolve},
    contract::{Contract, Platform},
    driver::{Launch, NativeDriver},
    recipe::{Action, State, Target},
    tree::Snapshot,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

mod backend;
mod cleanup;
mod expiry;
mod media;
mod sessions;
mod uploads;
use backend::Backend;

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactHandle {
    pub id: Uuid,
    pub platform: Platform,
    pub sha256: String,
    pub bytes: u64,
    pub remaining_seconds: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct SessionHandle {
    pub id: Uuid,
    pub target: Device,
    pub remaining_seconds: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    Active,
    Closing,
    Acknowledged,
    Complete,
}
struct Upload {
    id: Uuid,
    platform: Platform,
    sha256: String,
    bytes: u64,
    deadline: Instant,
    app: Option<Arc<UploadedApp>>,
    users: BTreeSet<Uuid>,
    handles: BTreeSet<Uuid>,
    cleanup: Cleanup,
    cleanup_inflight: bool,
}
struct Lane {
    id: Uuid,
    matrix_index: usize,
    deadline: Instant,
    app: Arc<Mutex<Upload>>,
    resources: Vec<crate::local::Lease>,
    driver: Option<NativeDriver>,
    tunnel: Option<Uuid>,
    attempted: bool,
    held_local: Option<Uuid>,
    cleanup: Cleanup,
    cleanup_inflight: bool,
    view_closed: Arc<std::sync::atomic::AtomicBool>,
}

/// Opaque handles select only this controller's resources. Each lane serializes independently.
pub struct Actor {
    audit: crate::audit::Audit,
    observations: crate::observations::Observations,
    media_reads: std::sync::atomic::AtomicUsize,
    workspace: Arc<Workspace>,
    backend: Arc<dyn Backend>,
    contract: Contract,
    matrix: Vec<ResolvedDevice>,
    pub(crate) local: Arc<Local>,
    uploads: Mutex<BTreeMap<Uuid, Arc<Mutex<Upload>>>>,
    lanes: Mutex<BTreeMap<Uuid, Arc<Mutex<Lane>>>>,
    claims: Arc<Mutex<BTreeMap<u16, Uuid>>>,
    admission: Mutex<()>,
    upload_admission: Mutex<()>,
    run: RwLock<()>,
    expiry_started: std::sync::atomic::AtomicBool,
    retiring: bool,
    stopping: std::sync::atomic::AtomicBool,
}
impl Actor {
    /// # Errors
    /// Trusted construction binds one configured account and exclusive project workspace.
    pub fn new(workspace: Arc<Workspace>, account: &Account, local: Arc<Local>, contract: Contract) -> Result<Self> {
        contract.validate()?;
        if !local.belongs_to(&workspace) {
            return Err(horizon_app_runtime::Error::OwnershipRefused.into());
        }
        let provider = workspace.provider(account)?;
        let matrix = resolve(&contract.matrix, &provider.devices()?)?;
        Ok(Self::from_backend(workspace, provider, local, contract, matrix))
    }
    fn from_backend(
        workspace: Arc<Workspace>,
        backend: Arc<dyn Backend>,
        local: Arc<Local>,
        contract: Contract,
        matrix: Vec<ResolvedDevice>,
    ) -> Self {
        Self {
            audit: crate::audit::Audit::default(),
            observations: crate::observations::Observations::default(),
            media_reads: std::sync::atomic::AtomicUsize::new(0),
            workspace,
            backend,
            local,
            contract,
            matrix,
            uploads: Mutex::new(BTreeMap::new()),
            lanes: Mutex::new(BTreeMap::new()),
            claims: Arc::new(Mutex::new(BTreeMap::new())),
            admission: Mutex::new(()),
            upload_admission: Mutex::new(()),
            run: RwLock::new(()),
            expiry_started: std::sync::atomic::AtomicBool::new(false),
            retiring: false,
            stopping: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub(crate) fn contract(&self) -> &Contract {
        &self.contract
    }
    /// # Errors
    /// Returns only this actor's bounded redacted operation receipts.
    pub fn audit(&self, after_sequence: u64, limit: usize) -> Result<crate::audit::Page> {
        self.audit.page(after_sequence, limit)
    }
    pub(crate) fn targets(&self) -> Vec<ResolvedDevice> {
        self.matrix.clone()
    }
    pub(crate) fn project_text(&self, path: &std::path::Path) -> Result<String> {
        crate::project::read(&self.workspace.root_directory()?, path)
    }
    pub(crate) fn available_parallel(&self, timeout: Duration) -> Result<usize> {
        usize::try_from(self.backend.capacity(timeout)?.quota.available()).map_err(|_| Error::Unavailable)
    }
    pub(crate) fn begin_run(&self) -> Result<std::sync::RwLockWriteGuard<'_, ()>> {
        let run = self.run.try_write().map_err(|_| Error::RunBusy)?;
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.expire()?;
        self.prune_completed_provider()?;
        if !self.workspace.journal().pending(self.workspace.owner())?.is_empty() {
            return Err(Error::RunBusy);
        }
        Ok(run)
    }
    fn durable_active(&self, id: Uuid, kind: Kind) -> Result<()> {
        let record = self.workspace.journal().status(self.workspace.owner(), id)?;
        if record.kind != kind || record.phase != Phase::Active {
            return Err(Error::CleanupUncertain);
        }
        Ok(())
    }
}

fn deadline(lifetime: Duration) -> Result<Instant> {
    if lifetime.is_zero() || lifetime > Duration::from_mins(30) {
        return Err(horizon_app_runtime::Error::OperationInvalid.into());
    }
    Ok(Instant::now() + lifetime)
}
fn remaining(until: Instant) -> Result<Duration> {
    let remaining = until.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(horizon_app_runtime::Error::OperationExpired.into());
    }
    Ok(remaining)
}

#[cfg(all(test, unix))]
pub(crate) mod tests;
