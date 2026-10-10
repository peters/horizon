//! What the close dialog offers for one cloud. Its provider resources are deleted
//! first; a cloud without any is removed at once. Only once a deletion failed or
//! cannot run may the cloud leave Horizon while its resources may remain.
use horizon_core::cloud_runtime::{
    CreateState,
    provider::{self, StoppedCost},
};

use super::super::{Runtime, Stage};

pub(super) const BUSY: &str = "Wait for the current cloud operation to finish before closing.";
pub(super) const STATE_UNKNOWN: &str = "Horizon cannot read this cloud's resource state, so it cannot delete them.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Primary {
    /// Delete the provider resources, then remove the cloud.
    Delete,
    /// Nothing is left at the provider: remove the cloud at once.
    Remove,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Offer {
    pub(super) primary: Option<Primary>,
    /// Remove from Horizon anyway, leaving whatever the provider still holds.
    pub(super) remove_anyway: bool,
    /// Why deleting cannot run, or what stopped the last attempt.
    pub(super) reason: Option<String>,
}

/// `failure` is what stopped this close's last deletion or removal.
pub(super) fn offer(runtime: &Runtime, deployment_started: bool, failure: Option<&str>) -> Offer {
    if runtime.busy() {
        return Offer {
            primary: None,
            remove_anyway: false,
            reason: Some(BUSY.into()),
        };
    }
    let state = runtime.state.as_ref();
    if runtime.state_unavailable || (state.is_none() && deployment_started) {
        return Offer {
            primary: None,
            remove_anyway: true,
            reason: Some(failure.unwrap_or(STATE_UNKNOWN).into()),
        };
    }
    if let Some(failure) = failure {
        return Offer {
            primary: state
                .is_some_and(|state| state.spec.is_some())
                .then_some(Primary::Delete),
            remove_anyway: true,
            reason: Some(failure.into()),
        };
    }
    let empty = state.is_none_or(|state| {
        state.stage == Stage::Deleted || (state.operation == CreateState::Prepared && state.spec.is_none())
    });
    Offer {
        primary: Some(if empty { Primary::Remove } else { Primary::Delete }),
        remove_anyway: false,
        reason: None,
    }
}

/// What may be left at the provider of a cloud on `provider_id` when it leaves Horizon
/// without a finished deletion, named as far as its record knows.
pub(super) fn remains(runtime: &Runtime, provider_id: &str) -> String {
    let Some(provider) = provider::by_id(provider_id) else {
        return "Its worker and storage may still exist at the provider and cost money until you delete them there."
            .into();
    };
    let worker = match runtime.state.as_ref().map(|state| &state.operation) {
        Some(CreateState::Bound { worker_id }) => Some(format!("{} {worker_id}", noun(provider.stopped))),
        Some(CreateState::Terminated { .. }) => None,
        _ => Some(format!("A {}", noun(provider.stopped).to_lowercase())),
    };
    let storage = match provider.stopped {
        StoppedCost::WorkerKept => "workspace storage",
        StoppedCost::ServerDeleted => "workspace volume and SSH key",
    };
    let what = worker.map_or_else(
        || format!("The {storage}"),
        |worker| match provider.stopped {
            StoppedCost::WorkerKept => format!("{worker} and its {storage}"),
            StoppedCost::ServerDeleted => format!("{worker}, its {storage}"),
        },
    );
    format!(
        "{what} may still exist at {} ({}) and cost money until you delete them there.",
        provider.label, provider.site
    )
}

fn noun(stopped: StoppedCost) -> &'static str {
    match stopped {
        StoppedCost::WorkerKept => "Worker",
        StoppedCost::ServerDeleted => "Server",
    }
}

#[cfg(test)]
mod tests;
