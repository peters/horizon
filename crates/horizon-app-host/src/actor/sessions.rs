//! Device admission and serialized native operations.
use super::{
    Action, Actor, Arc, Cleanup, Duration, Error, Instant, Kind, Lane, Launch, Mutex, NativeDriver, Platform, Result,
    SessionHandle, Snapshot, State, Target, Upload, Uuid, deadline, remaining,
};

impl Actor {
    /// # Errors
    /// Start one declared device with its own backend/tunnel resources, within the original deadline.
    pub fn create(&self, index: usize, artifact: Uuid, lifetime: Duration) -> Result<SessionHandle> {
        let _run = self.run.try_read().map_err(|_| Error::RunBusy)?;
        self.create_until(index, artifact, deadline(lifetime)?)
    }
    pub(crate) fn create_until(&self, index: usize, artifact: Uuid, until: Instant) -> Result<SessionHandle> {
        self.create_until_outcome(index, artifact, until).0
    }
    pub(crate) fn create_until_outcome(
        &self,
        index: usize,
        artifact: Uuid,
        until: Instant,
    ) -> (Result<SessionHandle>, bool) {
        let mut cleanup_confirmed = true;
        let result = self.audit.execute(None, "session_create", || {
            self.prune_completed_provider()?;
            let app = self.uploaded(artifact)?;
            self.create_app(index, app, until, false, &mut cleanup_confirmed)
        });
        (result, cleanup_confirmed)
    }
    pub(super) fn create_app(
        &self,
        index: usize,
        app: Arc<Mutex<Upload>>,
        until: Instant,
        retained: bool,
        cleanup_confirmed: &mut bool,
    ) -> Result<SessionHandle> {
        let target = self
            .matrix
            .iter()
            .find(|row| row.matrix_index == index)
            .ok_or(horizon_app_testing::Error::MatrixUnavailable)?
            .device
            .clone();
        let declaration = self.contract.apps.get(&target.platform).ok_or(Error::Unavailable)?;
        let lane = self.reserve_lane(index, target.platform, app, until, retained, cleanup_confirmed)?;
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
            if self.close_lane(&mut lane).is_err() {
                return Err(Error::CleanupUncertain);
            }
            *cleanup_confirmed = true;
        }
        result
    }
    pub(super) fn reserve_lane(
        &self,
        index: usize,
        platform: Platform,
        app: Arc<Mutex<Upload>>,
        until: Instant,
        retained: bool,
        cleanup_confirmed: &mut bool,
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
        *cleanup_confirmed = false;
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
            *cleanup_confirmed = true;
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
            view_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }));
        self.lanes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(operation.id, Arc::clone(&lane));
        Ok(lane)
    }
    pub(super) fn lane(&self, id: Uuid) -> Result<Arc<Mutex<Lane>>> {
        self.lanes
            .lock()
            .map_err(|_| Error::Unavailable)?
            .get(&id)
            .cloned()
            .ok_or(Error::SessionUnknown)
    }
    // A read-only viewer keeps this closure signal even after completed lanes are pruned.
    pub(crate) fn view_lifetime(&self, id: Uuid) -> Result<Arc<std::sync::atomic::AtomicBool>> {
        let lane = self.lane(id)?;
        let lane = lane.lock().map_err(|_| Error::Unavailable)?;
        if lane.cleanup != Cleanup::Active || lane.view_closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::SessionUnknown);
        }
        remaining(lane.deadline)?;
        Ok(Arc::clone(&lane.view_closed))
    }
    pub(super) fn active<'a>(&self, lane: &'a mut Lane) -> Result<&'a mut NativeDriver> {
        if lane.cleanup != Cleanup::Active {
            return Err(Error::SessionUnknown);
        }
        self.durable_active(lane.id, Kind::Session)?;
        remaining(lane.deadline)?;
        let tunnel = lane.tunnel.ok_or(Error::SessionUnknown)?;
        let resource = lane
            .resources
            .iter()
            .find(|resource| resource.id == tunnel)
            .ok_or(Error::SessionUnknown)?;
        if !resource.tunnel_status()?.ready {
            return Err(horizon_app_provider::Error::TunnelStartFailed.into());
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
    /// # Errors
    /// Reset replaces an exact closed allocation using the same upload and original deadline.
    pub fn reset(&self, id: Uuid) -> Result<SessionHandle> {
        let _run = self.run.try_read().map_err(|_| Error::RunBusy)?;
        self.reset_for_run_outcome(id).0
    }
    pub(crate) fn reset_for_run_outcome(&self, id: Uuid) -> (Result<SessionHandle>, bool) {
        let mut cleanup_confirmed = true;
        let result = self.audit.execute(Some(id), "reset", || {
            self.prune_completed_provider()?;
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
            let result = self.create_app(index, Arc::clone(&app), until, true, &mut cleanup_confirmed);
            let mut upload = app.lock().map_err(|_| Error::Unavailable)?;
            upload.users.remove(&reset);
            if upload.handles.is_empty() {
                self.close_upload(&mut upload)?;
            }
            result
        });
        (result, cleanup_confirmed)
    }
}
