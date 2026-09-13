//! Pure validation and cached presentation of the configured coordinator's result.

use super::{
    ConfiguredEnvironmentDeletion, Error, HorizonHome, Operation, RemoteEnvironmentSummary, RemoteProviderConfig,
    Request, Scope,
};
use horizon_core::{
    cloud_run::{CloudProvider, WorkerLifetime},
    remote_workspace::RemoteRuntimePhase,
};

pub(super) fn supported(summary: &RemoteEnvironmentSummary, operation: Operation) -> bool {
    if !matches!(summary.provider, CloudProvider::RunPod | CloudProvider::Azure)
        || summary.lifetime != WorkerLifetime::Persistent
        || !summary.worker_identity.as_ref().is_some_and(|identity| {
            identity.provider == summary.provider
                && Some(identity.workflow_id) == summary.workflow_id
                && Some(identity.job_id) == summary.job_id
                && !identity.resource_id.is_empty()
        })
    {
        return false;
    }
    matches!(
        (operation, summary.saved_phase),
        (
            Operation::Delete,
            Some(
                RemoteRuntimePhase::Ready
                    | RemoteRuntimePhase::Reconciling
                    | RemoteRuntimePhase::Failed
                    | RemoteRuntimePhase::Stopped { .. }
            )
        ) | (
            Operation::Check | Operation::Retry,
            Some(RemoteRuntimePhase::DeleteRequested { .. })
        )
    )
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Status {
    Pending,
    Verified,
    Historical,
    Unverified,
}

pub(super) struct Notice {
    scope: Scope,
    saved: Option<RemoteEnvironmentSummary>,
    pub(super) status: Status,
    pub(super) message: String,
}

impl Notice {
    pub(super) fn finish(request: Request, result: Result<ConfiguredEnvironmentDeletion, Error>) -> Self {
        let result = result.and_then(|result| validate(&request, &result).map(|status| (status, result.saved)));
        let (status, saved, message) = match result {
            Ok((Status::Pending, saved)) => (Status::Pending, Some(saved), "Delete remains pending: the resource is present. This does not establish that the earlier request failed.".into()),
            Ok((Status::Verified, saved)) => (Status::Verified, Some(saved), "Exact resource absence was verified and saved. This is a point-in-time result, not continuous monitoring or backup proof.".into()),
            Ok((Status::Historical, saved)) => (Status::Historical, Some(saved), "Previously verified worker absence (saved). No provider was contacted for this historical result.".into()),
            Ok((Status::Unverified, _)) | Err(Error::ResultMismatch) => (Status::Unverified, None, "The result does not match the saved deletion request. Refresh saved inventory; no completion is asserted.".into()),
            Err(Error::Storage) => (Status::Unverified, None, "Cannot safely open the existing workflow store. No provider request was made; no store was created or migrated.".into()),
            Err(Error::Worker) => (Status::Unverified, None, "The Delete operation could not report completion. Its provider outcome is unknown; refresh saved inventory before any further action.".into()),
            Err(Error::Core(error)) => (Status::Unverified, None, format!("Deletion is unverified: {error}")),
        };
        Self {
            scope: request.scope,
            saved,
            status,
            message,
        }
    }

    pub(super) fn matches(
        &self,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: Option<&RemoteEnvironmentSummary>,
    ) -> bool {
        self.scope.home == *home
            && self.scope.config == *config
            && (selected == Some(&self.scope.expected)
                || self.saved.as_ref().is_some_and(|saved| selected == Some(saved)))
    }
}

fn validate(request: &Request, result: &ConfiguredEnvironmentDeletion) -> Result<Status, Error> {
    use RemoteRuntimePhase::{DeleteRequested, Deleted};
    let expected = &request.scope.expected;
    if request.operation == Operation::Check && matches!(expected.saved_phase, Some(Deleted { .. })) {
        return if result.absence_verified && result.saved == *expected {
            Ok(Status::Historical)
        } else {
            Err(Error::ResultMismatch)
        };
    }
    if !supported(expected, request.operation) {
        return Err(Error::ResultMismatch);
    }
    let (requested, observed) = match result.saved.saved_phase {
        Some(DeleteRequested { requested_at_millis }) if !result.absence_verified => (requested_at_millis, None),
        Some(Deleted {
            requested_at_millis,
            observed_at_millis,
        }) if result.absence_verified && observed_at_millis >= requested_at_millis => {
            (requested_at_millis, Some(observed_at_millis))
        }
        _ => return Err(Error::ResultMismatch),
    };
    if requested < 0
        || (request.operation != Operation::Delete
            && expected.saved_phase
                != Some(DeleteRequested {
                    requested_at_millis: requested,
                }))
    {
        return Err(Error::ResultMismatch);
    }
    let delta = result
        .saved
        .revision
        .checked_sub(expected.revision)
        .ok_or(Error::ResultMismatch)?;
    let valid_delta = match (request.operation, observed.is_some()) {
        (Operation::Delete | Operation::Retry, false) | (Operation::Check, true) => delta == 1,
        (Operation::Delete, true) => delta == 2,
        (Operation::Check, false) => delta == 0,
        (Operation::Retry, true) => matches!(delta, 1 | 2),
    };
    let mut allowed = expected.clone();
    allowed.revision = result.saved.revision;
    allowed.saved_phase = result.saved.saved_phase;
    if !valid_delta || allowed != result.saved {
        return Err(Error::ResultMismatch);
    }
    Ok(if observed.is_some() {
        Status::Verified
    } else {
        Status::Pending
    })
}
