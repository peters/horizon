//! Recoverable growth of one identity-verified network volume. No shrinking or allocation.
use super::{Result, Spec, State, Volume, validate_request_size};
use crate::{Cancellation, CloudError, runpod::RunPod};
use serde::{Deserialize, Serialize};

/// Persist this intent before driving it, and retain it until the caller's storage
/// and profile records have committed the returned capacity. Growth never grants
/// a fresh-volume creation receipt or permission to initialize existing files.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Growth {
    spec: Spec,
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
    /// A larger capacity for an existing bound volume whose tier is recorded.
    /// # Errors
    /// Rejects unknown ownership, a pending deletion, missing tier, or a shrink.
    pub fn new(spec: &Spec, state: &State, requested_size: u32) -> Result<Self> {
        state.verify(spec)?;
        let State::Bound { volume, .. } = state else {
            return Err(CloudError::Invalid("Only bound workspace storage can grow"));
        };
        let growth = Self {
            spec: spec.clone(),
            original: volume.clone(),
            requested_size,
            phase: Phase::Prepared,
        };
        growth.validate()?;
        Ok(growth)
    }

    /// The requested capacity in GB.
    #[must_use]
    pub const fn requested_size(&self) -> u32 {
        self.requested_size
    }

    /// Whether the provider reported the requested capacity and it was persisted.
    #[must_use]
    pub fn confirmed(&self) -> bool {
        self.phase == Phase::Confirmed
    }

    /// # Errors
    /// Checks the original durable binding without changing a pending phase.
    pub fn verify_origin(&self, spec: &Spec, state: &State) -> Result<()> {
        let expected = Self::new(spec, state, self.requested_size)?;
        if self.spec != expected.spec || self.original != expected.original {
            return Err(CloudError::IdentityMismatch);
        }
        self.validate()
    }

    fn validate(&self) -> Result<()> {
        self.original.verify(&self.spec)?;
        validate_request_size(self.requested_size)?;
        if self.original.tier != Some(self.spec.tier) || self.requested_size <= self.original.size {
            return Err(CloudError::Invalid(
                "Storage growth requires a recorded tier and a larger capacity",
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

impl RunPod {
    /// Grows only the recorded volume and confirms its identity and new capacity.
    /// Each mutation is fenced by `persist`; failures retain the same target.
    /// A retry reads first and never patches an already-grown volume. An uncertain
    /// request may resend the same absolute size, which is idempotent and cannot
    /// allocate another volume. The caller holds its cloud ownership lock throughout.
    /// # Errors
    /// Rejects missing or changed storage, shrinking, and persistence failures.
    /// An unconfirmed provider update retains its journal for an explicit retry.
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
        self.request(
            "PATCH",
            &format!("/network-volumes/{}", growth.original.id),
            Some(serde_json::json!({"size": growth.requested_size})),
            cancel,
        )?;
        let current = self.observe_growth(growth, cancel)?;
        if current.size != growth.requested_size {
            return Err(CloudError::Invalid(
                "Storage growth is not confirmed; retry to reconcile the same capacity",
            ));
        }
        growth.transition(Phase::Confirmed, &mut persist)?;
        Ok(current)
    }

    fn observe_growth(&self, growth: &Growth, cancel: &Cancellation) -> Result<Volume> {
        let current = self
            .inspect_volume(&growth.original.id, cancel)?
            .ok_or(CloudError::Invalid(
                "Workspace volume is missing; growth cannot recreate it",
            ))?;
        growth.verify_observed(&current)?;
        Ok(current)
    }
}
