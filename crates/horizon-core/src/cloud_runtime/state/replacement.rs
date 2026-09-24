//! Journaled switch of a dedicated cloud's bound worker to a rebuilt image.
use super::{Deployment, Error, OperationId, Result, Stage};
use crate::cloud_runtime::repository;
use horizon_cloud::{CreateState, WorkerSpec};
use serde::{Deserialize, Serialize};

/// Refusal for actions that assume the worker runs its recorded image.
pub(in crate::cloud_runtime) const REPLACEMENT_PENDING: &str = "Image replacement pending; continue or cancel it";
const REPLACEMENT_MISMATCH: &str = "Image replacement journal does not match this cloud's worker";
const REPLACEMENT_UNBOUND: &str = "Only a bound worker's image can be replaced";

/// A journaled switch of a dedicated cloud's bound worker to a rebuilt image. It is
/// saved before each provider mutation and kept until the replacement commits or is
/// cancelled. Only the image digest and the registry credential that pulls it change;
/// the worker ID, workspace volume, protocol, sharing mode and SSH trust are kept.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageReplacement {
    pub version: u32,
    pub operation: OperationId,
    pub worker_id: String,
    pub previous_digest: String,
    pub previous_registry_auth_id: Option<String>,
    pub previous_registry_generation: Option<String>,
    /// Commit whose `.horizon` recipe builds the replacement image.
    pub recipe_revision: String,
    /// The replacement image's unique registry tag.
    pub tag: String,
    pub phase: ReplacementPhase,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplacementPhase {
    /// Nothing is built or sent; the worker runs the previous image. A struct variant,
    /// because an internally tagged unit variant would ignore unknown fields.
    Prepared {},
    /// The image is pushed and verified; the provider update is unsent.
    Built(ReplacementImage),
    /// The provider update may have been sent, so the worker may run either image.
    /// The deployment's stage is `Stage::Replace` exactly while this is recorded.
    Requested(ReplacementImage),
}

/// A pushed replacement image and the registry binding that pulls it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplacementImage {
    pub digest: String,
    pub registry_auth_id: Option<String>,
    pub registry_generation: Option<String>,
}

impl ImageReplacement {
    pub const VERSION: u32 = 1;

    /// The replacement image once it is built.
    #[must_use]
    pub const fn image(&self) -> Option<&ReplacementImage> {
        match &self.phase {
            ReplacementPhase::Prepared {} => None,
            ReplacementPhase::Built(image) | ReplacementPhase::Requested(image) => Some(image),
        }
    }

    /// Whether the provider update may have been sent.
    #[must_use]
    pub const fn requested(&self) -> bool {
        matches!(self.phase, ReplacementPhase::Requested(_))
    }
}

impl Deployment {
    /// The bound worker's specification on the replacement image once it is built:
    /// only the image digest and its registry credential differ from `spec`.
    /// # Errors
    /// Rejects a journal that does not match this deployment's worker, image and
    /// stage, and a replacement image that is not a different immutable digest.
    pub fn replacement_worker(&self) -> Result<Option<WorkerSpec>> {
        let Some(replacement) = &self.image_replacement else {
            if self.stage == Stage::Replace {
                return Err(Error::Invalid(REPLACEMENT_MISMATCH));
            }
            return Ok(None);
        };
        let spec = self.spec.as_ref().ok_or(Error::Invalid(REPLACEMENT_MISMATCH))?;
        let (CreateState::Bound { worker_id } | CreateState::Terminated { worker_id }) = &self.operation else {
            return Err(Error::Invalid(REPLACEMENT_MISMATCH));
        };
        if replacement.version != ImageReplacement::VERSION
            || replacement.worker_id != *worker_id
            || replacement.previous_digest != spec.image_digest
            || replacement.previous_registry_auth_id != spec.registry_auth_id
            || replacement.previous_registry_generation != self.registry_generation
            || replacement.requested() != (self.stage == Stage::Replace)
            || !repository::is_commit_id(&replacement.recipe_revision)
        {
            return Err(Error::Invalid(REPLACEMENT_MISMATCH));
        }
        let Some(image) = replacement.image() else {
            return Ok(None);
        };
        if !horizon_cloud::valid_image(&image.digest) || !image.digest.contains("@sha256:") {
            return Err(Error::Invalid("Replacement image must be an immutable digest"));
        }
        let mut next = spec.clone();
        next.image_digest.clone_from(&image.digest);
        next.registry_auth_id.clone_from(&image.registry_auth_id);
        spec.verify_replacement(&next)?;
        Ok(Some(next))
    }

    /// Stop, resume and device release act on the worker as recorded, so a pending
    /// replacement must be continued or cancelled first. Deletion stays available.
    pub(in crate::cloud_runtime) fn refuse_pending_replacement(&self) -> Result<()> {
        if self.image_replacement.is_some() || self.stage == Stage::Replace {
            return Err(Error::Invalid(REPLACEMENT_PENDING));
        }
        Ok(())
    }

    /// Reconnecting assumes the recorded image, which holds until the provider
    /// update may have been sent; a prepared or built replacement is kept.
    pub(in crate::cloud_runtime) fn refuse_unsettled_replacement(&self) -> Result<()> {
        self.replacement_worker()?;
        if self.image_replacement.as_ref().is_some_and(ImageReplacement::requested) {
            return Err(Error::Invalid(REPLACEMENT_PENDING));
        }
        Ok(())
    }

    /// Journals a replacement of a ready, bound worker. Nothing is built or sent yet.
    /// # Errors
    /// Refuses unless the cloud is ready on its bound worker with nothing pending.
    pub fn begin_replacement(&mut self, operation: OperationId, recipe_revision: String, tag: String) -> Result<()> {
        let CreateState::Bound { worker_id } = &self.operation else {
            return Err(Error::Invalid(REPLACEMENT_UNBOUND));
        };
        let spec = self
            .spec
            .as_ref()
            .ok_or(Error::Invalid("Missing worker specification"))?;
        if self.stage != Stage::Ready
            || self.stop_requested
            || self.image_replacement.is_some()
            || self.session_restart.is_some()
        {
            return Err(Error::Invalid(
                "Only a ready cloud without a pending replacement can replace its image",
            ));
        }
        let replacement = ImageReplacement {
            version: ImageReplacement::VERSION,
            operation,
            worker_id: worker_id.clone(),
            previous_digest: spec.image_digest.clone(),
            previous_registry_auth_id: spec.registry_auth_id.clone(),
            previous_registry_generation: self.registry_generation.clone(),
            recipe_revision,
            tag,
            phase: ReplacementPhase::Prepared {},
        };
        self.replace_journal(|state| {
            state.image_replacement = Some(replacement);
            Ok(())
        })
    }

    /// Records the pushed and verified image; the provider update is still unsent.
    /// # Errors
    /// Refuses unless the replacement is prepared, and an image that is not a
    /// different immutable digest.
    pub fn replacement_built(&mut self, image: ReplacementImage) -> Result<()> {
        self.replace_journal(|state| {
            let replacement = state.pending_replacement()?;
            if !matches!(replacement.phase, ReplacementPhase::Prepared {}) {
                return Err(Error::Invalid("Image replacement is already built"));
            }
            replacement.phase = ReplacementPhase::Built(image);
            Ok(())
        })
    }

    /// Marks the provider update as possibly sent and enters `Stage::Replace`, which
    /// older Horizon versions cannot parse. Save this before sending the update.
    /// Returns the stage to restore if the provider definitely refuses it.
    /// # Errors
    /// Refuses unless the replacement image is built and the update is unsent.
    pub fn request_replacement(&mut self) -> Result<Stage> {
        let previous = self.stage;
        self.replace_journal(|state| {
            let replacement = state.pending_replacement()?;
            let ReplacementPhase::Built(image) = &replacement.phase else {
                return Err(Error::Invalid("Build the replacement image before requesting it"));
            };
            replacement.phase = ReplacementPhase::Requested(image.clone());
            state.stage = Stage::Replace;
            Ok(())
        })?;
        Ok(previous)
    }

    /// Returns a provider update that was definitely not applied to `Built`, restoring
    /// the stage `request_replacement` returned. Only valid for the first update sent:
    /// once an update may have applied, a later refusal does not undo it.
    /// # Errors
    /// Refuses unless the update was requested.
    pub fn refuse_replacement(&mut self, stage: Stage) -> Result<()> {
        self.replace_journal(|state| {
            let replacement = state.pending_replacement()?;
            let ReplacementPhase::Requested(image) = &replacement.phase else {
                return Err(Error::Invalid("No image replacement was requested"));
            };
            replacement.phase = ReplacementPhase::Built(image.clone());
            state.stage = stage;
            Ok(())
        })
    }

    /// Commits a replacement that the provider reports on the new image: the worker's
    /// specification and registry binding switch to it, the journal is cleared and the
    /// sessions must relaunch before the cloud is ready again. Persist it only through
    /// `deployment::commit_replacement`, which rebinds the storage journal first.
    pub(in crate::cloud_runtime) fn commit_replacement(&mut self) -> Result<OperationId> {
        self.require_bound_worker()?;
        let next = self.replacement_worker()?;
        let (Some(next), Some(replacement)) = (next, self.image_replacement.as_ref()) else {
            return Err(Error::Invalid("No image replacement was requested"));
        };
        let ReplacementPhase::Requested(image) = &replacement.phase else {
            return Err(Error::Invalid("No image replacement was requested"));
        };
        let operation = replacement.operation;
        self.registry_generation.clone_from(&image.registry_generation);
        self.spec = Some(next);
        self.image_replacement = None;
        self.session_restart = Some(operation);
        self.stage = Stage::Readiness;
        Ok(operation)
    }

    /// Drops a replacement whose provider update was never sent.
    /// # Errors
    /// Refuses once the update may have been sent: that needs the provider.
    pub fn discard_replacement(&mut self) -> Result<()> {
        self.replacement_worker()?;
        if self.image_replacement.as_ref().is_some_and(ImageReplacement::requested) {
            return Err(Error::Invalid(
                "The image update may have been sent; cancel it through the provider",
            ));
        }
        self.image_replacement = None;
        Ok(())
    }

    fn pending_replacement(&mut self) -> Result<&mut ImageReplacement> {
        self.image_replacement
            .as_mut()
            .ok_or(Error::Invalid("No image replacement is pending"))
    }

    /// A terminated worker keeps its journal only so deletion can identify it on
    /// either image; a replacement moves forward only on the bound worker.
    fn require_bound_worker(&self) -> Result<()> {
        if matches!(self.operation, CreateState::Bound { .. }) {
            Ok(())
        } else {
            Err(Error::Invalid(REPLACEMENT_UNBOUND))
        }
    }

    /// Applies a journal transition only when the result is consistent.
    fn replace_journal(&mut self, change: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        self.require_bound_worker()?;
        self.replacement_worker()?;
        let mut next = self.clone();
        change(&mut next)?;
        next.replacement_worker()?;
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
