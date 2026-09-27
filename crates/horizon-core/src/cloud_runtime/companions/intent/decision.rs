use super::{Action, Binding, Origin, State};
use crate::cloud_runtime::{Stage, state::Deployment};
use horizon_cloud::{CreateState, WorkerStatus};

/// Trusted controller observations, never a remote caller's assertion.
#[derive(Clone, Copy)]
pub enum Observation<'a> {
    /// Check durable provider deletion fences as well as the deployment record:
    /// Hetzner deleting/terminated-volume and `RunPod` volume Deleting/Deleted.
    /// Missing required storage journals must fail observation, never mean Missing.
    /// This takes priority even when deployment.json is absent or still says Ready.
    DeletionPending,
    Missing,
    Existing(&'a Deployment),
    /// Reconciliation proved the recorded worker disappeared.
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Deleted,
    Lost,
    IdentityMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Reuse,
    Provision,
    ResumeThenReconnect,
    /// Hetzner resume allocates a new server on the retained volume; surface its cost.
    ResumeWithNewServer,
    Reconnect,
    VerifyAccess,
    ReconcileOnly,
    Stop,
    AlreadyStopped,
    Refuse(Refusal),
}

/// Refuses deployment-record deletion stages and termination with or without storage.
/// Callers must additionally inspect durable provider deletion journals and supply
/// `Observation::DeletionPending`; deployment stages alone cannot reveal those fences.
#[must_use]
pub fn refuses_deployment(state: &Deployment) -> bool {
    state.stage == Stage::Deleted
        || Stage::DELETION.contains(&state.stage)
        || matches!(state.operation, CreateState::Terminated { .. })
}

/// Pure planning only, for an explicitly authorized direct companion request.
/// `access_ready` means a freshly verified grant from this source to this worker.
/// The service must re-read under the target lock before execution; a decision is
/// neither permission for provider I/O nor evidence that its intent was saved.
#[must_use]
pub fn decide(
    action: Action,
    binding: &Binding,
    observation: Observation<'_>,
    intent: State,
    access_ready: bool,
) -> Decision {
    let state = match observation {
        Observation::DeletionPending => return Decision::Refuse(Refusal::Deleted),
        Observation::Lost => return Decision::Refuse(Refusal::Lost),
        Observation::Existing(state) => {
            if state.cloud_id != binding.target.cloud_id || state.repository != binding.checkout {
                return Decision::Refuse(Refusal::IdentityMismatch);
            }
            if refuses_deployment(state) {
                return Decision::Refuse(Refusal::Deleted);
            }
            Some(state)
        }
        Observation::Missing if binding.origin == Origin::Existing => return Decision::Refuse(Refusal::Lost),
        Observation::Missing => None,
    };
    // Terminal operation IDs are status results, not a new execution authorization.
    if intent != State::Submitted {
        return Decision::ReconcileOnly;
    }
    let Some(state) = state else {
        return match action {
            Action::EnsureReady => Decision::Provision,
            Action::Stop => Decision::AlreadyStopped,
        };
    };
    if matches!(state.operation, CreateState::Requested)
        || state.stage == Stage::Stopping
        || state.stage == Stage::Replace
        || state.image_replacement.is_some()
    {
        return Decision::ReconcileOnly;
    }
    match &state.operation {
        CreateState::Bound { worker_id } => {
            if state.worker.as_ref().is_some_and(|worker| &worker.id != worker_id) {
                return Decision::Refuse(Refusal::IdentityMismatch);
            }
            let released = state.profile.provider == horizon_cloud::hetzner::PROVIDER && state.stage == Stage::Stopped;
            if !released && state.worker.is_none() {
                return Decision::ReconcileOnly;
            }
            if !released
                && state
                    .worker
                    .as_ref()
                    .is_some_and(|worker| worker.status() == WorkerStatus::Lost)
            {
                return Decision::Refuse(Refusal::Lost);
            }
            let stopped = state.stage == Stage::Stopped
                || state
                    .worker
                    .as_ref()
                    .is_some_and(|worker| worker.status() == WorkerStatus::Stopped);
            match action {
                Action::Stop if stopped => Decision::AlreadyStopped,
                Action::Stop => Decision::Stop,
                Action::EnsureReady if stopped || state.stop_requested => {
                    if released {
                        Decision::ResumeWithNewServer
                    } else {
                        Decision::ResumeThenReconnect
                    }
                }
                Action::EnsureReady if state.worker_ready() => {
                    if access_ready {
                        Decision::Reuse
                    } else {
                        Decision::VerifyAccess
                    }
                }
                Action::EnsureReady => Decision::Reconnect,
            }
        }
        CreateState::Prepared if state.worker.is_none() && !state.stop_requested => match action {
            Action::EnsureReady => Decision::Reconnect,
            Action::Stop => Decision::AlreadyStopped,
        },
        _ => Decision::ReconcileOnly,
    }
}
