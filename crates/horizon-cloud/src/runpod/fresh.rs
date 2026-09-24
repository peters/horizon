//! Ephemeral direct-create witnesses. Persisted receipts cannot recreate these.
use super::{
    Provisioning, RunPod,
    volumes::{Spec, State, Volume},
};
use crate::{Cancellation, CloudError, CreateState, Worker, WorkerSpec};
use std::time::Duration;

type Result<T> = std::result::Result<T, CloudError>;

/// A direct, durably recorded volume creation in this process. Not serializable,
/// cloneable or constructible by callers, and not permission to initialize it.
pub struct FreshVolume<'a> {
    provider: &'a RunPod,
    volume: Volume,
}

/// Direct worker creation using the newly created volume and the same account.
pub struct CreatedAllocation<'a> {
    provider: &'a RunPod,
    volume: Volume,
    worker: Worker,
    spec: WorkerSpec,
}

/// A live first-attachment observation, consumed by the owning host coordinator.
/// Runtime mount, controller, pristine-state and one-shot host fences still apply.
pub struct FirstAttachment {
    volume: Volume,
    worker: Worker,
    spec: WorkerSpec,
}

impl RunPod {
    /// Caller must anchor Prepared and retain its canonical owner lock throughout
    /// creation and initialization. A restart may reconcile, never call this again.
    /// # Errors
    /// Refuses existing storage, uncertain creation and failed durable transitions.
    pub fn create_fresh_volume(
        &self,
        spec: &Spec,
        cancel: &Cancellation,
        persist: impl FnMut(&State) -> Result<()>,
    ) -> Result<FreshVolume<'_>> {
        let mut state = State::Prepared;
        let volume = self.ensure_volume(spec, &mut state, cancel, persist)?;
        if state.creation_receipt(spec)?.is_none() {
            return Err(CloudError::CreationUnresolved);
        }
        Ok(FreshVolume { provider: self, volume })
    }
}

impl<'a> FreshVolume<'a> {
    #[must_use]
    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Consumes this direct-create witness. A matching existing worker cannot be
    /// adopted, and an observed existing volume attachment blocks the request.
    /// # Errors
    /// Reports identity conflicts, uncertain POSTs and failed anchored saves.
    pub fn create_worker(
        self,
        spec: &WorkerSpec,
        cancel: &Cancellation,
        persist: impl FnMut(&CreateState) -> Result<()>,
    ) -> Result<CreatedAllocation<'a>> {
        self.volume.verify_worker_spec(spec)?;
        if spec.profile.gpu || spec.startup_metadata.is_none() {
            return Err(CloudError::Invalid("Fresh allocation requires CPU startup metadata"));
        }
        if self
            .provider
            .volume_attached_except(&self.volume.id, None, cancel, &mut |_| {})?
        {
            return Err(CloudError::IdentityMismatch);
        }
        let worker = self.provider.provision(
            spec,
            &mut CreateState::Prepared,
            Provisioning::New(&self.volume),
            cancel,
            persist,
            |_| {},
        )?;
        Ok(CreatedAllocation {
            provider: self.provider,
            volume: self.volume,
            worker,
            spec: spec.clone(),
        })
    }
}

impl CreatedAllocation<'_> {
    #[must_use]
    pub fn worker(&self) -> &Worker {
        &self.worker
    }

    /// Read-only readiness observation; never creates a replacement or new proof.
    /// # Errors
    /// Refuses a missing worker or changed immutable specification.
    pub fn observe(&self, cancel: &Cancellation) -> Result<Worker> {
        let worker = self
            .provider
            .inspect(&self.worker.id, cancel)?
            .ok_or(CloudError::WorkerLost)?;
        worker.verify(&self.spec)?;
        Ok(worker)
    }

    /// Consume the witness only after readiness. The current provider API must
    /// report exactly the created volume at /workspace, with no sibling attachment.
    /// # Errors
    /// An uncertain observation consumes the witness and cannot authorize bootstrap.
    pub fn confirm(self, cancel: &Cancellation) -> Result<FirstAttachment> {
        let mut worker = self.observe(cancel)?;
        if worker.desired_status != "RUNNING" {
            return Err(CloudError::IdentityMismatch);
        }
        self.provider
            .confirm_workspace_mount(&mut worker, &self.volume, cancel, Duration::from_secs(30))?;
        if self
            .provider
            .volume_attached_except(&self.volume.id, Some(&worker.id), cancel, &mut |_| {})?
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(FirstAttachment {
            volume: self.volume,
            worker,
            spec: self.spec,
        })
    }
}

impl FirstAttachment {
    #[must_use]
    pub fn volume(&self) -> &Volume {
        &self.volume
    }
    #[must_use]
    pub fn worker(&self) -> &Worker {
        &self.worker
    }
    #[must_use]
    pub fn spec(&self) -> &WorkerSpec {
        &self.spec
    }
}
