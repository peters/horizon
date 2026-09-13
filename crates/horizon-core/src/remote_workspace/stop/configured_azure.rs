//! Configured Azure saved-Stop admission: the exact named profile, its immutable CPU
//! binding, the retained persistent worker with its complete Azure handle and public pin
//! are bound before the CLI credential or client exists. Observation only: no Stop,
//! Start, creation, deletion, private-key repair or host-key attestation happens here.

use super::{
    ConfiguredStopConfirmation, ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopConfirmation,
    RemoteWorkspaceStopError, current_millis,
};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, RemoteCpuProfileBinding, StoredRemoteAllocation, WorkerLifetime,
        azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile, AzureWorker, resource_group_name},
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerProvider,
            InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
        interactive_worker_stop::{
            InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation, InteractiveWorkerStopObserver,
        },
    },
    remote_workspace::RemoteEnvironmentSummary,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// One admitted Azure allocation: exact summary, valid public request, persistent
/// target within the named profile, immutable binding to that profile, no `RunPod`
/// storage expectation, no management intent, and a retained worker whose complete
/// Azure handle validates under the profile's subscription with a complete pin.
pub(super) struct RetainedAzure {
    pub(super) allocation: StoredRemoteAllocation,
    request: InteractiveWorkerRequest,
    binding: RemoteCpuProfileBinding,
}

impl RetainedAzure {
    pub(super) fn load(
        store: &CloudWorkflowStore,
        profile: &AzureProfile,
        expected: &RemoteEnvironmentSummary,
    ) -> Result<Self, BindingError> {
        let allocation = store
            .load_remote_allocation(&expected.owning_session_id, &expected.workspace_local_id)
            .map_err(RemoteWorkspaceStopError::from)?
            .ok_or(RemoteWorkspaceStopError::MissingAllocation)?;
        if allocation.workspace().environment_summary() != *expected {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        let request = allocation.worker_request().map_err(RemoteWorkspaceStopError::from)?;
        if !request.is_valid_for(CloudProvider::Azure) {
            return Err(BindingError::InvalidBinding);
        }
        if request.target.lifetime != WorkerLifetime::Persistent {
            return Err(RemoteWorkspaceStopError::UnsupportedLifetime.into());
        }
        AzureDeploymentPlan::validate_target(profile, &request.target).map_err(|_| BindingError::InvalidBinding)?;
        let runtime = allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(BindingError::InvalidBinding)?;
        if runtime.cleanup.is_some() {
            return Err(RemoteWorkspaceStopError::ManagementConflict.into());
        }
        // The worker was created under one immutable profile binding; the named profile
        // must still be that exact profile (subscription and every approved field).
        // Missing provenance is never backfilled from the current configuration.
        let binding = store
            .load_remote_cpu_profile_binding(&allocation)
            .map_err(RemoteWorkspaceStopError::from)?
            .ok_or(BindingError::InvalidBinding)?;
        if !binding
            .matches_profile(profile)
            .map_err(|_| BindingError::InvalidBinding)?
        {
            return Err(BindingError::InvalidBinding);
        }
        // An Azure worker carries no RunPod volume expectation; a saved selection is a
        // foreign binding and is refused rather than passed to the observer.
        if store
            .load_remote_network_volume_selection(&allocation)
            .map_err(RemoteWorkspaceStopError::from)?
            .is_some()
        {
            return Err(BindingError::InvalidBinding);
        }
        let admitted = Self {
            allocation,
            request,
            binding,
        };
        admitted.trust(profile)?;
        Ok(admitted)
    }

    /// The retained worker is the reserved request's worker, its complete Azure handle
    /// (subscription, identity-derived group, exact group ID, image, lifetime) validates
    /// under the named profile, and its saved pin is complete. The pin is saved and
    /// shape-checked, never attested here; the client sees this handle later and must
    /// not be the first to reject it.
    fn trust(&self, profile: &AzureProfile) -> Result<(), BindingError> {
        let runtime = self
            .allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(BindingError::InvalidBinding)?;
        let worker = runtime.worker.as_ref().ok_or(RemoteWorkspaceStopError::MissingWorker)?;
        let ssh = runtime.ssh.as_ref().ok_or(RemoteWorkspaceStopError::MissingTrust)?;
        if worker.identity.workflow_id != self.request.workflow_id
            || worker.identity.job_id != self.request.job_id
            || worker.target != self.request.target
            || worker.ssh_public_key != self.request.ssh_public_key
            || !worker.is_valid_for(CloudProvider::Azure)
        {
            return Err(BindingError::InvalidBinding);
        }
        AzureWorker {
            workflow_id: worker.identity.workflow_id,
            job_id: worker.identity.job_id,
            subscription_id: profile.subscription_id.clone(),
            resource_group: resource_group_name(worker.identity.workflow_id, worker.identity.job_id),
            group_id: worker.identity.resource_id.clone(),
            image: worker.target.image.clone(),
            lifetime: worker.lifetime.clone(),
        }
        .validate()
        .map_err(|_| BindingError::InvalidBinding)?;
        if !ssh.is_complete() {
            return Err(RemoteWorkspaceStopError::MissingTrust.into());
        }
        Ok(())
    }

    /// The production observer: the CLI credential pinned to the profile's subscription
    /// and the Azure client over it. Both are lazy; no token is requested or persisted
    /// here, and a missing CLI login surfaces as an unverified observation. Callers
    /// reach this only through [`azure_with`], after admission.
    pub(super) fn client(store: &CloudWorkflowStore, profile: &AzureProfile) -> Result<AzureClient, BindingError> {
        let credential =
            AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| BindingError::InvalidBinding)?;
        AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| BindingError::InvalidBinding)
    }

    /// The exact allocation, its binding and the absence of a storage selection must
    /// all still hold: any drift around the observation is a changed state, not a result.
    pub(super) fn check_current(
        &self,
        store: &CloudWorkflowStore,
        expected: &StoredRemoteAllocation,
    ) -> Result<(), BindingError> {
        let current = store
            .load_remote_allocation(
                expected.workspace().session_id(),
                &expected.workspace().state().spec.workspace_local_id,
            )
            .map_err(RemoteWorkspaceStopError::from)?;
        if current.as_ref() != Some(expected)
            || store
                .load_remote_cpu_profile_binding(expected)
                .map_err(RemoteWorkspaceStopError::from)?
                .as_ref()
                != Some(&self.binding)
            || store
                .load_remote_network_volume_selection(expected)
                .map_err(RemoteWorkspaceStopError::from)?
                .is_some()
        {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        Ok(())
    }
}

/// The admitted observer: every observation is followed, before it reaches the shared
/// coordinator, by the same binding recheck admission ran, so a binding that changed
/// while the control plane was being read can never be completed as a retained Stop.
pub(super) struct BoundObserver<'a, P> {
    inner: P,
    store: &'a CloudWorkflowStore,
    admitted: &'a RetainedAzure,
    drifted: AtomicBool,
}

impl<P> BoundObserver<'_, P> {
    fn drifted(&self) -> bool {
        self.drifted.load(Ordering::SeqCst)
    }
}

/// The provider's own error, or the saved binding drifting during an observation.
#[derive(Debug)]
pub(super) enum BoundError<E> {
    Provider(E),
    Drift,
}

impl<E: std::fmt::Display> std::fmt::Display for BoundError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::Drift => formatter.write_str("saved Azure binding changed during the observation"),
        }
    }
}

impl<E: std::error::Error> std::error::Error for BoundError<E> {}

impl<P: InteractiveWorkerProvider> InteractiveWorkerProvider for BoundObserver<'_, P> {
    type Error = BoundError<P::Error>;
    fn provider(&self) -> CloudProvider {
        self.inner.provider()
    }
    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        self.inner.ensure_worker(request).map_err(BoundError::Provider)
    }
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.inner.inspect_worker(worker).map_err(BoundError::Provider)
    }
    fn reconcile_worker(
        &self,
        request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.inner.reconcile_worker(request).map_err(BoundError::Provider)
    }
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.inner.delete_worker(worker).map_err(BoundError::Provider)
    }
}

impl<P: InteractiveWorkerStopObserver> InteractiveWorkerStopObserver for BoundObserver<'_, P> {
    fn observe_worker_stop(
        &self,
        expected: InteractiveWorkerStopExpectation<'_>,
    ) -> Result<InteractiveWorkerStopObservation, Self::Error> {
        let observation = self.inner.observe_worker_stop(expected).map_err(BoundError::Provider);
        if self
            .admitted
            .check_current(self.store, &self.admitted.allocation)
            .is_err()
        {
            self.drifted.store(true, Ordering::SeqCst);
            return Err(BoundError::Drift);
        }
        observation
    }
}

/// Admission, then the observer, then the shared coordinator, with the saved state
/// rechecked before the observer exists, inside every observation and after the
/// coordinator returned, even on error. `client` and `confirm` are injectable so tests
/// run the real ordering without the Azure CLI or ARM.
pub(super) fn azure_with<P: InteractiveWorkerStopObserver>(
    store: &CloudWorkflowStore,
    profile: &AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    confirm: impl FnOnce(
        &BoundObserver<'_, P>,
        &StoredRemoteAllocation,
    ) -> Result<RemoteWorkspaceStopConfirmation, RemoteWorkspaceStopError>,
) -> Result<ConfiguredStopConfirmation, BindingError> {
    let admitted = RetainedAzure::load(store, profile, expected)?;
    let requested = admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.phase.stop_requested_at_millis())
        .ok_or(RemoteWorkspaceStopError::MissingStopIntent)?;
    if requested > current_millis()? {
        return Err(RemoteWorkspaceStopError::InvalidTimestamp.into());
    }
    // The client (and its lazy credential) is built first, then the saved state is
    // rechecked, so drift during that step is a changed state even when the client
    // could not be built.
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = BoundObserver {
        inner: provider?,
        store,
        admitted: &admitted,
        drifted: AtomicBool::new(false),
    };
    let result = confirm(&bound, &admitted.allocation);
    if bound.drifted() {
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    let current = result
        .as_ref()
        .map_or(&admitted.allocation, |result| &result.allocation);
    admitted.check_current(store, current)?;
    let result = result?;
    Ok(ConfiguredStopConfirmation {
        saved: result.allocation.workspace().environment_summary(),
        observation: result.observation,
    })
}
