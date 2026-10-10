//! Bounded expiry and detached controller retirement.
use super::{Actor, Arc, BTreeMap, Cleanup, Duration, Error, Instant, Result};
use crate::lifecycle::{Lock as HostLock, Operation as HostOperation};

impl Actor {
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
            .map_err(|error| Error::host_io(HostOperation::Expiry, &error))?;
        Ok(())
    }
    /// One host sweeper calls this; physical guardians also retain their independent crash lifetimes.
    pub(crate) fn expire(&self) -> Result<()> {
        let lanes = self
            .lanes
            .lock()
            .map_err(|_| Error::host_lock(HostLock::Lanes))?
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
            .map_err(|_| Error::host_lock(HostLock::Uploads))?
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
