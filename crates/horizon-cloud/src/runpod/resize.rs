//! CPU replacement retains an identity-verified network workspace and fences allocation.
use super::{RunPod, volumes::Volume};
use crate::{Cancellation, CloudError, CreateState, Progress, Worker, WorkerSpec, valid_id};
use serde::{Deserialize, Serialize};
use std::time::Duration;

type Result<T> = std::result::Result<T, CloudError>;

/// The caller holds the cloud lock and retains this journal until its profile and
/// storage binding have committed. Replacing compute never initializes workspace files.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    current: WorkerSpec,
    next: WorkerSpec,
    original_worker: String,
    volume: Volume,
    phase: Phase,
    creation: CreateState,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Terminating,
    Creating,
    Completed,
}

impl Replacement {
    /// # Errors
    /// Accepts only CPU/memory changes on a bound CPU worker with retained network storage.
    pub fn new(current: WorkerSpec, next: WorkerSpec, worker_id: String, volume: Volume) -> Result<Self> {
        let intent = Self {
            current,
            next,
            original_worker: worker_id,
            volume,
            phase: Phase::Prepared,
            creation: CreateState::Prepared,
        };
        intent.validate()?;
        Ok(intent)
    }

    #[must_use]
    pub fn specification(&self) -> &WorkerSpec {
        &self.next
    }

    #[must_use]
    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    #[must_use]
    pub fn completed(&self) -> bool {
        self.phase == Phase::Completed
    }

    /// # Errors
    /// Confirms the journal still names the caller's original worker and storage.
    pub fn verify_origin(&self, spec: &WorkerSpec, worker_id: &str, volume: &Volume) -> Result<()> {
        self.validate()?;
        if self.current != *spec || self.original_worker != worker_id || self.volume != *volume {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        self.current.validate()?;
        self.next.validate_request()?;
        let mut expected = self.current.clone();
        expected.profile.cpu = self.next.profile.cpu;
        expected.profile.memory_gb = self.next.profile.memory_gb;
        expected.cpu_flavors.clone_from(&self.next.cpu_flavors);
        if self.current.profile.gpu
            || expected != self.next
            || (self.current.profile.cpu, self.current.profile.memory_gb)
                == (self.next.profile.cpu, self.next.profile.memory_gb)
            || !valid_id(&self.original_worker)
            || self.volume.tier.is_none()
            || matches!(self.creation, CreateState::Terminated { .. })
            || (matches!(self.phase, Phase::Prepared | Phase::Terminating) && self.creation != CreateState::Prepared)
            || (self.phase == Phase::Completed && !matches!(self.creation, CreateState::Bound { .. }))
        {
            return Err(CloudError::Invalid(
                "Compute replacement requires only a new CPU size on retained network storage",
            ));
        }
        if let CreateState::Bound { worker_id } = &self.creation
            && (!valid_id(worker_id) || *worker_id == self.original_worker)
        {
            return Err(CloudError::IdentityMismatch);
        }
        self.volume.verify_worker_spec(&self.current)?;
        self.volume.verify_worker_spec(&self.next)
    }

    fn transition(
        &mut self,
        phase: Phase,
        creation: CreateState,
        persist: &mut impl FnMut(&Self) -> Result<()>,
    ) -> Result<()> {
        let next = Self {
            phase,
            creation,
            ..self.clone()
        };
        next.validate()?;
        persist(&next)?;
        *self = next;
        Ok(())
    }
}

impl RunPod {
    /// Replace CPU compute while retaining the exact network volume. Termination
    /// intent is durable before DELETE; a lost creation response never authorizes
    /// another POST. The caller must persist every transition before returning.
    /// # Errors
    /// Rejects changed identities, resources, mounts, concurrent attachments and
    /// uncertain provider state. The retained journal supports explicit recovery.
    pub fn resize_cpu(
        &self,
        intent: &mut Replacement,
        cancel: &Cancellation,
        mut persist: impl FnMut(&Replacement) -> Result<()>,
        mut progress: impl FnMut(Progress),
    ) -> Result<Worker> {
        intent.validate()?;
        cancel.check()?;
        self.verify_resize_volume(&intent.volume, cancel)?;
        if matches!(intent.phase, Phase::Prepared | Phase::Terminating) {
            let current = self.inspect(&intent.original_worker, cancel)?;
            if let Some(mut worker) = current {
                self.verify_resize_worker(&intent.current, &mut worker, &intent.volume, cancel)?;
                if intent.phase == Phase::Prepared && !worker.is_starting_or_running() {
                    return Err(CloudError::Invalid("The original worker is not running"));
                }
            } else if intent.phase == Phase::Prepared {
                return Err(CloudError::WorkerLost);
            }
            if intent.phase == Phase::Prepared {
                intent.transition(Phase::Terminating, CreateState::Prepared, &mut persist)?;
            }
            let mut termination = CreateState::Bound {
                worker_id: intent.original_worker.clone(),
            };
            self.terminate_with_progress(
                &intent.current.clone(),
                &mut termination,
                cancel,
                |_| intent.transition(Phase::Creating, CreateState::Prepared, &mut persist),
                &mut progress,
            )?;
        }
        // Completed calls still confirm the current worker rather than return a stale snapshot.
        if intent.creation == CreateState::Prepared
            && self.volume_attached_except(&intent.volume.id, None, cancel, &mut progress)?
        {
            return Err(CloudError::IdentityMismatch);
        }
        let mut creation = intent.creation.clone();
        let mut worker = self.ensure_with_volume(
            &intent.next.clone(),
            &mut creation,
            Some(&intent.volume.clone()),
            cancel,
            |next| intent.transition(Phase::Creating, next.clone(), &mut persist),
            &mut progress,
        )?;
        if worker.id == intent.original_worker {
            return Err(CloudError::IdentityMismatch);
        }
        self.verify_resize_worker(&intent.next, &mut worker, &intent.volume, cancel)?;
        intent.transition(Phase::Completed, creation, &mut persist)?;
        Ok(worker)
    }

    fn verify_resize_volume(&self, volume: &Volume, cancel: &Cancellation) -> Result<()> {
        if self.inspect_volume(&volume.id, cancel)?.as_ref() != Some(volume) {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }

    fn verify_resize_worker(
        &self,
        spec: &WorkerSpec,
        worker: &mut Worker,
        volume: &Volume,
        cancel: &Cancellation,
    ) -> Result<()> {
        worker.verify(spec)?;
        self.confirm_workspace_mount(worker, volume, cancel, Duration::from_secs(30))?;
        worker.verify_resources_with_volume(spec, Some(volume))?;
        if self.volume_attached_except(&volume.id, Some(&worker.id), cancel, &mut |_| {})? {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}
