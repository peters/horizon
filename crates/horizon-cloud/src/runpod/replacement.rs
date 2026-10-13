//! In-place image replacement of an existing worker through the provider's pod update.
use super::{REQUEST_TIMEOUT, RunPod, json, volumes::Volume};
use crate::{Cancellation, CloudError, ImageSide, Worker, WorkerSpec};
use serde_json::Value;
use std::time::{Duration, Instant};

/// The image the provider records for a worker whose image is being replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    Previous,
    Next,
    /// Two consecutive provider observations report different images; observe again.
    Unsettled,
}
impl From<ImageSide> for Observed {
    fn from(side: ImageSide) -> Self {
        match side {
            ImageSide::Previous => Self::Previous,
            ImageSide::Next => Self::Next,
        }
    }
}

/// Whether a failed `RunPod::replace_image` may still have applied the update: a
/// lost response or a server failure. A client error (4xx) is the provider's refusal,
/// and every other error is raised before the update is sent.
#[must_use]
pub const fn may_have_applied(error: &CloudError) -> bool {
    match error {
        CloudError::Transport => true,
        CloudError::Http(status, _) => *status < 400 || *status >= 500,
        _ => false,
    }
}

impl RunPod {
    /// Switches a running worker from `current`'s image to `next`'s through the
    /// pod update. This resets the container: the pod ID and its `/workspace`
    /// volume persist, but the container disk is wiped and every process restarts.
    /// Callers must durably journal the intent before calling, because the update
    /// may apply even when this returns an error. A worker that already reports
    /// `next` is updated, and therefore reset, again.
    /// # Errors
    /// Refuses a worker that is not running or not verifiably this operation's,
    /// and any change `WorkerSpec::verify_replacement` refuses. Only errors
    /// for which `may_have_applied` holds leave the outcome uncertain; observe the
    /// worker with `observe_image` before retrying or reverting.
    pub fn replace_image(
        &self,
        current: &WorkerSpec,
        next: &WorkerSpec,
        worker_id: &str,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        let body = update_body(current, next)?;
        current.validate()?;
        next.validate()?;
        let worker = self
            .inspect(worker_id, cancel)
            .map_err(before_update)?
            .ok_or(CloudError::WorkerLost)?;
        identify(&worker, current, next)?;
        if worker.desired_status != "RUNNING" {
            return Err(CloudError::Invalid("Worker must be running to replace its image"));
        }
        self.request("PATCH", &format!("/pods/{worker_id}"), Some(body), cancel)?;
        Ok(())
    }

    /// Reports which image of the replacement the provider records for the worker.
    /// A worker with a CPU workspace `volume` is also read through the current API,
    /// and the replacement settles only when both observations report the same image.
    /// A worker whose environment still selects the other side's capabilities is
    /// unsettled too. `timeout` bounds the whole observation.
    /// # Errors
    /// `WorkerLost` when the provider no longer returns the worker; `IdentityMismatch`
    /// for a third image, a missing or different operation marker, or another mount.
    pub fn observe_image(
        &self,
        worker_id: &str,
        current: &WorkerSpec,
        next: &WorkerSpec,
        volume: Option<&Volume>,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<Observed, CloudError> {
        current.verify_replacement(next)?;
        if let Some(volume) = volume {
            volume.verify_worker_spec(current)?;
        }
        let started = Instant::now();
        let worker = self
            .inspect_with_timeout(worker_id, cancel, timeout)?
            .ok_or(CloudError::WorkerLost)?;
        let listed = identify(&worker, current, next)?;
        // The image and the environment may not change at once; a worker that still
        // carries the other side's capabilities is not settled on this one.
        let spec = if listed == ImageSide::Next { next } else { current };
        if !carries_capabilities(&worker, spec) {
            return Ok(Observed::Unsettled);
        }
        let Some(volume) = volume else {
            return Ok(listed.into());
        };
        let remaining = timeout.saturating_sub(started.elapsed()).min(REQUEST_TIMEOUT);
        if remaining.is_zero() {
            return Err(CloudError::Transport);
        }
        let mounted = self.mounted_image(&worker, volume, cancel, remaining)?;
        let mounted = ImageSide::of(&mounted, current, next).ok_or(CloudError::IdentityMismatch)?;
        Ok(if mounted == listed {
            listed.into()
        } else {
            Observed::Unsettled
        })
    }
}

/// A read that fails before the update was sent leaves nothing uncertain, even when
/// the read itself failed in transit or on the server.
fn before_update(error: CloudError) -> CloudError {
    if may_have_applied(&error) {
        CloudError::Invalid("Could not confirm the worker before switching its image; no update was sent")
    } else {
        error
    }
}

/// Stricter than `Worker::verify_either`: an update that dropped the environment
/// would also drop the worker's key and capabilities, so the operation marker
/// must be present rather than merely consistent.
fn identify(worker: &Worker, current: &WorkerSpec, next: &WorkerSpec) -> Result<ImageSide, CloudError> {
    let side = worker.verify_either(current, next)?;
    if worker.env.get("HORIZON_CLOUD_OPERATION") != Some(&current.operation_id) {
        return Err(CloudError::IdentityMismatch);
    }
    Ok(side)
}

/// Whether the worker's environment selects `spec`'s capabilities. A worker created
/// before Horizon recorded them in the environment carries none, which any spec accepts.
fn carries_capabilities(worker: &Worker, spec: &WorkerSpec) -> bool {
    worker.env.get("HORIZON_WORKER_CAPABILITIES").is_none_or(|selected| {
        serde_json::from_str::<crate::Capabilities>(selected)
            .is_ok_and(|selected| selected == spec.profile.capabilities)
    })
}

/// Sends only the fields that change, relying on the update to keep omitted ones. A
/// rebuild that changes the worker's capabilities or idle stop sends the whole new
/// environment, which the update replaces as a whole.
fn update_body(current: &WorkerSpec, next: &WorkerSpec) -> Result<Value, CloudError> {
    current.verify_replacement(next)?;
    let mut body = json!({"image": next.image_digest});
    let environment = super::create::environment(next);
    if environment != super::create::environment(current) {
        body["env"] = environment;
    }
    match (&current.registry_auth_id, &next.registry_auth_id) {
        // How the update clears a credential is unverified; refuse rather than guess.
        (Some(_), None) => {
            return Err(CloudError::Invalid(
                "An image replacement cannot remove the registry credential",
            ));
        }
        (previous, Some(id)) if previous.as_ref() != Some(id) => body["registry"] = json!(id),
        _ => {}
    }
    Ok(body)
}
