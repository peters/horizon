//! One controller owns uploads and device lanes for MCP, CLI, recipes and live views.
use crate::{Error, Result, local::Local};
use horizon_app_provider::{
    api::{BrowserStack, UploadedApp},
    artifact::Artifact,
};
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
use horizon_browser::ClassicTransport;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) trait Backend: Send + Sync {
    fn capacity(&self, timeout: Duration) -> Result<Capacity>;
    fn driver(&self) -> Result<Arc<dyn ClassicTransport>>;
    fn verify(&self, id: &str, target: &Device, app: &UploadedApp, operation: Uuid, timeout: Duration) -> Result<()>;
    fn upload(&self, artifact: &mut Artifact, operation: Uuid, timeout: Duration) -> Result<UploadedApp>;
    fn delete(&self, app: &UploadedApp) -> Result<()>;
    fn confirmed_closed(&self, reference: &str) -> Result<bool>;
    fn session_link(&self, _reference: &str, _timeout: Duration) -> Result<String> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
    fn media(&self, _reference: &str, _kind: horizon_app_provider::media::Kind, _timeout: Duration) -> Result<Vec<u8>> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
}
impl Backend for BrowserStack {
    fn session_link(&self, reference: &str, timeout: Duration) -> Result<String> {
        Ok(self
            .session_with_timeout(reference, timeout.min(Duration::from_secs(10)))?
            .dashboard_link()?)
    }
    fn media(&self, reference: &str, kind: horizon_app_provider::media::Kind, timeout: Duration) -> Result<Vec<u8>> {
        Ok(BrowserStack::media_with_timeout(self, reference, kind, timeout)?)
    }
    fn capacity(&self, timeout: Duration) -> Result<Capacity> {
        let observation = self.native_capacity(timeout)?;
        Ok(Capacity::observed(observation.quota, observation.running)?)
    }
    fn driver(&self) -> Result<Arc<dyn ClassicTransport>> {
        Ok(BrowserStack::driver(self)?)
    }
    fn verify(&self, id: &str, target: &Device, app: &UploadedApp, operation: Uuid, timeout: Duration) -> Result<()> {
        let observed = self.session_with_timeout(id, timeout)?;
        observed.verify(target, app, operation)?;
        if !observed.active()? {
            return Err(horizon_app_provider::Error::DeviceUnverified.into());
        }
        Ok(())
    }
    fn confirmed_closed(&self, reference: &str) -> Result<bool> {
        Ok(!self
            .session_with_timeout(reference, Duration::from_secs(15))?
            .active()?)
    }
    fn upload(&self, artifact: &mut Artifact, operation: Uuid, timeout: Duration) -> Result<UploadedApp> {
        Ok(self.upload_with_timeout(artifact, operation, timeout)?)
    }
    fn delete(&self, app: &UploadedApp) -> Result<()> {
        Ok(self.delete_app(app)?)
    }
}

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
    /// # Errors
    /// Explicit normal-host shutdown does not depend on the last viewer/request Arc disappearing.
    pub fn shutdown(&self) -> Result<()> {
        self.stopping.store(true, std::sync::atomic::Ordering::Release);
        let _uploads = self.upload_admission.lock().map_err(|_| Error::CleanupUncertain)?;
        let _admission = self.admission.lock().map_err(|_| Error::CleanupUncertain)?;
        let lanes = self
            .lanes
            .lock()
            .map_err(|_| Error::CleanupUncertain)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut failed = false;
        for lane in lanes {
            match lane.lock() {
                Ok(mut lane) => failed |= self.close_lane(&mut lane).is_err(),
                Err(_) => failed = true,
            }
        }
        let uploads = self
            .uploads
            .lock()
            .map_err(|_| Error::CleanupUncertain)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for upload in uploads {
            match upload.lock() {
                Ok(mut upload) => {
                    upload.handles.clear();
                    failed |= self.close_upload(&mut upload).is_err();
                }
                Err(_) => failed = true,
            }
        }
        if failed { Err(Error::CleanupUncertain) } else { Ok(()) }
    }
    pub(crate) fn retain_run_evidence(&self) -> Result<crate::observations::Run<'_>> {
        self.observations.begin_run()
    }

    pub(crate) fn begin_run(&self) -> Result<std::sync::RwLockWriteGuard<'_, ()>> {
        let run = self.run.try_write().map_err(|_| Error::RunBusy)?;
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.expire()?;
        if !self.workspace.journal().pending(self.workspace.owner())?.is_empty() {
            return Err(Error::RunBusy);
        }
        Ok(run)
    }
    fn upload_handle(upload: &mut Upload) -> ArtifactHandle {
        let handle = Uuid::new_v4();
        upload.handles.insert(handle);
        ArtifactHandle {
            id: handle,
            platform: upload.platform,
            sha256: upload.sha256.clone(),
            bytes: upload.bytes,
            remaining_seconds: upload.deadline.saturating_duration_since(Instant::now()).as_secs(),
        }
    }
    /// # Errors
    /// Capture only a declared immutable artifact; content reuse never renews its original deadline.
    pub fn upload(&self, platform: Platform, lifetime: Duration) -> Result<ArtifactHandle> {
        let _run = self.run.try_read().map_err(|_| Error::RunBusy)?;
        self.upload_until(platform, deadline(lifetime)?)
    }
    pub(crate) fn upload_until(&self, platform: Platform, until: Instant) -> Result<ArtifactHandle> {
        self.audit.execute(None, "upload", || {
            let _serial = self.upload_admission.lock().map_err(|_| Error::Unavailable)?;
            if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            remaining(until)?;
            let mut artifact =
                Artifact::capture_directory(&self.workspace.root_directory()?, &self.contract, platform)?;
            self.expire()?;
            let mut uploads = self.uploads.lock().map_err(|_| Error::Unavailable)?;
            uploads.retain(|_, upload| upload.lock().map_or(true, |upload| upload.cleanup != Cleanup::Complete));
            for upload in uploads.values() {
                let mut upload = upload.lock().map_err(|_| Error::Unavailable)?;
                if upload.platform == platform
                    && upload.sha256 == artifact.sha256()
                    && upload.cleanup != Cleanup::Active
                    && upload.cleanup != Cleanup::Complete
                {
                    return Err(Error::CleanupUncertain);
                }
                if upload.platform == platform
                    && upload.sha256 == artifact.sha256()
                    && upload.app.is_some()
                    && upload.cleanup == Cleanup::Active
                    && Instant::now() < upload.deadline
                {
                    self.durable_active(upload.id, Kind::Upload)?;
                    remaining(upload.deadline)?;
                    return Ok(Self::upload_handle(&mut upload));
                }
            }
            if uploads.len() >= 64 {
                return Err(Error::Unavailable);
            }
            let operation = self.workspace.start(Kind::Upload, remaining(until)?)?;
            self.workspace
                .journal()
                .upload_intent(self.workspace.owner(), operation.id, artifact.sha256())?;
            let upload = Upload {
                id: operation.id,
                platform,
                sha256: artifact.sha256().to_owned(),
                bytes: artifact.bytes(),
                deadline: until,
                app: None,
                users: BTreeSet::new(),
                handles: BTreeSet::new(),
                cleanup: Cleanup::Active,
                cleanup_inflight: false,
            };
            let budget = match remaining(until) {
                Ok(budget) => budget.min(Duration::from_secs(180)),
                Err(error) => {
                    self.workspace
                        .journal()
                        .confirm_released(self.workspace.owner(), operation.id)?;
                    return Err(error);
                }
            };
            // Retain attempted ownership before any post-dispatch bookkeeping can fail.
            let upload = Arc::new(Mutex::new(upload));
            uploads.insert(operation.id, Arc::clone(&upload));
            drop(uploads);
            let mut upload = upload.lock().map_err(|_| Error::Unavailable)?;
            match self.backend.upload(&mut artifact, operation.id, budget) {
                Ok(app) => upload.app = Some(Arc::new(app)),
                Err(error) => {
                    upload.cleanup = Cleanup::Closing;
                    let recorded = self.workspace.journal().uncertain(self.workspace.owner(), operation.id);
                    return Err(if recorded.is_err() {
                        Error::CleanupUncertain
                    } else {
                        error
                    });
                }
            }
            let recorded = upload
                .app
                .as_ref()
                .ok_or(Error::ArtifactUnknown)?
                .use_for_driver(|reference| {
                    self.workspace
                        .journal()
                        .uploaded(self.workspace.owner(), operation.id, reference)
                });
            if recorded.is_err() || Instant::now() >= until {
                self.close_upload(&mut upload)?;
                return Err(Error::ArtifactUnknown);
            }
            let handle = Self::upload_handle(&mut upload);
            Ok(handle)
        })
    }
    fn uploaded(&self, handle: Uuid) -> Result<Arc<Mutex<Upload>>> {
        let uploads = self.uploads.lock().map_err(|_| Error::Unavailable)?;
        for upload in uploads.values() {
            if upload.lock().map_err(|_| Error::Unavailable)?.handles.contains(&handle) {
                return Ok(Arc::clone(upload));
            }
        }
        Err(Error::ArtifactUnknown)
    }
    fn close_upload(&self, upload: &mut Upload) -> Result<()> {
        Self::close_owned_upload(&self.workspace, self.backend.as_ref(), upload)
    }
    fn close_owned_upload(workspace: &Workspace, backend: &dyn Backend, upload: &mut Upload) -> Result<()> {
        if upload.cleanup == Cleanup::Complete {
            return Ok(());
        }
        if !upload.users.is_empty() || !upload.handles.is_empty() && Instant::now() < upload.deadline {
            return Ok(());
        }
        if upload.cleanup != Cleanup::Acknowledged {
            upload.cleanup = Cleanup::Closing;
            let _ = workspace.journal().uncertain(workspace.owner(), upload.id);
            let app = upload.app.as_ref().ok_or(Error::CleanupUncertain)?;
            // Exact in-memory ownership permits cleanup even after a failed receipt write.
            backend.delete(app).map_err(|_| Error::CleanupUncertain)?;
            upload.cleanup = Cleanup::Acknowledged;
        }
        workspace
            .journal()
            .confirm_released(workspace.owner(), upload.id)
            .map_err(|_| Error::CleanupUncertain)?;
        upload.app.take();
        upload.cleanup = Cleanup::Complete;
        Ok(())
    }
    /// # Errors
    /// Release the interactive handle; active lanes keep their original upload alive.
    pub fn release_upload(&self, id: Uuid) -> Result<()> {
        let upload = self.uploaded(id)?;
        let mut upload = upload.lock().map_err(|_| Error::Unavailable)?;
        upload.handles.remove(&id);
        let result = self.close_upload(&mut upload);
        if result.is_err() {
            upload.handles.insert(id);
        }
        result
    }
    /// # Errors
    /// Start one declared device with its own backend/tunnel resources, within the original deadline.
    pub fn create(&self, index: usize, artifact: Uuid, lifetime: Duration) -> Result<SessionHandle> {
        let _run = self.run.try_read().map_err(|_| Error::RunBusy)?;
        self.create_until(index, artifact, deadline(lifetime)?)
    }
    pub(crate) fn create_until(&self, index: usize, artifact: Uuid, until: Instant) -> Result<SessionHandle> {
        self.audit.execute(None, "session_create", || {
            let app = self.uploaded(artifact)?;
            self.create_app(index, app, until, false)
        })
    }
    fn create_app(
        &self,
        index: usize,
        app: Arc<Mutex<Upload>>,
        until: Instant,
        retained: bool,
    ) -> Result<SessionHandle> {
        let target = self
            .matrix
            .iter()
            .find(|row| row.matrix_index == index)
            .ok_or(horizon_app_testing::Error::MatrixUnavailable)?
            .device
            .clone();
        let declaration = self.contract.apps.get(&target.platform).ok_or(Error::Unavailable)?;
        let lane = self.reserve_lane(index, target.platform, app, until, retained)?;
        let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
        let result = (|| {
            if self.stopping.load(std::sync::atomic::Ordering::Acquire) || lane.cleanup != Cleanup::Active {
                return Err(Error::Cancelled);
            }
            let (tunnel, arguments) = crate::services::start(
                &self.local,
                &self.contract,
                &self.claims,
                lane.id,
                lane.deadline,
                &mut lane.resources,
            )?;
            lane.tunnel = Some(tunnel);
            let launch = lane
                .app
                .lock()
                .map_err(|_| Error::Unavailable)?
                .app
                .as_ref()
                .ok_or(Error::ArtifactUnknown)?
                .use_for_driver(|token| {
                    Launch::new(
                        target.clone(),
                        declaration.clone(),
                        arguments,
                        token.to_owned(),
                        horizon_app_provider::tunnel::local_identifier(tunnel),
                        lane.id.to_string(),
                        self.contract.evidence.clone(),
                    )
                })?;
            let transport = self.backend.driver()?;
            self.workspace
                .journal()
                .allocation_intent(self.workspace.owner(), lane.id)?;
            let budget = remaining(lane.deadline)?.min(Duration::from_secs(180));
            lane.attempted = true;
            lane.driver = Some(NativeDriver::allocate_with_timeout(transport, &launch, budget)?);
            let driver = lane.driver.as_ref().ok_or(Error::SessionUnknown)?;
            let mut recorded = Err(horizon_app_runtime::Error::OperationInvalid);
            driver.record_allocation(|reference| {
                recorded = self
                    .workspace
                    .journal()
                    .allocated(self.workspace.owner(), lane.id, reference);
            });
            recorded?;
            let mut verified = Err(Error::Unavailable);
            let upload = lane.app.lock().map_err(|_| Error::Unavailable)?;
            let app = Arc::clone(upload.app.as_ref().ok_or(Error::ArtifactUnknown)?);
            drop(upload);
            let budget = remaining(lane.deadline)?;
            driver.record_allocation(|reference| {
                verified = self.backend.verify(reference, &target, &app, lane.id, budget);
            });
            verified?;
            let mut observed = Err(Error::Unavailable);
            driver.record_allocation(|reference| {
                observed = (|| {
                    let protected = self
                        .lanes
                        .lock()
                        .map_err(|_| Error::Unavailable)?
                        .keys()
                        .copied()
                        .collect();
                    self.observations.retain(lane.id, reference, lane.deadline, &protected)
                })();
            });
            observed?;
            remaining(lane.deadline)?;
            Ok(SessionHandle {
                id: lane.id,
                target,
                remaining_seconds: lane.deadline.saturating_duration_since(Instant::now()).as_secs(),
            })
        })();
        if let Err(error) = &result {
            if let Error::LocalCleanupUncertain(operation) = error {
                lane.held_local = Some(*operation);
            }
            if lane.attempted && lane.driver.is_none() {
                self.workspace.journal().uncertain(self.workspace.owner(), lane.id)?;
            }
            let cleaned = self.close_lane(&mut lane);
            if cleaned.is_err() {
                return Err(Error::CleanupUncertain);
            }
        }
        result
    }
    fn reserve_lane(
        &self,
        index: usize,
        platform: Platform,
        app: Arc<Mutex<Upload>>,
        until: Instant,
        retained: bool,
    ) -> Result<Arc<Mutex<Lane>>> {
        let _admission = self.admission.lock().map_err(|_| Error::Unavailable)?;
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let mut lanes = self.lanes.lock().map_err(|_| Error::Unavailable)?;
        lanes.retain(|_, lane| lane.try_lock().map_or(true, |lane| lane.cleanup != Cleanup::Complete));
        if lanes.len() >= self.contract.max_parallel.min(2) {
            return Err(Error::AdmissionDeferred);
        }
        drop(lanes);
        let mut upload = app.lock().map_err(|_| Error::Unavailable)?;
        let until = until.min(upload.deadline);
        if upload.platform != platform
            || upload.cleanup != Cleanup::Active
            || upload.app.is_none()
            || upload.handles.is_empty() && !retained
        {
            return Err(Error::ArtifactUnknown);
        }
        self.durable_active(upload.id, Kind::Upload)?;
        remaining(until)?;
        let operation = self.workspace.start(Kind::Session, remaining(until)?)?;
        upload.users.insert(operation.id);
        drop(upload);
        let observation = remaining(until)
            .map_err(|_| horizon_app_runtime::Error::OperationExpired)
            .and_then(|budget| {
                self.backend
                    .capacity(budget)
                    .map_err(|_| horizon_app_runtime::Error::CapacityUnavailable)
            });
        let reserve = self
            .workspace
            .journal()
            .reserve_setup(self.workspace.owner(), operation.id, || {
                remaining(until).map_err(|_| horizon_app_runtime::Error::OperationExpired)?;
                observation
            });
        if let Err(error) = reserve {
            app.lock().map_err(|_| Error::Unavailable)?.users.remove(&operation.id);
            self.workspace
                .journal()
                .confirm_released(self.workspace.owner(), operation.id)?;
            return Err(if error == horizon_app_runtime::Error::CapacityUnavailable {
                Error::AdmissionDeferred
            } else {
                error.into()
            });
        }
        let lane = Arc::new(Mutex::new(Lane {
            id: operation.id,
            matrix_index: index,
            deadline: until,
            app,
            resources: Vec::new(),
            driver: None,
            tunnel: None,
            attempted: false,
            held_local: None,
            cleanup: Cleanup::Active,
            cleanup_inflight: false,
        }));
        self.lanes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(operation.id, Arc::clone(&lane));
        Ok(lane)
    }
    fn lane(&self, id: Uuid) -> Result<Arc<Mutex<Lane>>> {
        self.lanes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .get(&id)
            .cloned()
            .ok_or(Error::SessionUnknown)
    }
    fn durable_active(&self, id: Uuid, kind: Kind) -> Result<()> {
        let record = self.workspace.journal().status(self.workspace.owner(), id)?;
        if record.kind != kind || record.phase != Phase::Active {
            return Err(Error::CleanupUncertain);
        }
        Ok(())
    }
    fn active<'a>(&self, lane: &'a mut Lane) -> Result<&'a mut NativeDriver> {
        if lane.cleanup != Cleanup::Active {
            return Err(Error::SessionUnknown);
        }
        self.durable_active(lane.id, Kind::Session)?;
        remaining(lane.deadline)?;
        for resource in &lane.resources {
            if resource.id == lane.tunnel.ok_or(Error::SessionUnknown)? {
                resource.tunnel_status()?;
            }
        }
        let driver = lane.driver.as_mut().ok_or(Error::SessionUnknown)?;
        driver.limit_to_deadline(lane.deadline)?;
        Ok(driver)
    }
    /// # Errors
    /// A snapshot invalidates earlier refs only in this lane.
    pub fn snapshot(&self, id: Uuid) -> Result<Snapshot> {
        self.audit.execute(Some(id), "snapshot", || {
            let lane = self.lane(id)?;
            let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
            self.active(&mut lane)?.snapshot().map_err(Error::from)
        })
    }
    /// # Errors
    /// Typed actions share the same lane lock and deadline as every interface.
    pub fn act(&self, id: Uuid, action: &Action) -> Result<()> {
        self.audit.execute(Some(id), action.audit_name(), || {
            let lane = self.lane(id)?;
            let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
            self.active(&mut lane)?.act(action).map_err(Error::from)
        })
    }
    /// # Errors
    /// Waits serialize only the selected lane.
    pub fn wait(&self, id: Uuid, target: &Target, state: State, timeout: Duration) -> Result<()> {
        self.audit.execute(Some(id), "wait", || {
            let lane = self.lane(id)?;
            let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
            self.active(&mut lane)?
                .wait(target, state, timeout)
                .map_err(Error::from)
        })
    }
    /// # Errors
    /// Return bounded screenshot bytes only to trusted evidence/view adapters.
    pub fn screenshot(&self, id: Uuid) -> Result<Vec<u8>> {
        let lane = self.lane(id)?;
        let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
        self.active(&mut lane)?.screenshot().map_err(Error::from)
    }
    /// # Errors
    /// Evidence reads use only the original verified driver ID, including after acknowledged closure.
    pub fn media(&self, id: Uuid, kind: horizon_app_provider::media::Kind) -> Result<Vec<u8>> {
        self.media_with_timeout(id, kind, Duration::from_secs(30))
    }
    pub(crate) fn media_with_timeout(
        &self,
        id: Uuid,
        kind: horizon_app_provider::media::Kind,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        self.export_media(id, kind, timeout, Ok)
    }
    pub(crate) fn export_media<T>(
        &self,
        id: Uuid,
        kind: horizon_app_provider::media::Kind,
        timeout: Duration,
        export: impl FnOnce(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        self.audit.execute(Some(id), "media", || {
            let _read = MediaRead::acquire(&self.media_reads)?;
            let reference = self.observations.reference(id)?;
            export(self.backend.media(&reference, kind, timeout)?)
        })
    }
    pub(crate) fn session_link(&self, id: Uuid, timeout: Duration) -> Result<String> {
        self.audit.execute(Some(id), "session_link", || {
            let reference = self.observations.reference(id)?;
            self.backend.session_link(&reference, timeout)
        })
    }
    /// # Errors
    /// Provider recording is selected at allocation by the project's evidence policy.
    pub fn video_enabled(&self, id: Uuid) -> Result<bool> {
        let _reference = self.observations.reference(id)?;
        Ok(self.contract.evidence.video)
    }
    /// # Errors
    /// Return the owned tunnel's redacted status.
    pub fn tunnel_status(&self, id: Uuid) -> Result<horizon_app_provider::tunnel::Status> {
        let lane = self.lane(id)?;
        let lane = lane.lock().map_err(|_| Error::Unavailable)?;
        if lane.cleanup != Cleanup::Active {
            return Err(Error::SessionUnknown);
        }
        self.durable_active(lane.id, Kind::Session)?;
        remaining(lane.deadline)?;
        lane.resources
            .iter()
            .find(|resource| Some(resource.id) == lane.tunnel)
            .ok_or(Error::SessionUnknown)?
            .tunnel_status()
    }
    fn close_lane(&self, lane: &mut Lane) -> Result<()> {
        Self::close_owned_lane(&self.workspace, self.backend.as_ref(), &self.local, &self.claims, lane)
    }
    fn close_owned_lane(
        workspace: &Workspace,
        backend: &dyn Backend,
        local: &Local,
        claims: &Mutex<BTreeMap<u16, Uuid>>,
        lane: &mut Lane,
    ) -> Result<()> {
        if lane.cleanup == Cleanup::Complete {
            return Ok(());
        }
        if let Some(operation) = lane.held_local {
            local.recover(operation)?;
            lane.held_local = None;
        }
        if lane.cleanup != Cleanup::Acknowledged {
            lane.cleanup = Cleanup::Closing;
            let _ = workspace.journal().uncertain(workspace.owner(), lane.id);
            if let Some(driver) = &mut lane.driver {
                if driver.close().is_err() {
                    let mut confirmed = Err(Error::CleanupUncertain);
                    driver.record_allocation(|reference| {
                        confirmed = backend.confirmed_closed(reference);
                    });
                    if !confirmed.map_err(|_| Error::CleanupUncertain)? {
                        return Err(Error::CleanupUncertain);
                    }
                }
            } else if lane.attempted {
                return Err(Error::CleanupUncertain);
            }
            lane.cleanup = Cleanup::Acknowledged;
        }
        let recorded = workspace.journal().confirm_released(workspace.owner(), lane.id);
        // Known native closure must still stop exact retained guardians if persistence fails.
        let stopped = crate::services::close(&mut lane.resources);
        if recorded.is_err() || stopped.is_err() {
            return Err(Error::CleanupUncertain);
        }
        claims
            .lock()
            .map_err(|_| Error::Unavailable)?
            .retain(|_, owner| *owner != lane.id);
        let mut upload = lane.app.lock().map_err(|_| Error::Unavailable)?;
        upload.users.remove(&lane.id);
        lane.driver.take();
        lane.cleanup = Cleanup::Complete;
        if upload.handles.is_empty() || Instant::now() >= upload.deadline {
            Self::close_owned_upload(workspace, backend, &mut upload)?;
        }
        Ok(())
    }
    /// # Errors
    /// Exact native DELETE must be acknowledged before backend and tunnel cleanup.
    pub fn close(&self, id: Uuid) -> Result<()> {
        self.audit.execute(Some(id), "session_close", || {
            if let Ok(lane) = self.lane(id) {
                let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
                return self.close_lane(&mut lane);
            }
            let record = self.workspace.journal().status(self.workspace.owner(), id)?;
            if record.kind == Kind::Session && record.phase == Phase::Complete {
                Ok(())
            } else {
                Err(Error::SessionUnknown)
            }
        })
    }
    /// # Errors
    /// Reset replaces an exact closed allocation using the same upload and original deadline.
    pub fn reset(&self, id: Uuid) -> Result<SessionHandle> {
        let _run = self.run.try_read().map_err(|_| Error::RunBusy)?;
        self.reset_for_run(id)
    }
    pub(crate) fn reset_for_run(&self, id: Uuid) -> Result<SessionHandle> {
        self.audit.execute(Some(id), "reset", || {
            let lane = self.lane(id)?;
            let mut lane = lane.lock().map_err(|_| Error::Unavailable)?;
            self.active(&mut lane)?;
            let app = Arc::clone(&lane.app);
            // Temporarily retain the upload across exact native/service cleanup.
            let reset = Uuid::new_v4();
            app.lock().map_err(|_| Error::Unavailable)?.users.insert(reset);
            let index = lane.matrix_index;
            let until = lane.deadline;
            if let Err(error) = self.close_lane(&mut lane) {
                app.lock().map_err(|_| Error::Unavailable)?.users.remove(&reset);
                return Err(error);
            }
            drop(lane);
            let result = self.create_app(index, Arc::clone(&app), until, true);
            let mut upload = app.lock().map_err(|_| Error::Unavailable)?;
            upload.users.remove(&reset);
            if upload.handles.is_empty() {
                self.close_upload(&mut upload)?;
            }
            result
        })
    }
    /// # Errors
    /// One bounded expiry worker serves the whole controller; it retains no idle actor ownership.
    pub(crate) fn arm(self: &Arc<Self>) -> Result<()> {
        use std::sync::atomic::Ordering;
        if self.expiry_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let controller = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("native-run-expiry".into())
            .spawn(move || {
                loop {
                    let Some(controller) = controller.upgrade() else {
                        break;
                    };
                    let _ = controller.expire();
                    drop(controller);
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    /// One host sweeper calls this; physical guardians also retain their independent crash lifetimes.
    pub(crate) fn expire(&self) -> Result<()> {
        let lanes = self
            .lanes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for lane in lanes {
            let dispatch = lane.try_lock().is_ok_and(|mut owned| {
                if owned.cleanup == Cleanup::Complete || owned.cleanup_inflight || Instant::now() < owned.deadline {
                    return false;
                }
                owned.cleanup_inflight = true;
                true
            });
            if !dispatch {
                continue;
            }
            let workspace = Arc::clone(&self.workspace);
            let backend = Arc::clone(&self.backend);
            let local = Arc::clone(&self.local);
            let claims = Arc::clone(&self.claims);
            let task = Arc::clone(&lane);
            if std::thread::Builder::new()
                .name("native-lane-expiry".into())
                .spawn(move || {
                    if let Ok(mut owned) = task.lock() {
                        let _ = Self::close_owned_lane(&workspace, backend.as_ref(), &local, &claims, &mut owned);
                        owned.cleanup_inflight = false;
                    }
                })
                .is_err()
                && let Ok(mut owned) = lane.lock()
            {
                owned.cleanup_inflight = false;
            }
        }
        let uploads = self
            .uploads
            .lock()
            .map_err(|_| Error::Unavailable)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for upload in uploads {
            let dispatch = upload.try_lock().is_ok_and(|mut owned| {
                if owned.cleanup == Cleanup::Complete || owned.cleanup_inflight || Instant::now() < owned.deadline {
                    return false;
                }
                owned.handles.clear();
                if !owned.users.is_empty() {
                    return false;
                }
                owned.cleanup_inflight = true;
                true
            });
            if !dispatch {
                continue;
            }
            let workspace = Arc::clone(&self.workspace);
            let backend = Arc::clone(&self.backend);
            let task = Arc::clone(&upload);
            if std::thread::Builder::new()
                .name("native-upload-expiry".into())
                .spawn(move || {
                    if let Ok(mut owned) = task.lock() {
                        let _ = Self::close_owned_upload(&workspace, backend.as_ref(), &mut owned);
                        owned.cleanup_inflight = false;
                    }
                })
                .is_err()
                && let Ok(mut owned) = upload.lock()
            {
                owned.cleanup_inflight = false;
            }
        }
        Ok(())
    }
}
impl Drop for Actor {
    fn drop(&mut self) {
        let lanes = self.lanes.lock().map(|lanes| lanes.clone()).unwrap_or_default();
        let uploads = self.uploads.lock().map(|uploads| uploads.clone()).unwrap_or_default();
        let mut held = BTreeMap::new();
        for (id, lane) in lanes {
            let failed = match lane.lock() {
                Ok(mut owned) => self.close_lane(&mut owned).is_err(),
                Err(_) => true,
            };
            if failed && !self.retiring {
                held.insert(id, Arc::clone(&lane));
            }
        }
        let mut failed_upload = false;
        for upload in uploads.values() {
            if let Ok(mut upload) = upload.lock() {
                upload.handles.clear();
                failed_upload |= self.close_upload(&mut upload).is_err();
            } else {
                failed_upload = true;
            }
        }
        if held.is_empty() && !failed_upload || self.retiring {
            return;
        }
        let mut retired = Self::from_backend(
            Arc::clone(&self.workspace),
            Arc::clone(&self.backend),
            Arc::clone(&self.local),
            self.contract.clone(),
            self.matrix.clone(),
        );
        retired.retiring = true;
        let until = held
            .values()
            .map(|lane| lane.lock().unwrap_or_else(std::sync::PoisonError::into_inner).deadline)
            .chain(uploads.values().map(|upload| {
                upload
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .deadline
            }))
            .max()
            .unwrap_or_else(Instant::now);
        *retired
            .lanes
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = held.clone();
        *retired
            .uploads
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = uploads;
        if std::thread::Builder::new()
            .name("native-run-retirement".into())
            .spawn(move || {
                while Instant::now() < until {
                    let _ = retired.expire();
                    std::thread::sleep(
                        until
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(250)),
                    );
                }
                let _ = retired.expire();
            })
            .is_err()
        {
            // Physical guardians retain their original TTL even if a host thread cannot start.
            // Keep failed native ownership alive rather than prematurely closing its service pipes.
            std::mem::forget(held);
        }
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

// Media reads never acquire lane locks or renew resources. Refuse bursts before allocating bodies.
struct MediaRead<'a>(&'a std::sync::atomic::AtomicUsize);
impl<'a> MediaRead<'a> {
    fn acquire(count: &'a std::sync::atomic::AtomicUsize) -> Result<Self> {
        use std::sync::atomic::Ordering;
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value < 2).then_some(value + 1)
            })
            .map_err(|_| Error::MediaBusy)?;
        Ok(Self(count))
    }
}
impl Drop for MediaRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
#[cfg(test)]
mod media_admission_tests {
    use super::*;
    #[test]
    fn two_reads_are_admitted_and_errors_release_the_slot() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let first = MediaRead::acquire(&count).unwrap();
        let second = MediaRead::acquire(&count).unwrap();
        assert!(matches!(MediaRead::acquire(&count), Err(Error::MediaBusy)));
        drop(first);
        let replacement = MediaRead::acquire(&count).unwrap();
        assert!(matches!(MediaRead::acquire(&count), Err(Error::MediaBusy)));
        drop((second, replacement));
        assert_eq!(count.load(std::sync::atomic::Ordering::Acquire), 0);
    }
}
