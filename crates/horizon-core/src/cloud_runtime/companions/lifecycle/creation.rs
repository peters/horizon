//! Creating a companion cloud the owner confirmed on the source cloud's card: the
//! reserved identity with the agent's operation, then the owner's confirmation.
use super::{Action, Binding, Error, Operation, OperationId, Phase, Request, Result, State, intent, receipt, submit};
use crate::cloud_runtime::state::Store;

/// Reserve a fresh cloud identity for a missing companion and record the agent's Ensure
/// Ready on it in one journal write, so neither the binding nor the operation can exist
/// without the other. Retrying with the same binding and operation ID returns the
/// operation already recorded, and restores its target claim if only that write was lost.
/// # Errors
/// Refuses an alias bound to another cloud or checkout, a target that already exists and
/// an operation ID that belongs to another request, each before anything is written.
pub fn reserve(request: &Request<'_>, binding: Binding, id: OperationId) -> Result<Operation> {
    {
        let (store, mut state) = request.load()?;
        match state.intents.binding(request.alias) {
            // A retry of this reservation; submitting again records nothing twice.
            Some(bound) if *bound == binding => {
                let unstarted = state.intents.operation(id).is_some_and(|intent| {
                    intent.state == State::Submitted
                        && intent.action == Action::EnsureReady
                        && intent.target_cloud_id == binding.target().cloud_id
                });
                if unstarted {
                    let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
                    let _execution = receipt::execution_lock(&root)?;
                    let target = request.target_store(&binding)?;
                    // The journal write landed but the claim did not: finish it, so the
                    // owner can still confirm this reservation.
                    if receipt::load(target.root())?.is_none() {
                        receipt::save(&target, request.owner, id, Phase::Submitted)?;
                    }
                }
            }
            Some(bound) if bound.target() == binding.target() => {
                return Err(Error::Invalid(
                    "This companion is reserved for another checkout; choose that one",
                ));
            }
            Some(_) => {
                return Err(Error::Invalid(
                    "This companion is already bound to another cloud; nothing was reserved",
                ));
            }
            None => {
                let declaration = request
                    .context
                    .declarations
                    .get(request.alias)
                    .ok_or(Error::Invalid("Companion declaration is missing"))?;
                binding.validate(request.owner, request.alias, declaration)?;
                if binding.origin() != intent::Origin::Reserved {
                    return Err(Error::Invalid("Only a fresh cloud identity can be reserved"));
                }
                // Submit's lock order: source journal, execution, then target.
                let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
                let _execution = receipt::execution_lock(&root)?;
                let target = request.target_store(&binding)?;
                require_fresh(request, &binding, &target)?;
                state.intents.bind(request.owner, request.alias, binding)?;
                request.authorize(&state)?;
                let intent = state.intents.submit(request.alias, Action::EnsureReady, id)?;
                store.save(&state)?;
                // As in submit, a crash before the claim leaves an unstarted submission
                // that a decline cancels explicitly.
                receipt::save(&target, request.owner, id, Phase::Submitted)?;
                return Ok(Operation {
                    intent,
                    phase: Phase::Submitted,
                });
            }
        }
    }
    submit(request, Action::EnsureReady, id)
}

/// Refuses a reserved identity that already names a cloud, its record, claim or
/// provider resources: an existing target requires the owner's selection instead.
pub(super) fn require_fresh(request: &Request<'_>, binding: &Binding, target: &Store) -> Result<()> {
    if request
        .context
        .inventory
        .iter()
        .any(|target| target.cloud_id == binding.target().cloud_id)
        || target.load()?.is_some()
        || receipt::load(target.root())?.is_some()
        || ["hetzner.json", "workspace-volume.json", "workspace-volume.required"]
            .iter()
            .map(|file| target.root().join(file).try_exists())
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .any(|exists| exists)
    {
        return Err(Error::Invalid(
            "Reserve a fresh cloud identity; an existing target requires selection",
        ));
    }
    Ok(())
}

/// Record the owner's confirmation, given on the source cloud's card, that a submitted
/// Ensure Ready may create its reserved companion cloud. The card first creates that
/// cloud and durably prepares its deployment record with the reserved ID and the bound
/// checkout; `execute` then allocates its first worker through the ordinary deployment.
/// Confirming the same unstarted operation again, as the card's Retry does after a
/// failed start, is idempotent: it records nothing new and never runs anything twice.
/// An operation that already started under its confirmation is accepted as it is, so
/// a recovered card's Retry continues to its reconciliation.
/// # Errors
/// Refuses anything but an unstarted Ensure Ready whose target record, reserved or
/// existing, is prepared with no worker ever requested and matches the binding's
/// checkout, while the owner's selection covers it.
pub fn confirm_creation(request: &Request<'_>, id: OperationId) -> Result<()> {
    // Held through the write, in execution's lock order (source, execution, target),
    // so an uncheck cannot land between the checks and the recorded confirmation.
    let (_source, state) = request.load()?;
    let binding = request.authorize(&state)?;
    // An operation that already started under this confirmation, then lost its card to
    // a crash, needs none again: execution reconciles it, and an uncheck never blocks that.
    let started = state.intents.operation(id).is_some_and(|intent| {
        matches!(intent.state, State::Executing | State::Uncertain)
            && intent.action == Action::EnsureReady
            && intent.target_cloud_id == binding.target().cloud_id
    });
    if started {
        let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
        let _execution = receipt::execution_lock(&root)?;
        let target = request.target_store(&binding)?;
        let confirmed = receipt::load(target.root())?
            .is_some_and(|claim| claim.owner == *request.owner && claim.id == id && claim.confirmed == Some(id));
        return if confirmed {
            Ok(())
        } else {
            Err(Error::Invalid("The operation no longer owns the companion target"))
        };
    }
    // Access is verified once the worker is ready, so the owner's selection must
    // already cover the new cloud; otherwise the paid worker could never become Ready.
    let declaration = request
        .context
        .declarations
        .get(request.alias)
        .ok_or(Error::Invalid("Companion declaration is missing"))?;
    request
        .require_selected(&state, &binding, declaration)
        .map_err(|_| Error::Invalid("Select the new companion cloud before confirming it"))?;
    let grant = state
        .grants
        .get(request.alias)
        .map(|grant| grant.id.clone())
        .ok_or(Error::Invalid("Select the new companion cloud before confirming it"))?;
    state
        .intents
        .operation(id)
        .filter(|intent| {
            intent.state == State::Submitted
                && intent.action == Action::EnsureReady
                && intent.target_cloud_id == binding.target().cloud_id
        })
        .ok_or(Error::Invalid("Only an unstarted Ensure Ready can be confirmed"))?;
    let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
    let _execution = receipt::execution_lock(&root)?;
    let target = request.target_store(&binding)?;
    if !receipt::load(target.root())?.is_some_and(|claim| {
        claim.owner == *request.owner
            && claim.id == id
            && matches!(claim.phase, Phase::Submitted | Phase::ConfirmationRequired)
    }) {
        return Err(Error::Invalid("The operation no longer owns the companion target"));
    }
    // A reserved cloud, or one whose confirmed first start failed before allocating:
    // either way its record is still prepared and no worker was ever requested.
    let prepared = target
        .load()?
        .ok_or(Error::Invalid("Create the companion cloud before confirming it"))?;
    if prepared.cloud_id != binding.target().cloud_id
        || prepared.repository != binding.checkout()
        || prepared.operation != crate::cloud_runtime::CreateState::Prepared
        || prepared.worker.is_some()
        || prepared.stop_requested
    {
        return Err(Error::Invalid(
            "The companion cloud does not match its binding or already has a worker",
        ));
    }
    receipt::confirm(&target, request.owner, id, &grant)
}

/// The checkout a companion is bound to, when it is bound. Reads only; lets the card
/// offer a reservation again after Horizon closed before adding its cloud.
/// # Errors
/// Refuses changed ownership and corrupt durable state.
pub fn bound_checkout(request: &Request<'_>) -> Result<Option<std::path::PathBuf>> {
    let (_, state) = request.load()?;
    Ok(state
        .intents
        .binding(request.alias)
        .map(|binding| binding.checkout().to_owned()))
}

/// Whether the owner's checkbox currently selects a cloud for this companion. Reads
/// only: a selected cloud that cannot be bound is an error to report, never a missing
/// companion to create.
/// # Errors
/// Refuses changed ownership and corrupt durable state.
pub fn selects_a_cloud(request: &Request<'_>) -> Result<bool> {
    let (_, state) = request.load()?;
    Ok(state.grants.get(request.alias).is_some_and(|grant| grant.selected))
}

/// Whether a reserved companion's card can be added again from its binding: its cloud
/// has no record, or only the prepared one a card saved before any worker was
/// requested, and no provider resources. Reads only.
/// # Errors
/// Refuses changed ownership and corrupt durable state.
pub fn card_recoverable(request: &Request<'_>) -> Result<bool> {
    let (_, state) = request.load()?;
    let Some(binding) = state.intents.binding(request.alias) else {
        return Ok(false);
    };
    if binding.origin() != intent::Origin::Reserved {
        return Ok(false);
    }
    let target = request.target_store(binding)?;
    let provider_resources = ["hetzner.json", "workspace-volume.json", "workspace-volume.required"]
        .iter()
        .map(|file| target.root().join(file).try_exists())
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .any(|exists| exists);
    if provider_resources {
        return Ok(false);
    }
    Ok(target.load()?.is_none_or(|record| {
        record.cloud_id == binding.target().cloud_id
            && record.repository == binding.checkout()
            && record.operation == crate::cloud_runtime::CreateState::Prepared
            && record.worker.is_none()
            && !record.stop_requested
    }))
}
