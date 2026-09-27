//! Each provider's lifecycle behind one interface. `lifecycle` asks the cloud's
//! provider for what it records and does, and keeps what every provider shares: the
//! deployment record, its lock and what a found or terminated worker means for it.
//! Adding a provider means one implementation here and its description in
//! `horizon_cloud::provider`.
mod hetzner;
mod runpod;

use super::{
    Cancellation, CreateState, Result,
    settings::Settings,
    state::{Deployment, Store},
};
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
}

/// Whether anything the cloud created on its provider may still exist, read from
/// local records only, so it needs no provider credential.
pub(super) fn retained(store: &Store, state: &Deployment) -> Result<bool> {
    match horizon_cloud::provider::Description::of(&state.profile).id {
        horizon_cloud::hetzner::PROVIDER => hetzner::retained(store),
        _ => runpod::retained(store, state),
    }
}

/// The lifecycle of the provider `state`'s profile names.
pub(super) fn lifecycle<'a>(state: &Deployment, settings: &'a Settings) -> Box<dyn Lifecycle + 'a> {
    match horizon_cloud::provider::Description::of(&state.profile).id {
        horizon_cloud::hetzner::PROVIDER => Box::new(hetzner::Hetzner { settings }),
        _ => Box::new(runpod::RunPod { settings }),
    }
}
