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
use horizon_cloud::runpod::recovery::Reconciliation;
use horizon_cloud::{Profile, provider::Kind};

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

/// Which provider `profile` names; a match on it must handle every provider.
fn kind(profile: &Profile) -> Kind {
    horizon_cloud::provider::Description::of(profile).kind
}

/// Checks what the provider needs before any record, build or request, so an
/// unsupported request fails at once and leaves nothing behind.
pub(super) fn preflight(cloud_id: &str, profile: &Profile, settings: &Settings) -> Result<()> {
    match kind(profile) {
        Kind::Hetzner => super::deployment::hetzner::preflight(cloud_id, profile, settings),
        Kind::RunPod => Ok(()),
    }
}

/// Checks what the provider needs before the deployment is recorded for `image`,
/// unless its worker is already requested or bound and so only reconciled.
pub(super) fn admit(settings: &Settings, profile: &Profile, image: &str, operation: &CreateState) -> Result<()> {
    match kind(profile) {
        Kind::Hetzner => super::deployment::hetzner::admit(settings, image, operation),
        Kind::RunPod => Ok(()),
    }
}

/// The machine types a worker may run on, in the order they are tried: `RunPod` CPU
/// flavors, or a Hetzner cloud's server types.
pub(super) fn cpu_flavors(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    match kind(profile) {
        Kind::Hetzner => hetzner::cpu_flavors(settings),
        Kind::RunPod => runpod::cpu_flavors(profile, settings),
    }
}

/// Where a worker may run: `RunPod` data centers, or a Hetzner cloud's locations.
pub(super) fn data_centers(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    match kind(profile) {
        Kind::Hetzner => hetzner::data_centers(settings),
        Kind::RunPod => Ok(settings.data_centers.clone()),
    }
}

/// The provider a cloud of `request` deploys on, with its client.
/// # Errors
/// Refuses missing credentials and settings the provider needs.
pub(super) fn compute(request: &super::deployment::Request) -> Result<Box<dyn super::deployment::Compute>> {
    Ok(match kind(&request.profile) {
        Kind::Hetzner => Box::new(super::deployment::hetzner::Compute::new(&request.settings)?),
        Kind::RunPod => Box::new(horizon_cloud::runpod::RunPod::new(request.settings.credential()?)),
    })
}

/// Whether anything the cloud created on its provider may still exist, read from
/// local records only, so it needs no provider credential.
pub(super) fn retained(store: &Store, state: &Deployment) -> Result<bool> {
    match kind(&state.profile) {
        Kind::Hetzner => hetzner::retained(store),
        Kind::RunPod => runpod::retained(store, state),
    }
}

/// The lifecycle of the provider `state`'s profile names.
pub(super) fn lifecycle<'a>(state: &Deployment, settings: &'a Settings) -> Box<dyn Lifecycle + 'a> {
    match kind(&state.profile) {
        Kind::Hetzner => Box::new(hetzner::Hetzner { settings }),
        Kind::RunPod => Box::new(runpod::RunPod { settings }),
    }
}
