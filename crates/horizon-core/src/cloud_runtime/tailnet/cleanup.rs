//! Exact captured-node cleanup after the owner controller has deleted provider resources.
use super::receipt::{EnrollmentReceipt, ManualCleanupReason};
use crate::cloud_runtime::{CreateState, Error, Event, Result, Stage, state};
use horizon_cloud::tailnet::{NodeApi, NodeDeletion};
use std::time::{Duration, Instant};

/// The existing lifecycle owner holds this Store lock before discarding a card.
/// No HTTP or credential-store operation occurs here.
pub(in crate::cloud_runtime) fn require_complete(store: &state::Store, state: &state::Deployment) -> Result<()> {
    if let Some(receipt) = state
        .tailnet_enrollment
        .receipts()
        .iter()
        .find(|entry| !entry.node_deleted)
    {
        return Err(Error::TailnetNodeCleanup {
            tailnet_id: receipt.tailnet_id.as_str().into(),
            device_id: receipt.device_id.as_str().into(),
            reason: "recorded node removal is unconfirmed; finish node cleanup before removing this cloud",
        });
    }
    let selected = state.tailnet_enrollment.is_empty()
        && prior_worker(state)
        && super::Selection::load(store.root())
            .map_err(super::mapped)?
            .tailnet
            .is_some();
    if missing_identity(state, selected) {
        return Err(Error::Invalid(ManualCleanupReason::DeviceIdentityUnavailable.message()));
    }
    Ok(())
}

/// Existing cloud-delete callers hold this Store's lifecycle lock.
/// # Errors
/// Retains IDs and uncertainty on missing/offline API access or persistence failure.
pub(in crate::cloud_runtime) fn retry(
    store: &state::Store,
    state: &mut state::Deployment,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let settings = store
        .root()
        .parent()
        .ok_or(Error::Invalid("Missing cloud settings root"))?;
    let api = NodeApi::new(super::store(settings));
    run(store, state, emit, |receipt, budget| {
        api.delete_recorded(receipt.tailnet_id.as_str(), receipt.device_id.as_str(), budget)
    })
}

fn run(
    store: &state::Store,
    state: &mut state::Deployment,
    emit: &dyn Fn(Event),
    mut remove: impl FnMut(&EnrollmentReceipt, Duration) -> NodeDeletion,
) -> Result<()> {
    let selection = super::Selection::load(store.root()).map_err(super::mapped)?;
    if state.tailnet_enrollment.is_empty() && selection.tailnet.is_none() {
        return Ok(());
    }
    require_owned_deletion(store, state)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    for receipt in state.tailnet_enrollment.receipts().to_vec() {
        if receipt.node_deleted || !state.tailnet_enrollment.receipts().contains(&receipt) {
            continue;
        }
        let mut confirmed = state.clone();
        confirmed
            .tailnet_enrollment
            .confirm_removed(&receipt)
            .map_err(|_| Error::TailnetNodeCleanup {
                tailnet_id: receipt.tailnet_id.as_str().into(),
                device_id: receipt.device_id.as_str().into(),
                reason: "local confirmation cannot fit the saved ledger; no delete request was sent",
            })?;
        emit(Event::Progress(crate::cloud_runtime::progress::Progress::activity(
            "Removing the recorded tailnet node",
        )));
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = if remaining.is_zero() {
            NodeDeletion::Retained(horizon_cloud::tailnet::RetentionReason::Transport)
        } else {
            remove(&receipt, remaining)
        };
        match result {
            NodeDeletion::ConfirmedDeleted => {
                store.save(&confirmed).map_err(|_| Error::TailnetNodeCleanup {
                    tailnet_id: receipt.tailnet_id.as_str().into(),
                    device_id: receipt.device_id.as_str().into(),
                    reason: "the confirmed response could not be saved; local cleanup remains uncertain",
                })?;
                *state = confirmed;
            }
            NodeDeletion::Retained(reason) => {
                return Err(Error::TailnetNodeCleanup {
                    tailnet_id: receipt.tailnet_id.as_str().into(),
                    device_id: receipt.device_id.as_str().into(),
                    reason: reason.message(),
                });
            }
        }
    }
    if missing_identity(state, selection.tailnet.is_some()) {
        return Err(Error::Invalid(ManualCleanupReason::DeviceIdentityUnavailable.message()));
    }
    Ok(())
}

fn missing_identity(state: &state::Deployment, selected: bool) -> bool {
    !state.tailnet_enrollment.attempts().is_empty()
        || (selected && state.tailnet_enrollment.is_empty() && prior_worker(state))
}

fn prior_worker(state: &state::Deployment) -> bool {
    !matches!(state.operation, CreateState::Prepared)
        || state.worker.is_some()
        || state.stop_requested
        || state.source_ready
        || state.ready_history == state::ReadyHistory::Observed
        || state.ready_after_seconds.is_some()
        || state.tailnet_device.is_some()
        || state
            .timeline
            .as_ref()
            .is_some_and(|timeline| timeline.reconnected || timeline.resume_requested.is_some())
}

fn require_owned_deletion(store: &state::Store, state: &state::Deployment) -> Result<()> {
    if store.root().file_name().and_then(|name| name.to_str()) != Some(state.cloud_id.as_str())
        || state.stage != Stage::Deleted
        || state.worker.is_some()
        || state.image_replacement.is_some()
        || !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. })
    {
        return Err(Error::Invalid(
            "Tailnet cleanup requires its owning cloud and confirmed provider deletion",
        ));
    }
    if state
        .tailnet_enrollment
        .receipts()
        .iter()
        .any(|receipt| receipt.allocation_request_id.as_str() != state.cloud_id)
    {
        return Err(Error::Invalid(
            "Tailnet receipt allocation differs from its owning cloud",
        ));
    }
    if let Some(receipt) = state
        .tailnet_enrollment
        .receipts()
        .iter()
        .find(|entry| entry.provider.as_str() != horizon_cloud::provider::Description::of(&state.profile).id)
    {
        return Err(Error::TailnetNodeCleanup {
            tailnet_id: receipt.tailnet_id.as_str().into(),
            device_id: receipt.device_id.as_str().into(),
            reason: "the receipt names another original provider; its resource deletion cannot be confirmed here",
        });
    }
    if let Some(receipt) = state
        .tailnet_enrollment
        .receipts()
        .iter()
        .find(|entry| !entry.node_deleted)
        && !super::super::providers::deletion_confirmed(store, state).unwrap_or(false)
    {
        return Err(Error::TailnetNodeCleanup {
            tailnet_id: receipt.tailnet_id.as_str().into(),
            device_id: receipt.device_id.as_str().into(),
            reason: "the original provider resource journals do not confirm deletion; no API request was sent",
        });
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
