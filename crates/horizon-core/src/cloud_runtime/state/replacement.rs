//! Journaled switch of a dedicated cloud's bound worker to a rebuilt image.
use super::{Deployment, Error, OperationId, Result, Stage, UncommittedImage};
use crate::cloud_runtime::repository;
use horizon_cloud::{CreateState, Profile, WorkerSpec};
use serde::{Deserialize, Serialize};

/// Refusal for actions that assume the worker runs its recorded image.
pub(in crate::cloud_runtime) const REPLACEMENT_PENDING: &str = "Image replacement pending; continue or cancel it";
const REPLACEMENT_MISMATCH: &str = "Image replacement journal does not match this cloud's worker";
const REPLACEMENT_UNBOUND: &str = "Only a bound worker's image can be replaced";
const TAG_TOO_LONG: &str = "This cloud's identity is too long for an image tag";
const PROFILE_MISMATCH: &str = "Image replacement journal changes more of this cloud's profile than a rebuild can";

/// A journaled switch of a dedicated cloud's bound worker to a rebuilt image. It is
/// saved before each provider mutation and kept until the replacement commits or is
/// cancelled. Only the image digest, the registry credential that pulls it and the
/// profile's rebuild settings (`Profile::with_rebuild_settings`) change; the worker ID,
/// workspace volume, protocol, sharing mode and SSH trust are kept.
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
    /// For a cloud with same-worker siblings, the commit of each sibling, in layering
    /// order, whose recipe the replacement image layers. Omitted without siblings, so
    /// single-repository journals keep their encoding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_revisions: Vec<String>,
    /// The replacement image's unique registry tag, `horizon-<cloud>-<operation>`.
    pub tag: String,
    pub phase: ReplacementPhase,
    /// The cloud's profile once the replacement commits, when the rebuild changes its
    /// settings. Omitted otherwise, so journals of an unchanged profile keep their
    /// encoding; an earlier Horizon version refuses a journal that carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Box<Profile>>,
    /// The hash of the uncommitted files a person confirmed for the image's build context
    /// (`repository::uncommitted`), when the build takes any. Omitted otherwise, so other
    /// journals keep their encoding; an earlier Horizon version refuses a journal that
    /// carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted: Option<String>,
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

/// The registry tag of the replacement journaled as `operation`: unique per replacement
/// and within Docker's tag grammar of at most 128 word characters, dots and dashes.
fn replacement_tag(cloud_id: &str, operation: OperationId) -> Option<String> {
    let tag = format!("horizon-{cloud_id}-{}", uuid::Uuid::from(operation).simple());
    (tag.len() <= 128 && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))).then_some(tag)
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
    /// only the image digest, its registry credential and the journaled profile differ
    /// from `spec`.
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
            || replacement.sibling_revisions.len() != self.siblings.as_ref().map_or(0, |set| set.members.len())
            || !replacement
                .sibling_revisions
                .iter()
                .all(|revision| repository::is_commit_id(revision))
            || replacement_tag(&self.cloud_id, replacement.operation).as_ref() != Some(&replacement.tag)
            || replacement
                .uncommitted
                .as_ref()
                .is_some_and(|hash| !repository::uncommitted::valid_hash(hash))
        {
            return Err(Error::Invalid(REPLACEMENT_MISMATCH));
        }
        // A changed profile must be one that a rebuild may bind: valid, with a recipe, and
        // different from the bound one only in its rebuild settings.
        if let Some(profile) = &replacement.profile
            && (**profile == spec.profile
                || spec.profile.with_rebuild_settings(profile) != **profile
                || profile.build.is_none()
                || profile.validate(false).is_err())
        {
            return Err(Error::Invalid(PROFILE_MISMATCH));
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
        if let Some(profile) = &replacement.profile {
            next.profile.clone_from(profile);
        }
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

    /// Journals a replacement of a ready, bound worker under its unique image tag.
    /// Nothing is built or sent yet.
    /// # Errors
    /// Refuses unless the cloud is ready on its bound worker with nothing pending, and
    /// a cloud identity too long for an image tag.
    pub fn begin_replacement(&mut self, operation: OperationId, recipe_revision: String) -> Result<()> {
        self.begin_layered_replacement(operation, recipe_revision, Vec::new())
    }

    /// As [`Self::begin_replacement`] for a cloud with same-worker siblings, whose image
    /// also layers each sibling's recipe at `sibling_revisions`, in layering order.
    /// # Errors
    /// As [`Self::begin_replacement`], and revisions that do not match the siblings.
    pub fn begin_layered_replacement(
        &mut self,
        operation: OperationId,
        recipe_revision: String,
        sibling_revisions: Vec<String>,
    ) -> Result<()> {
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
        let tag = replacement_tag(&self.cloud_id, operation).ok_or(Error::Invalid(TAG_TOO_LONG))?;
        let replacement = ImageReplacement {
            version: ImageReplacement::VERSION,
            operation,
            worker_id: worker_id.clone(),
            previous_digest: spec.image_digest.clone(),
            previous_registry_auth_id: spec.registry_auth_id.clone(),
            previous_registry_generation: self.registry_generation.clone(),
            recipe_revision,
            sibling_revisions,
            tag,
            phase: ReplacementPhase::Prepared {},
            profile: None,
            uncommitted: None,
        };
        self.replace_journal(|state| {
            state.image_replacement = Some(replacement);
            Ok(())
        })
    }

    /// Records `profile` as the cloud's profile once the prepared replacement commits.
    /// # Errors
    /// Refuses unless the replacement is prepared, and a profile that differs from the
    /// bound one in more than its rebuild settings.
    pub fn replacement_changes_profile(&mut self, profile: Profile) -> Result<()> {
        self.replace_journal(|state| {
            let changed = state.spec.as_ref().is_some_and(|spec| spec.profile != profile);
            let replacement = state.pending_replacement()?;
            if !matches!(replacement.phase, ReplacementPhase::Prepared {}) {
                return Err(Error::Invalid("Image replacement is already built"));
            }
            replacement.profile = changed.then(|| Box::new(profile));
            Ok(())
        })
    }

    /// Records `hash`, the hash of the confirmed uncommitted files that the prepared
    /// replacement builds from, or `None` when it builds from the commit alone.
    /// # Errors
    /// Refuses unless the replacement is prepared, and a hash of another form.
    pub fn replacement_takes_uncommitted(&mut self, hash: Option<String>) -> Result<()> {
        self.replace_journal(|state| {
            let replacement = state.pending_replacement()?;
            if !matches!(replacement.phase, ReplacementPhase::Prepared {}) {
                return Err(Error::Invalid("Image replacement is already built"));
            }
            replacement.uncommitted = hash;
            Ok(())
        })
    }

    /// The profile the pending replacement builds for and binds once it commits; the
    /// bound profile without one.
    #[must_use]
    pub fn replacement_profile(&self) -> &Profile {
        self.image_replacement
            .as_ref()
            .and_then(|replacement| replacement.profile.as_deref())
            .unwrap_or(&self.profile)
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
    /// specification, registry binding and the cloud's profile switch to it, the
    /// journal is cleared and the sessions must relaunch before the cloud is ready
    /// again. Persist it only through `deployment::commit_replacement`, which rebinds
    /// the storage journal first.
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
        // The image now layers these recipes; the checkouts on the worker stay as imported.
        if let Some(set) = &mut self.siblings {
            for (sibling, revision) in set.members.iter_mut().zip(&replacement.sibling_revisions) {
                sibling.image_revision = (*revision != sibling.revision).then(|| revision.clone());
            }
        }
        self.registry_generation.clone_from(&image.registry_generation);
        self.uncommitted_image = replacement.uncommitted.as_ref().map(|hash| UncommittedImage {
            revision: replacement.recipe_revision.clone(),
            hash: hash.clone(),
        });
        self.profile.clone_from(&next.profile);
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
