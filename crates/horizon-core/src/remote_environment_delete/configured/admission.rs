use super::{ConfiguredEnvironmentDeletion, Error, Operation, RemoteEnvironmentDeleteError as DeleteError};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, RemoteCpuProfileBinding, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerProvider,
            InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
        interactive_worker_delete::{InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation},
        runpod::RunPodNetworkVolumeExpectation,
    },
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::{RemoteEnvironmentSummary, RemoteRuntimePhase},
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(super) struct Admitted {
    pub(super) allocation: StoredRemoteAllocation,
    pub(super) request: InteractiveWorkerRequest,
    pub(super) network: Option<RunPodNetworkVolumeExpectation>,
    pub(super) cpu: Option<RemoteCpuProfileBinding>,
}

impl Admitted {
    fn load(
        store: &CloudWorkflowStore,
        config: &RemoteProviderConfig,
        expected: &RemoteEnvironmentSummary,
        operation: Operation,
    ) -> Result<Self, Error> {
        let allocation = store
            .load_remote_allocation(&expected.owning_session_id, &expected.workspace_local_id)?
            .ok_or(DeleteError::MissingAllocation)?;
        if allocation.workspace().environment_summary() != *expected {
            return Err(DeleteError::StateChanged.into());
        }
        let request = allocation.worker_request()?;
        let worker = super::super::retained_worker(&allocation)?;
        if !request.is_valid_for(expected.provider)
            || !worker.is_valid_for(expected.provider)
            || worker.identity.workflow_id != request.workflow_id
            || worker.identity.job_id != request.job_id
            || worker.target != request.target
            || worker.ssh_public_key != request.ssh_public_key
        {
            return Err(Error::InvalidBinding);
        }
        if request.target.lifetime != WorkerLifetime::Persistent {
            return Err(DeleteError::UnsupportedLifetime.into());
        }
        let runtime = allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(Error::InvalidBinding)?;
        let allowed = match operation {
            Operation::Delete => {
                runtime.cleanup.is_none()
                    && matches!(
                        runtime.phase,
                        RemoteRuntimePhase::Ready
                            | RemoteRuntimePhase::Reconciling
                            | RemoteRuntimePhase::Failed
                            | RemoteRuntimePhase::Stopped { .. }
                    )
            }
            Operation::Check => matches!(
                runtime.phase,
                RemoteRuntimePhase::DeleteRequested { .. } | RemoteRuntimePhase::Deleted { .. }
            ),
            Operation::Retry => matches!(runtime.phase, RemoteRuntimePhase::DeleteRequested { .. }),
        };
        if !allowed {
            return Err(match operation {
                Operation::Delete => DeleteError::ManagementConflict,
                Operation::Check | Operation::Retry => DeleteError::MissingDeleteIntent,
            }
            .into());
        }
        if let Some(requested) = runtime.phase.delete_requested_at_millis()
            && requested > super::super::current_millis()?
        {
            return Err(DeleteError::InvalidTimestamp.into());
        }
        let admitted = Self {
            network: store.load_remote_network_volume_selection(&allocation)?,
            cpu: store.load_remote_cpu_profile_binding(&allocation)?,
            allocation,
            request,
        };
        match expected.provider {
            CloudProvider::RunPod => super::runpod::validate(&admitted, config.runpod_profile(&expected.profile)?)?,
            CloudProvider::Azure => super::azure::validate(&admitted, config.azure_profile(&expected.profile)?)?,
            CloudProvider::LocalDocker => return Err(Error::UnsupportedProvider),
        }
        admitted.check_current(store, &admitted.allocation)?;
        Ok(admitted)
    }

    fn current(&self, store: &CloudWorkflowStore) -> Result<StoredRemoteAllocation, Error> {
        store
            .load_remote_allocation(
                self.allocation.workspace().session_id(),
                &self.allocation.workspace().state().spec.workspace_local_id,
            )?
            .ok_or(DeleteError::MissingAllocation.into())
    }

    fn check_current(&self, store: &CloudWorkflowStore, expected: &StoredRemoteAllocation) -> Result<(), Error> {
        if self.current(store)? != *expected
            || store.load_remote_network_volume_selection(expected)? != self.network
            || store.load_remote_cpu_profile_binding(expected)? != self.cpu
        {
            return Err(DeleteError::StateChanged.into());
        }
        Ok(())
    }
}

pub(super) fn with_provider<P: InteractiveWorkerDeleteObserver>(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
    operation: Operation,
    factory: impl FnOnce(&Admitted) -> Result<P, Error>,
) -> Result<ConfiguredEnvironmentDeletion, Error> {
    let admitted = Admitted::load(store, config, expected, operation)?;
    if operation == Operation::Check && matches!(expected.saved_phase, Some(RemoteRuntimePhase::Deleted { .. })) {
        return Ok(ConfiguredEnvironmentDeletion {
            saved: expected.clone(),
            absence_verified: true,
        });
    }
    let provider = factory(&admitted);
    // Recheck even a failed lazy factory; failure must not conceal saved-state drift.
    admitted.check_current(store, &admitted.allocation)?;
    let provider = provider?;
    if provider.provider() != admitted.request.target.provider {
        return Err(Error::InvalidBinding);
    }
    if expected.provider == CloudProvider::RunPod && operation == Operation::Delete {
        let observation = provider.observe_worker_deletion(super::super::retained_worker(&admitted.allocation)?);
        admitted.check_current(store, &admitted.allocation)?;
        if !matches!(observation, Ok(InteractiveWorkerDeletionObservation::Present)) {
            return Err(Error::UnverifiedRunPodContext);
        }
    }
    let bound = Bound {
        inner: provider,
        store,
        admitted: &admitted,
        revision: AtomicU64::new(admitted.allocation.workspace().revision()),
        may_delete: AtomicBool::new(operation != Operation::Check),
        drifted: AtomicBool::new(false),
        witnessed: AtomicBool::new(expected.provider == CloudProvider::Azure || operation == Operation::Delete),
        deleted: AtomicBool::new(expected.provider == CloudProvider::Azure),
        unverified_context: AtomicBool::new(false),
    };
    let result = match operation {
        Operation::Delete => super::super::delete_remote_environment(store, &bound, &admitted.allocation),
        Operation::Check => super::super::confirm_remote_environment_deletion(store, &bound, &admitted.allocation),
        Operation::Retry => super::super::retry_remote_environment_deletion(store, &bound, &admitted.allocation),
    };
    if bound.drifted.load(Ordering::SeqCst) {
        return Err(DeleteError::StateChanged.into());
    }
    if bound.unverified_context.load(Ordering::SeqCst) {
        return Err(Error::UnverifiedRunPodContext);
    }
    // Every provider result was fenced before the coordinator's exact CAS. Do not
    // turn a written completion into a late, contradictory validation failure.
    let result = result?;
    Ok(ConfiguredEnvironmentDeletion {
        saved: result.allocation.workspace().environment_summary(),
        absence_verified: result.absence_verified,
    })
}

struct Bound<'a, P> {
    inner: P,
    store: &'a CloudWorkflowStore,
    admitted: &'a Admitted,
    revision: AtomicU64,
    may_delete: AtomicBool,
    drifted: AtomicBool,
    witnessed: AtomicBool,
    deleted: AtomicBool,
    unverified_context: AtomicBool,
}

impl<P> Bound<'_, P> {
    fn snapshot(&self) -> Result<StoredRemoteAllocation, Error> {
        let current = self.admitted.current(self.store)?;
        let mut expected_state = self.admitted.allocation.workspace().state().clone();
        let expected_runtime = expected_state.runtime.as_mut().ok_or(Error::InvalidBinding)?;
        let runtime = current
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(Error::InvalidBinding)?;
        if !matches!(runtime.phase, RemoteRuntimePhase::DeleteRequested { .. })
            || expected_runtime
                .phase
                .delete_requested_at_millis()
                .is_some_and(|time| runtime.phase.delete_requested_at_millis() != Some(time))
        {
            return Err(DeleteError::StateChanged.into());
        }
        // Only the coordinator's one intended revision/phase/cleanup transition is
        // allowed. All other workspace and workflow fields remain exactly admitted.
        expected_runtime.phase = runtime.phase;
        expected_runtime.cleanup.clone_from(&runtime.cleanup);
        if current.workspace().revision() != self.revision.load(Ordering::SeqCst)
            || current.workspace().state() != &expected_state
            || current.workflow() != self.admitted.allocation.workflow()
        {
            return Err(DeleteError::StateChanged.into());
        }
        self.admitted.check_current(self.store, &current)?;
        Ok(current)
    }

    fn guarded<T>(&self, call: impl FnOnce(&P) -> Result<T, BoundError>) -> Result<T, BoundError> {
        if self.drifted.load(Ordering::SeqCst) {
            return Err(BoundError);
        }
        let before = self.snapshot().map_err(|_| {
            self.drifted.store(true, Ordering::SeqCst);
            BoundError
        })?;
        let result = call(&self.inner);
        if self.admitted.check_current(self.store, &before).is_err() {
            self.drifted.store(true, Ordering::SeqCst);
            return Err(BoundError);
        }
        result
    }
}

#[derive(Debug, thiserror::Error)]
#[error("configured deletion provider or saved binding is unavailable")]
struct BoundError;

impl<P: InteractiveWorkerProvider> InteractiveWorkerProvider for Bound<'_, P> {
    type Error = BoundError;
    fn provider(&self) -> CloudProvider {
        self.admitted.request.target.provider
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        Err(BoundError)
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(BoundError)
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(BoundError)
    }
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        if !self.may_delete.swap(false, Ordering::SeqCst)
            || self
                .revision
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |revision| revision.checked_add(1))
                .is_err()
        {
            self.drifted.store(true, Ordering::SeqCst);
            return Err(BoundError);
        }
        let result = self.guarded(|provider| provider.delete_worker(worker).map_err(|_| BoundError))?;
        // RunPod's Deleted requires HTTP 204 plus its bounded absence check.
        // Failed/AlreadyAbsent dispatch supplies no acknowledged-mutation witness.
        if result == InteractiveWorkerCleanup::Deleted {
            self.deleted.store(true, Ordering::SeqCst);
        }
        Ok(result)
    }
}

impl<P: InteractiveWorkerDeleteObserver> InteractiveWorkerDeleteObserver for Bound<'_, P> {
    fn observe_worker_deletion(
        &self,
        worker: &InteractiveWorker,
    ) -> Result<InteractiveWorkerDeletionObservation, Self::Error> {
        let observation = self.guarded(|provider| provider.observe_worker_deletion(worker).map_err(|_| BoundError))?;
        match observation {
            InteractiveWorkerDeletionObservation::Present => self.witnessed.store(true, Ordering::SeqCst),
            InteractiveWorkerDeletionObservation::Absent
                if !self.witnessed.load(Ordering::SeqCst) || !self.deleted.load(Ordering::SeqCst) =>
            {
                self.unverified_context.store(true, Ordering::SeqCst);
                return Err(BoundError);
            }
            InteractiveWorkerDeletionObservation::Absent => {}
        }
        Ok(observation)
    }
}
