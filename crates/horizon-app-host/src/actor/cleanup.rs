//! Acknowledged resource closure and completed-operation pruning.
use super::{
    Actor, BTreeMap, BTreeSet, Backend, Cleanup, Error, Instant, Kind, Lane, Local, Mutex, Phase, Result, Uuid,
    Workspace,
};

impl Actor {
    /// # Errors
    /// Explicit normal-host shutdown does not depend on the last viewer/request Arc disappearing.
    pub fn shutdown(&self) -> Result<()> {
        self.stopping.store(true, std::sync::atomic::Ordering::Release);
        // An in-flight matrix owns its handles through report cleanup. Wait before clearing them.
        let _run = self.run.write().map_err(|_| Error::CleanupUncertain)?;
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
    pub(super) fn prune_completed_provider(&self) -> Result<()> {
        let _admission = self.admission.lock().map_err(|_| Error::Unavailable)?;
        let mut uploads = self.uploads.lock().map_err(|_| Error::Unavailable)?;
        let mut lanes = self.lanes.lock().map_err(|_| Error::Unavailable)?;
        uploads.retain(|_, value| {
            value
                .try_lock()
                .map_or(true, |value| value.cleanup != Cleanup::Complete)
        });
        lanes.retain(|_, value| {
            value
                .try_lock()
                .map_or(true, |value| value.cleanup != Cleanup::Complete)
        });
        let protected = uploads.keys().chain(lanes.keys()).copied().collect::<BTreeSet<_>>();
        let mut completed = self.workspace.journal().completed(self.workspace.owner())?;
        completed
            .retain(|record| matches!(record.kind, Kind::Upload | Kind::Session) && !protected.contains(&record.id));
        completed.sort_by_key(|record| (record.created_seconds, record.id));
        for record in completed.iter().take(completed.len().saturating_sub(32)) {
            self.workspace.journal().retire(self.workspace.owner(), record.id)?;
        }
        Ok(())
    }

    pub(super) fn close_lane(&self, lane: &mut Lane) -> Result<()> {
        Self::close_owned_lane(&self.workspace, self.backend.as_ref(), &self.local, &self.claims, lane)
    }
    pub(super) fn close_owned_lane(
        workspace: &Workspace,
        backend: &dyn Backend,
        local: &Local,
        claims: &Mutex<BTreeMap<u16, Uuid>>,
        lane: &mut Lane,
    ) -> Result<()> {
        lane.view_closed.store(true, std::sync::atomic::Ordering::Release);
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
}
