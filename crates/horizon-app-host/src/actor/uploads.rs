//! Immutable upload handles and exact upload retirement.
use super::{
    Actor, Arc, Artifact, ArtifactHandle, BTreeSet, Backend, Cleanup, Duration, Error, Instant, Kind, Mutex, Platform,
    Result, Upload, Uuid, Workspace, deadline, remaining,
};

impl Actor {
    pub(super) fn upload_handle(upload: &mut Upload) -> ArtifactHandle {
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
            self.prune_completed_provider()?;
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
    pub(super) fn uploaded(&self, handle: Uuid) -> Result<Arc<Mutex<Upload>>> {
        let uploads = self.uploads.lock().map_err(|_| Error::Unavailable)?;
        for upload in uploads.values() {
            if upload.lock().map_err(|_| Error::Unavailable)?.handles.contains(&handle) {
                return Ok(Arc::clone(upload));
            }
        }
        Err(Error::ArtifactUnknown)
    }
    pub(super) fn close_upload(&self, upload: &mut Upload) -> Result<()> {
        Self::close_owned_upload(&self.workspace, self.backend.as_ref(), upload)
    }
    pub(super) fn close_owned_upload(workspace: &Workspace, backend: &dyn Backend, upload: &mut Upload) -> Result<()> {
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
}
