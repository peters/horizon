//! Hetzner's lifecycle: `deployment::hetzner::lifecycle` over the Hetzner client,
//! with the cloud's records beside the deployment.
use super::{
    super::{Error, deployment::hetzner as deployment},
    Cancellation, CreateState, Deployment, Event, Lifecycle, Result, Settings, Store,
};
use horizon_cloud::runpod::recovery::Reconciliation;

/// The machine's Hetzner settings, which every Hetzner deployment needs.
fn settings(settings: &Settings) -> Result<&super::super::settings::Hetzner> {
    settings.hetzner.as_ref().ok_or(Error::Invalid(
        "Add a hetzner section to the cloud settings before deploying a Hetzner cloud",
    ))
}

/// The server types Horizon tries, in order; the spec keeps them as its CPU flavors.
pub(super) fn cpu_flavors(machine: &Settings) -> Result<Vec<String>> {
    Ok(settings(machine)?.server_types.clone())
}

/// The locations this cloud may run in, narrowed by its placement.
pub(super) fn data_centers(machine: &Settings) -> Result<Vec<String>> {
    settings(machine)?.locations_for(machine.placement.as_ref())
}

/// The workspace volume, or the SSH key, which is recorded before it is registered.
pub(super) fn retained(store: &Store) -> Result<bool> {
    deployment::retained(store.root())
}

pub(super) struct Hetzner<'a> {
    pub(super) settings: &'a Settings,
}

impl Lifecycle for Hetzner<'_> {
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        _worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)> {
        let report = deployment::lifecycle::reconcile(store, state, self.settings, cancel)?;
        Ok((report, state.operation.clone()))
    }

    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
        deployment::lifecycle::stop(store, state, self.settings, cancel)
    }

    fn resume(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
        // No provider call follows here, so a cancelled Resume must stop before the
        // fence is cleared: the reconnect it leads to creates a billed server.
        cancel.check()?;
        deployment::lifecycle::resume(store, state)
    }

    /// Hetzner profiles cannot request hosted devices, so a Hetzner cloud has none to
    /// release; nothing here may reach another provider for it.
    fn release_devices(&self, _store: &Store, _state: &mut Deployment, _cancel: &Cancellation) -> Result<()> {
        Err(Error::Invalid("Hetzner clouds hold no hosted devices to release"))
    }

    fn delete(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<()> {
        super::super::deployment::deletion::delete_hetzner(store, state, self.settings, cancel, emit)
    }
}
