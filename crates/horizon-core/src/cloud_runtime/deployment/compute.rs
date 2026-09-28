//! Each provider's part in deploying a worker, behind the one interface
//! `deploy_locked` drives.
use super::{
    Cancellation, Connection, Deployment, Error, Mutation, Request, Result, RunPod, Runner, Store, WorkerContract,
    WorkerSpec, bind_registry, hetzner, mutation, provision, readiness, replacement,
};
use crate::cloud_runtime::state::ImageReplacement;

/// What a provider does to deploy a cloud's worker; `providers::compute` picks it
/// from the cloud's provider description. `observe` is told before each provider
/// mutation and once the provider's work is settled.
pub(in crate::cloud_runtime) trait Compute {
    /// Settles what an earlier attempt left with the provider, before this one.
    /// # Errors
    /// Reports provider and persistence failures.
    fn settle(
        &self,
        _store: &Store,
        _state: &mut Deployment,
        _cancel: &Cancellation,
        _observe: mutation::Observer<'_>,
    ) -> Result<()> {
        Ok(())
    }
    /// Verifies the private image and, for a provider that stores the pull
    /// credential, binds it; a provider whose host logs in to the registry itself
    /// stores none.
    /// # Errors
    /// Reports registry, provider and persistence failures.
    fn verify_registry(
        &self,
        _store: &Store,
        state: &mut Deployment,
        registry: &mut crate::cloud_runtime::registry::Prepared,
        cancel: &Cancellation,
        _observe: mutation::Observer<'_>,
    ) -> Result<()> {
        verify_image(state, registry, cancel)
    }
    /// Provisions the worker, or reconnects to the one it has, and waits until it is ready.
    /// # Errors
    /// Reports provider, persistence and readiness failures.
    fn provision_ready(
        &self,
        request: &Request,
        store: &Store,
        runner: &Runner<'_>,
        state: &mut Deployment,
        spec: &WorkerSpec,
        observe: mutation::Observer<'_>,
    ) -> Result<(Connection, WorkerContract)>;
}

impl Compute for RunPod {
    fn settle(
        &self,
        store: &Store,
        state: &mut Deployment,
        cancel: &Cancellation,
        observe: mutation::Observer<'_>,
    ) -> Result<()> {
        let requested = state
            .image_replacement
            .as_ref()
            .is_some_and(ImageReplacement::requested);
        if requested {
            observe(Mutation::Pending)?;
        }
        replacement::settle(self, store, state, cancel)?;
        if requested {
            observe(Mutation::Settled)?;
        }
        Ok(())
    }

    fn verify_registry(
        &self,
        store: &Store,
        state: &mut Deployment,
        registry: &mut crate::cloud_runtime::registry::Prepared,
        cancel: &Cancellation,
        observe: mutation::Observer<'_>,
    ) -> Result<()> {
        // An earlier binding left uncertain stays so even when verification fails.
        registry.observe_pending(observe)?;
        verify_image(state, registry, cancel)?;
        bind_registry(store, state, || {
            registry.ensure_provider_observed(self, cancel, observe)
        })
    }

    fn provision_ready(
        &self,
        request: &Request,
        store: &Store,
        runner: &Runner<'_>,
        state: &mut Deployment,
        spec: &WorkerSpec,
        observe: mutation::Observer<'_>,
    ) -> Result<(Connection, WorkerContract)> {
        provision(self, store, state, spec, runner.cancel, runner.emit, observe)?;
        readiness::wait(request, self, store, runner, state, spec)
    }
}

impl Compute for hetzner::Compute {
    /// Hetzner cannot report a server's image, so a rebuild whose release may have
    /// begun is finished or cancelled only through the rebuild; a reconnect must not
    /// place a server around it.
    fn settle(
        &self,
        _store: &Store,
        state: &mut Deployment,
        _cancel: &Cancellation,
        _observe: mutation::Observer<'_>,
    ) -> Result<()> {
        state.refuse_unsettled_replacement()
    }

    fn provision_ready(
        &self,
        request: &Request,
        store: &Store,
        runner: &Runner<'_>,
        state: &mut Deployment,
        spec: &WorkerSpec,
        observe: mutation::Observer<'_>,
    ) -> Result<(Connection, WorkerContract)> {
        hetzner::provision(self, store, state, spec, runner.cancel, runner.emit, observe)?;
        hetzner::wait(request, self, store, runner, state, spec)
    }
}

/// Checks that `registry` still serves the worker's exact image digest.
fn verify_image(
    state: &Deployment,
    registry: &mut crate::cloud_runtime::registry::Prepared,
    cancel: &Cancellation,
) -> Result<()> {
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Deployment has no image to validate"))?;
    registry.verify_image(&spec.image_digest, cancel)
}
