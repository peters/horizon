//! Explicit retained `RunPod` compute Start; configured admission precedes credentials.

use super::{ConfiguredStart, RemoteWorkspaceStartError};
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

#[cfg(target_os = "linux")]
use {
    super::start_remote_workspace,
    crate::{
        cloud_run::{
            CloudProvider,
            interactive_worker::{
                InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerProvider,
                InteractiveWorkerRequest, InteractiveWorkerStatus,
            },
            interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
            runpod::{RunPodApiKey, RunPodProfile},
        },
        remote_workspace::{
            RemoteRuntimePhase,
            stop::{
                ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError,
                configured_runpod::RetainedRunPod,
            },
        },
    },
    std::sync::atomic::{AtomicBool, Ordering},
};

/// Start only an explicitly confirmed, retained persistent `RunPod` worker on Linux.
/// Requires the exact named profile, current allocation, complete saved public pin
/// and HPS selection before reading `RUNPOD_API_KEY`. No private SSH key is needed.
/// The shared coordinator records intent before dispatch; an explicit retry uses
/// that intent and never reposts a running worker. A provider call may take up to
/// its existing five-minute observation bound. It does not allocate, replace,
/// refresh endpoints, prepare a repository, delete storage or replay any task.
/// Compute billing resumes; retained HPS is not a backup or durability certificate.
/// # Errors
/// Rejects unsupported platforms/providers, stale or invalid selections, missing
/// HPS/pin/profile, conflicting phases, credential failures and uncertain starts.
/// Post-dispatch failures retain identity and intent for an explicit retry.
pub fn start_configured_runpod_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredStart, ConfiguredRunPodStartError> {
    #[cfg(target_os = "linux")]
    {
        if expected.provider != CloudProvider::RunPod {
            return Err(ConfiguredRunPodStartError::UnsupportedProvider);
        }
        let profile = config.runpod_profile(&expected.profile)?;
        start_with(store, profile, expected, |admitted| {
            let key = RunPodApiKey::from_env().map_err(|_| ConfiguredRunPodStartError::CredentialUnavailable)?;
            admitted.provider(store, profile, &key).map_err(Into::into)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, expected);
        Err(ConfiguredRunPodStartError::UnsupportedProvider)
    }
}

#[cfg(target_os = "linux")]
fn start_with<P: InteractiveWorkerStartProvider>(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedRunPod) -> Result<P, ConfiguredRunPodStartError>,
) -> Result<ConfiguredStart, ConfiguredRunPodStartError> {
    let admitted = RetainedRunPod::load(store, profile, expected)?;
    if admitted.selection.is_none() {
        return Err(ConfiguredRunPodStartError::InvalidBinding);
    }
    let phase = admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(ConfiguredRunPodStartError::InvalidBinding)?
        .phase;
    if !matches!(
        phase,
        RemoteRuntimePhase::Stopped { .. } | RemoteRuntimePhase::Starting { .. }
    ) {
        return Err(RemoteWorkspaceStartError::NotStopped.into());
    }
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound {
        inner: provider?,
        store,
        admitted: &admitted,
        drifted: AtomicBool::new(false),
    };
    let result = start_remote_workspace(store, &bound, &admitted.allocation);
    if bound.drifted.load(Ordering::SeqCst) {
        return Err(RemoteWorkspaceStartError::StateChanged.into());
    }
    let result = result?;
    Ok(ConfiguredStart {
        saved: result.allocation.workspace().environment_summary(),
        lifecycle: result.lifecycle,
        already_running: result.already_running,
    })
}

#[cfg(target_os = "linux")]
struct Bound<'a, P> {
    inner: P,
    store: &'a CloudWorkflowStore,
    admitted: &'a RetainedRunPod,
    drifted: AtomicBool,
}

#[cfg(target_os = "linux")]
impl<P> Bound<'_, P> {
    fn check_binding(&self) -> Result<(), ConfiguredRunPodStartError> {
        if self.admitted.check_binding(self.store).is_err() {
            self.drifted.store(true, Ordering::SeqCst);
            return Err(RemoteWorkspaceStartError::StateChanged.into());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl<P: InteractiveWorkerStartProvider> InteractiveWorkerProvider for Bound<'_, P> {
    type Error = ConfiguredRunPodStartError;
    fn provider(&self) -> CloudProvider {
        self.inner.provider()
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        Err(Self::Error::InvalidBinding)
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(Self::Error::InvalidBinding)
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(Self::Error::InvalidBinding)
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        Err(Self::Error::InvalidBinding)
    }
}

#[cfg(target_os = "linux")]
impl<P: InteractiveWorkerStartProvider> InteractiveWorkerStartProvider for Bound<'_, P> {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        self.check_binding()?;
        let result = self.inner.start_worker(worker);
        // A successful provider reply cannot acknowledge a changed HPS binding.
        // This runs before the coordinator writes Reconciling, even on errors.
        self.check_binding()?;
        result.map_err(|_| RemoteWorkspaceStartError::ProviderUnavailable.into())
    }
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredRunPodStartError {
    #[error("configured RunPod Start requires a retained persistent worker on Linux")]
    UnsupportedProvider,
    #[error("RunPod Start requires a valid RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error("RunPod Start requires the exact named profile, retained worker, public pin and saved HPS binding")]
    InvalidBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error("RunPod Start could not be verified; refresh saved state and retry Start explicitly")]
    Start(#[from] RemoteWorkspaceStartError),
}

#[cfg(target_os = "linux")]
impl From<BindingError> for ConfiguredRunPodStartError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable => Self::CredentialUnavailable,
            BindingError::InvalidBinding => Self::InvalidBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => Self::Start(match error {
                RemoteWorkspaceStopError::MissingAllocation => RemoteWorkspaceStartError::MissingAllocation,
                RemoteWorkspaceStopError::MissingWorker => RemoteWorkspaceStartError::MissingWorker,
                RemoteWorkspaceStopError::MissingTrust => RemoteWorkspaceStartError::MissingTrust,
                RemoteWorkspaceStopError::StateChanged => RemoteWorkspaceStartError::StateChanged,
                RemoteWorkspaceStopError::ProviderMismatch => RemoteWorkspaceStartError::ProviderMismatch,
                RemoteWorkspaceStopError::UnsupportedLifetime => RemoteWorkspaceStartError::UnsupportedLifetime,
                RemoteWorkspaceStopError::ManagementConflict => RemoteWorkspaceStartError::ManagementConflict,
                RemoteWorkspaceStopError::InvalidTimestamp => RemoteWorkspaceStartError::InvalidTimestamp,
                RemoteWorkspaceStopError::StorageUnavailable
                | RemoteWorkspaceStopError::MissingStopIntent
                | RemoteWorkspaceStopError::ProviderUnavailable
                | RemoteWorkspaceStopError::ResourceAbsent => RemoteWorkspaceStartError::StorageUnavailable,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
