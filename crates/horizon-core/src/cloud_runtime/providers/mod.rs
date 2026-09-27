//! Each provider's lifecycle behind one interface. `lifecycle` asks the cloud's
//! provider for what it records and does, and keeps what every provider shares: the
//! deployment record, its lock and what a found or terminated worker means for it.
//! Adding a provider means one implementation here and its description in
//! `horizon_cloud::provider`.
mod hetzner;
mod runpod;

use super::{
    Cancellation, CreateState, Event, Result,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::Profile;
use horizon_cloud::runpod::recovery::Reconciliation;

/// What a provider does for a cloud's lifecycle.
pub(super) trait Lifecycle {
    /// Checks the recorded worker without creating, starting or deleting anything,
    /// recording what it finds; returns the report and the server fence after it.
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)>;
    /// Stops the cloud's worker as the provider stops it (`provider::StoppedCost`).
    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()>;
    /// Resumes a stopped cloud; the reconnect that follows finishes it.
    fn resume(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()>;
    /// Releases the worker's hosted devices and removes their credentials from it.
    fn release_devices(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()>;
    /// Deletes everything the cloud created on the provider, each proven absent, and
    /// emits each step; recording the cloud as deleted is left to the caller.
    fn delete(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<()>;
}

/// Whether `profile` is a Hetzner one; every other profile is a `RunPod` one.
fn hetzner(profile: &Profile) -> bool {
    horizon_cloud::provider::Description::of(profile).id == horizon_cloud::hetzner::PROVIDER
}

/// Checks what the provider needs before any record, build or request, so an
/// unsupported request fails at once and leaves nothing behind.
pub(super) fn preflight(cloud_id: &str, profile: &Profile, settings: &Settings) -> Result<()> {
    if hetzner(profile) {
        return super::deployment::hetzner::preflight(cloud_id, profile, settings);
    }
    Ok(())
}

/// Checks what the provider needs before the deployment is recorded for `image`,
/// unless its worker is already requested or bound and so only reconciled.
pub(super) fn admit(settings: &Settings, profile: &Profile, image: &str, operation: &CreateState) -> Result<()> {
    if hetzner(profile) {
        return super::deployment::hetzner::admit(settings, image, operation);
    }
    Ok(())
}

/// The machine types a worker may run on, in the order they are tried: `RunPod` CPU
/// flavors, or a Hetzner cloud's server types.
pub(super) fn cpu_flavors(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    if hetzner(profile) {
        return hetzner::cpu_flavors(settings);
    }
    runpod::cpu_flavors(profile, settings)
}

/// Where a worker may run: `RunPod` data centers, or a Hetzner cloud's locations.
pub(super) fn data_centers(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    if hetzner(profile) {
        return hetzner::data_centers(settings);
    }
    Ok(settings.data_centers.clone())
}

/// Whether anything the cloud created on its provider may still exist, read from
/// local records only, so it needs no provider credential.
pub(super) fn retained(store: &Store, state: &Deployment) -> Result<bool> {
    if hetzner(&state.profile) {
        return hetzner::retained(store);
    }
    runpod::retained(store, state)
}

/// The lifecycle of the provider `state`'s profile names.
pub(super) fn lifecycle<'a>(state: &Deployment, settings: &'a Settings) -> Box<dyn Lifecycle + 'a> {
    if hetzner(&state.profile) {
        return Box::new(hetzner::Hetzner { settings });
    }
    Box::new(runpod::RunPod { settings })
}
