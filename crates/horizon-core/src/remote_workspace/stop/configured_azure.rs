//! Configured Azure Stop admission, shared by the first explicit Stop and the saved-Stop
//! check: the exact named profile, its immutable CPU binding, the retained persistent
//! worker with its complete Azure handle and public pin are bound before the CLI
//! credential or client exists. Neither path creates, starts, deletes, repairs a private
//! key or attests a host key; the check never sends Stop, and Stop is sent once.

use super::{
    ConfiguredStopConfirmation, ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopConfirmation,
    RemoteWorkspaceStopError, current_millis, stop_allocation,
};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, RemoteCpuProfileBinding, StoredRemoteAllocation, WorkerLifetime,
        azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile, AzureWorker, resource_group_name},
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerProvider,
            InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
        interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
        interactive_worker_stop::{
            InteractiveWorkerStop, InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation,
            InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
        },
    },
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Send one explicitly confirmed Stop for an exact retained persistent Azure worker.
/// Run off the render thread after explicit confirmation. Requires the named
/// `remote.azure` profile, the allocation's immutable CPU profile binding, the retained
/// worker with its complete handle and complete saved public pin, and no `RunPod` storage
/// expectation, all before the subscription-pinned CLI credential or client exists. The
/// pin is saved and shape-checked, not freshly attested; no private SSH key is needed.
/// Durable intent precedes the provider call through the shared coordinator. Once intent
/// exists this entry point refuses another Stop: use the saved-Stop check after
/// uncertainty. Does not create, start, delete, poll beyond the coordinator's
/// verification, or repair identity. Verified Stop is point-in-time deallocated compute
/// with the retained data disk, not a backup, task checkpoint, process-memory
/// preservation, filesystem durability or proof that disk billing has ceased.
/// # Errors
/// Rejects unsupported, stale, missing or malformed selections, profile, binding, pin
/// or storage drift, competing management, existing intent and an unverified Stop.
/// Failures after dispatch retain intent and identity; refresh and check it.
pub fn stop_configured_azure_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentSummary, ConfiguredAzureStopError> {
    if expected.provider != CloudProvider::Azure {
        return Err(ConfiguredAzureStopError::UnsupportedProvider);
    }
    let profile = config.azure_profile(&expected.profile)?;
    stop_with(
        store,
        profile,
        expected,
        |_admitted| RetainedAzure::client(store, profile),
        |provider, allocation| stop_allocation(store, provider, allocation),
    )
}

/// Admission, then the provider, then the shared Stop coordinator. Existing intent is
/// refused before the client exists; the saved state is rechecked after client
/// construction; the binding and storage are rechecked inside the provider call, after
/// intent was recorded and before completion can be written; nothing fallible follows
/// a written completion. `client` and `stop` are injectable so tests run the real
/// ordering without the Azure CLI or ARM.
pub(super) fn stop_with<P: InteractiveWorkerStopProvider>(
    store: &CloudWorkflowStore,
    profile: &AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    stop: impl FnOnce(&Bound<'_, P>, &StoredRemoteAllocation) -> Result<StoredRemoteAllocation, RemoteWorkspaceStopError>,
) -> Result<RemoteEnvironmentSummary, ConfiguredAzureStopError> {
    let admitted = RetainedAzure::load(store, profile, expected)?;
    if admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(BindingError::InvalidBinding)?
        .phase
        .stop_requested_at_millis()
        .is_some()
    {
        return Err(ConfiguredAzureStopError::ExistingStopIntent);
    }
    // A start in flight is resolved first (explicit Start retry, then the saved-Stop
    // check); Stop never races it, and the refusal precedes the client.
    if admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.phase.start_requested_at_millis().is_some())
    {
        return Err(ConfiguredAzureStopError::Stop(
            RemoteWorkspaceStopError::ManagementConflict,
        ));
    }
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound::new(provider?, store, &admitted);
    let result = stop(&bound, &admitted.allocation);
    if bound.drifted() {
        // Intent is recorded and the provider may have been asked; completion was not
        // written. The saved-Stop check is the only way forward.
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    Ok(result?.workspace().environment_summary())
}

/// One admitted Azure allocation: exact summary, valid public request, persistent
/// target within the named profile, immutable binding to that profile, no `RunPod`
/// storage expectation, no management intent, and a retained worker whose complete
/// Azure handle validates under the profile's subscription with a complete pin.
pub(in crate::remote_workspace) struct RetainedAzure {
    pub(in crate::remote_workspace) allocation: StoredRemoteAllocation,
    request: InteractiveWorkerRequest,
    binding: RemoteCpuProfileBinding,
}

impl RetainedAzure {
    pub(in crate::remote_workspace) fn load(
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
    pub(in crate::remote_workspace) fn client(
        store: &CloudWorkflowStore,
        profile: &AzureProfile,
    ) -> Result<AzureClient, BindingError> {
        let credential =
            AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| BindingError::InvalidBinding)?;
        AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| BindingError::InvalidBinding)
    }

    /// The binding and the absence of a storage selection must still hold for the
    /// allocation as it is now; used where the coordinator has already advanced the
    /// allocation (intent recorded) and fences it with its own CAS.
    pub(in crate::remote_workspace) fn check_binding(&self, store: &CloudWorkflowStore) -> Result<(), BindingError> {
        let current = store
            .load_remote_allocation(
                self.allocation.workspace().session_id(),
                &self.allocation.workspace().state().spec.workspace_local_id,
            )
            .map_err(RemoteWorkspaceStopError::from)?
            .ok_or(RemoteWorkspaceStopError::MissingAllocation)?;
        if store
            .load_remote_cpu_profile_binding(&current)
            .map_err(RemoteWorkspaceStopError::from)?
            .as_ref()
            != Some(&self.binding)
            || store
                .load_remote_network_volume_selection(&current)
                .map_err(RemoteWorkspaceStopError::from)?
                .is_some()
        {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        Ok(())
    }

    /// The exact allocation, its binding and the absence of a storage selection must
    /// all still hold: any drift around the observation is a changed state, not a result.
    pub(in crate::remote_workspace) fn check_current(
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

/// The admitted provider: every observation and every Stop is followed, before its
/// answer reaches the shared coordinator, by the binding recheck admission ran, so a
/// binding that changed while the control plane was being read or the Stop was in
/// flight can never be completed as a retained Stop.
pub(in crate::remote_workspace) struct Bound<'a, P> {
    inner: P,
    store: &'a CloudWorkflowStore,
    admitted: &'a RetainedAzure,
    drifted: AtomicBool,
}

impl<'a, P> Bound<'a, P> {
    pub(in crate::remote_workspace) fn new(
        inner: P,
        store: &'a CloudWorkflowStore,
        admitted: &'a RetainedAzure,
    ) -> Self {
        Self {
            inner,
            store,
            admitted,
            drifted: AtomicBool::new(false),
        }
    }

    pub(in crate::remote_workspace) fn drifted(&self) -> bool {
        self.drifted.load(Ordering::SeqCst)
    }

    /// The provider's answer only if the admitted state is still `intact`; otherwise the
    /// drift is recorded and reported instead of the answer.
    fn fence<T>(&self, answer: Result<T, BoundError<P::Error>>, intact: bool) -> Result<T, BoundError<P::Error>>
    where
        P: InteractiveWorkerProvider,
    {
        if !intact {
            self.drifted.store(true, Ordering::SeqCst);
            return Err(BoundError::Drift);
        }
        answer
    }
}

/// The provider's own error, or the saved binding drifting during an observation or Stop.
#[derive(Debug)]
pub(in crate::remote_workspace) enum BoundError<E> {
    Provider(E),
    Drift,
}

impl<E: std::fmt::Display> std::fmt::Display for BoundError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::Drift => formatter.write_str("saved Azure binding changed during the provider call"),
        }
    }
}

impl<E: std::error::Error> std::error::Error for BoundError<E> {}

impl<P: InteractiveWorkerProvider> InteractiveWorkerProvider for Bound<'_, P> {
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

impl<P: InteractiveWorkerStopObserver> InteractiveWorkerStopObserver for Bound<'_, P> {
    fn observe_worker_stop(
        &self,
        expected: InteractiveWorkerStopExpectation<'_>,
    ) -> Result<InteractiveWorkerStopObservation, Self::Error> {
        let observation = self.inner.observe_worker_stop(expected).map_err(BoundError::Provider);
        // The observation writes nothing, so the whole admitted snapshot must still hold.
        self.fence(
            observation,
            self.admitted
                .check_current(self.store, &self.admitted.allocation)
                .is_ok(),
        )
    }
}

impl<P: InteractiveWorkerStopProvider> InteractiveWorkerStopProvider for Bound<'_, P> {
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        let stopped = self.inner.stop_worker(worker).map_err(BoundError::Provider);
        // The coordinator has recorded intent (its own CAS fences the allocation); the
        // binding and storage must still be the admitted ones before completion.
        self.fence(stopped, self.admitted.check_binding(self.store).is_ok())
    }
}

impl<P: InteractiveWorkerStartProvider> InteractiveWorkerStartProvider for Bound<'_, P> {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        let started = self.inner.start_worker(worker).map_err(BoundError::Provider);
        // Start intent is recorded (the coordinator's CAS fences the allocation); the
        // binding and storage must still be the admitted ones before the renewed
        // observation is written.
        self.fence(started, self.admitted.check_binding(self.store).is_ok())
    }
}

/// Admission, then the observer, then the shared coordinator, with the saved state
/// rechecked before the observer exists, inside every observation and, for every
/// outcome that wrote nothing, after the coordinator returned. `client` and `confirm`
/// are injectable so tests run the real ordering without the Azure CLI or ARM.
pub(super) fn azure_with<P: InteractiveWorkerStopObserver>(
    store: &CloudWorkflowStore,
    profile: &AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    confirm: impl FnOnce(
        &Bound<'_, P>,
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
    let bound = Bound::new(provider?, store, &admitted);
    let result = confirm(&bound, &admitted.allocation);
    if bound.drifted() {
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    // Completion is written by the coordinator's CAS against the allocation it re-read
    // after the observation, and the binding was rechecked inside that observation. Once
    // that write exists no fallible validation may follow it: a late error would deny a
    // completion that is already saved. Every non-writing outcome is still fenced here.
    let written = result
        .as_ref()
        .is_ok_and(|result| result.allocation != admitted.allocation);
    if !written {
        admitted.check_current(store, &admitted.allocation)?;
    }
    let result = result?;
    Ok(ConfiguredStopConfirmation {
        saved: result.allocation.workspace().environment_summary(),
        observation: result.observation,
    })
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredAzureStopError {
    #[error("configured Azure Stop is supported only for retained persistent Azure workers")]
    UnsupportedProvider,
    #[error(
        "the configured Azure profile or retained worker, public pin, profile binding and storage binding is invalid"
    )]
    InvalidBinding,
    #[error("Stop intent already exists; use Check saved Stop without sending another Stop request")]
    ExistingStopIntent,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error("Azure Stop could not be verified; refresh saved state and use Check if Stop intent exists")]
    Stop(#[from] RemoteWorkspaceStopError),
}

impl From<BindingError> for ConfiguredAzureStopError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            // The Azure credential is lazy and never consulted during admission.
            BindingError::CredentialUnavailable | BindingError::InvalidBinding => Self::InvalidBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => Self::Stop(error),
        }
    }
}
