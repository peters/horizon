//! Explicit Start of a saved-Stopped cloud worker's retained compute from the overview:
//! eligibility, the background callback and the result typing. Painting grants nothing.

use super::{
    CloudProvider, CloudWorkflowStore, ConfiguredAzureStartError, ConfiguredRunPodStartError, ConfiguredStart,
    HorizonHome, RemoteEnvironmentSummary, RemoteProviderConfig, RemoteRuntimePhase, StopError, StopNotice,
    WorkerLifetime, same_target,
};
use horizon_core::{
    cloud_run::interactive_worker::InteractiveWorkerLifecycle,
    remote_workspace::start::{start_configured_azure_environment, start_configured_runpod_environment},
};

/// Explicit Start is offered only for a supported retained persistent worker whose saved
/// identity matches its provider and whose phase is a verified Stop or existing Start intent
/// (a retry). A Stop still in flight is checked first, never started over.
pub(super) fn start_supported(summary: &RemoteEnvironmentSummary) -> bool {
    (summary.provider == CloudProvider::Azure
        || (cfg!(target_os = "linux") && summary.provider == CloudProvider::RunPod))
        && summary.lifetime == WorkerLifetime::Persistent
        && summary
            .worker_identity
            .as_ref()
            .is_some_and(|identity| identity.provider == summary.provider)
        && matches!(
            summary.saved_phase,
            Some(RemoteRuntimePhase::Stopped { .. } | RemoteRuntimePhase::Starting { .. })
        )
}

pub(super) fn execute_start(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredStart, StopError> {
    if !start_supported(expected) {
        return Err(if expected.provider == CloudProvider::RunPod {
            StopError::RunPodStart(ConfiguredRunPodStartError::UnsupportedProvider)
        } else {
            StopError::AzureStart(ConfiguredAzureStartError::UnsupportedProvider)
        });
    }
    // Intent and the renewed observation need a writer, but a Start must not initialize
    // or migrate storage.
    let store = CloudWorkflowStore::open_existing_without_migration(home).map_err(|_| StopError::StorageUnavailable)?;
    match expected.provider {
        CloudProvider::RunPod => {
            start_configured_runpod_environment(&store, config, expected).map_err(StopError::RunPodStart)
        }
        _ => start_configured_azure_environment(&store, config, expected).map_err(StopError::AzureStart),
    }
}

/// A verified start writes intent (unless it already existed) and then the renewed
/// observation: the same record with exactly those revisions and the saved phase
/// `Reconciling`, nothing else changed.
pub(super) fn valid_start_result(expected: &RemoteEnvironmentSummary, result: &ConfiguredStart) -> bool {
    let steps = match expected.saved_phase {
        Some(RemoteRuntimePhase::Stopped { .. }) => 2,
        Some(RemoteRuntimePhase::Starting { .. }) => 1,
        _ => return false,
    };
    let Some(revision) = expected.revision.checked_add(steps) else {
        return false;
    };
    let mut allowed = expected.clone();
    allowed.revision = revision;
    allowed.saved_phase = Some(RemoteRuntimePhase::Reconciling);
    start_supported(expected) && same_target(expected, &result.saved) && result.saved == allowed
}

impl StopNotice {
    pub(super) fn started(expected: RemoteEnvironmentSummary, result: Result<ConfiguredStart, StopError>) -> Self {
        let result = result.and_then(|result| {
            if !valid_start_result(&expected, &result) {
                return Err(StopError::SelectionChanged);
            }
            Ok(result)
        });
        let succeeded = result.is_ok();
        let unverified = matches!(
            result,
            Err(StopError::AzureStart(ConfiguredAzureStartError::Start(
                horizon_core::remote_workspace::start::RemoteWorkspaceStartError::ProviderUnavailable
            )) | StopError::RunPodStart(ConfiguredRunPodStartError::Start(
                horizon_core::remote_workspace::start::RemoteWorkspaceStartError::ProviderUnavailable
            )))
        );
        let message = match result {
            Ok(started) => {
                let compute = if started.already_running {
                    "The worker was already running; nothing was re-posted."
                } else {
                    "Compute started for the same worker under its saved identity."
                };
                let endpoint = match started.lifecycle {
                    InteractiveWorkerLifecycle::Ready => "The attested endpoint matches the saved pin.",
                    _ => "The endpoint is not attested yet; the worker is running but may not be reachable for a while.",
                };
                format!(
                    "{compute} {endpoint} Saved phase is Reconciling: reconnect session panels to continue. Nothing resumed a task; in-memory work did not survive the stop."
                )
            }
            Err(StopError::WorkerUnavailable) => {
                "The Start could not finish locally. Refresh saved inventory; if Start intent remains, press Start again."
                    .into()
            }
            Err(StopError::StorageUnavailable) => "The saved environment could not be safely accessed for Start. No Start request was dispatched.".into(),
            Err(StopError::SelectionChanged) => "The Start result does not match this environment. Refresh saved inventory; compute may already be billing.".into(),
            Err(error) => error.message(),
        };
        Self {
            expected,
            message,
            succeeded,
            checked: false,
            started: true,
            unverified,
        }
    }
}
