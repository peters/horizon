//! Explicit Start of a saved-Stopped Azure worker's retained compute from the overview:
//! eligibility, the background callback and the result typing. Painting grants nothing.

use super::{
    CloudProvider, CloudWorkflowStore, ConfiguredAzureStart, ConfiguredAzureStartError, HorizonHome,
    RemoteEnvironmentSummary, RemoteProviderConfig, RemoteRuntimePhase, StopError, StopNotice, WorkerLifetime,
    same_target,
};
use horizon_core::{
    cloud_run::interactive_worker::InteractiveWorkerLifecycle,
    remote_workspace::start::start_configured_azure_environment,
};

/// Explicit Start is offered only for a retained persistent Azure worker whose saved
/// identity is Azure and whose saved phase is a verified Stop or existing Start intent
/// (a retry). A Stop still in flight is checked first, never started over.
pub(super) fn start_supported(summary: &RemoteEnvironmentSummary) -> bool {
    summary.provider == CloudProvider::Azure
        && summary.lifetime == WorkerLifetime::Persistent
        && summary
            .worker_identity
            .as_ref()
            .is_some_and(|identity| identity.provider == CloudProvider::Azure)
        && matches!(
            summary.saved_phase,
            Some(RemoteRuntimePhase::Stopped { .. } | RemoteRuntimePhase::Starting { .. })
        )
}

pub(super) fn execute_start(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredAzureStart, StopError> {
    if !start_supported(expected) {
        return Err(StopError::AzureStart(ConfiguredAzureStartError::UnsupportedProvider));
    }
    // Intent and the renewed observation need a writer, but a Start must not initialize
    // or migrate storage.
    let store = CloudWorkflowStore::open_existing_without_migration(home).map_err(|_| StopError::StorageUnavailable)?;
    start_configured_azure_environment(&store, config, expected).map_err(StopError::AzureStart)
}

/// A verified start writes intent (unless it already existed) and then the renewed
/// observation: the same record with exactly those revisions and the saved phase
/// `Reconciling`, nothing else changed.
pub(super) fn valid_start_result(expected: &RemoteEnvironmentSummary, result: &ConfiguredAzureStart) -> bool {
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
    pub(super) fn started(expected: RemoteEnvironmentSummary, result: Result<ConfiguredAzureStart, StopError>) -> Self {
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
