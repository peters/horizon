//! Recoverable capacity growth; filesystem expansion remains a separate worker step.
use super::{Hetzner, SIZE_GB, Volume};
use crate::{Cancellation, CloudError, CreateState};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, CloudError>;

/// The caller holds the ownership lock and retains this intent until provider
/// capacity, the worker filesystem and its durable profile have all committed.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Growth {
    operation: String,
    original: Volume,
    requested_size: u32,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Requested,
    Confirmed,
}

impl Growth {
    /// # Errors
    /// Requires an available owned volume, its bound allocation and a larger size.
    pub fn new(operation: &str, state: &CreateState, volume: &Volume, requested_size: u32) -> Result<Self> {
        if !matches!(state, CreateState::Bound { worker_id } if *worker_id == volume.id.to_string()) {
            return Err(CloudError::IdentityMismatch);
        }
        let growth = Self {
            operation: operation.into(),
            original: volume.clone(),
            requested_size,
            phase: Phase::Prepared,
        };
        growth.validate()?;
        Ok(growth)
    }

    #[must_use]
    pub const fn requested_size(&self) -> u32 {
        self.requested_size
    }

    /// Provider capacity only; this does not claim the filesystem has expanded.
    #[must_use]
    pub fn confirmed(&self) -> bool {
        self.phase == Phase::Confirmed
    }

    fn validate(&self) -> Result<()> {
        self.original.verify(&self.operation)?;
        if self.original.id == 0
            || self.original.status != "available"
            || !SIZE_GB.contains(&self.requested_size)
            || self.requested_size <= self.original.size
        {
            return Err(CloudError::Invalid(
                "Storage growth requires an available volume and a larger capacity",
            ));
        }
        Ok(())
    }

    fn verify_observed(&self, volume: &Volume) -> Result<()> {
        let mut expected = self.original.clone();
        if volume.size == self.requested_size {
            expected.size = self.requested_size;
        }
        if *volume != expected {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }

    fn transition(&mut self, phase: Phase, persist: &mut impl FnMut(&Self) -> Result<()>) -> Result<()> {
        let next = Self { phase, ..self.clone() };
        persist(&next)?;
        *self = next;
        Ok(())
    }
}

impl Hetzner {
    /// Increase one recorded block volume behind a durable absolute-size intent.
    /// Retries inspect first and never repeat an already-confirmed increase. A lost
    /// action response can resend the same target, never allocate a second volume.
    /// # Errors
    /// Rejects identity, attachment or capacity drift, shrinking and persistence failure.
    /// The caller must subsequently grow the filesystem on its verified worker.
    pub fn grow_volume(
        &self,
        growth: &mut Growth,
        cancel: &Cancellation,
        mut persist: impl FnMut(&Growth) -> Result<()>,
    ) -> Result<Volume> {
        growth.validate()?;
        cancel.check()?;
        let current = self.observe_growth(growth, cancel)?;
        if current.size == growth.requested_size {
            growth.transition(Phase::Confirmed, &mut persist)?;
            return Ok(current);
        }
        if growth.confirmed() {
            return Err(CloudError::IdentityMismatch);
        }
        growth.transition(Phase::Requested, &mut persist)?;
        self.volume_action(
            growth.original.id,
            "resize",
            Some(serde_json::json!({"size": growth.requested_size})),
            cancel,
        )?;
        let current = self.observe_growth(growth, cancel)?;
        if current.size != growth.requested_size {
            return Err(CloudError::Invalid(
                "Storage growth is not confirmed; retry the same capacity",
            ));
        }
        growth.transition(Phase::Confirmed, &mut persist)?;
        Ok(current)
    }

    fn observe_growth(&self, growth: &Growth, cancel: &Cancellation) -> Result<Volume> {
        let current = self
            .inspect_volume(growth.original.id, cancel)?
            .ok_or(CloudError::WorkerLost)?;
        growth.verify_observed(&current)?;
        Ok(current)
    }
}
