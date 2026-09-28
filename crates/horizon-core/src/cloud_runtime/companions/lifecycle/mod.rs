//! Explicit controller operations. Loading, selecting and polling never execute work.
mod execution;
mod receipt;
#[cfg(all(test, unix))]
mod tests;

use super::{Context, Error, Owner, Result, intent, journal};
use crate::cloud_runtime::{Cancellation, Event, settings::Settings, state::Store};
use horizon_cloud_protocol::OperationId;
use intent::{Action, Binding, Intent, State};
use std::path::Path;

/// Trusted owning-controller inputs, refreshed from the current workspace before each call.
/// Never construct this from an agent's claimed workspace, inventory or credentials.
pub struct Request<'a> {
    pub root: &'a Path,
    pub owner: &'a Owner,
    pub context: &'a Context,
    pub alias: &'a str,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Submitted,
    Running,
    Inspecting,
    Settling,
    VerifyingAccess,
    ConfirmationRequired,
    ReconcileRequired,
    /// Provider outcome is definite, but completion needs a new explicit request.
    RetryRequired,
    Ready,
    Stopped,
    Refused,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub intent: Intent,
    pub phase: Phase,
}

impl Request<'_> {
    fn load(&self) -> Result<(journal::Store, journal::State)> {
        let store = journal::Store::open(self.root, self.owner)?;
        let state = store.load()?;
        if state.owner != *self.owner
            || self.context.source.scope != self.owner.scope
            || self.context.source.cloud_id != self.owner.cloud_id
        {
            return Err(Error::Invalid("Companion controller ownership changed"));
        }
        Ok((store, state))
    }

    fn authorize(&self, state: &journal::State) -> Result<Binding> {
        let binding = state
            .intents
            .binding(self.alias)
            .ok_or(Error::Invalid("Companion is not bound"))?;
        let declaration = self
            .context
            .declarations
            .get(self.alias)
            .ok_or(Error::Invalid("Companion declaration is missing"))?;
        binding.validate(self.owner, self.alias, declaration)?;
        if binding.origin() == intent::Origin::Existing {
            self.require_selected(state, binding, declaration)?;
        }
        Ok(binding.clone())
    }

    /// The owner's checkbox grant for exactly this binding's target, resolved against
    /// the current inventory.
    fn require_selected(
        &self,
        state: &journal::State,
        binding: &Binding,
        declaration: &super::Declaration,
    ) -> Result<()> {
        let grant = state
            .grants
            .get(self.alias)
            .filter(|grant| {
                grant.selected
                    && grant.target.cloud_id == binding.target().cloud_id
                    && grant.target.scope == binding.target().scope
                    && grant.target.declaration.matches(&binding.target().declaration)
            })
            .ok_or(Error::Invalid("Companion is no longer selected"))?;
        grant
            .selection
            .resolve(&self.context.source, self.alias, declaration, &self.context.inventory)
            .map_err(|_| Error::Invalid("Companion target is missing or changed"))?;
        Ok(())
    }

    fn target_store(&self, binding: &Binding) -> Result<Store> {
        Store::lock(&crate::cloud_runtime::state::cloud_directory(
            self.root,
            &binding.target().cloud_id,
        )?)
    }
}

/// Save an explicit machine-local selection without starting or preparing a worker.
/// # Errors
/// Refuses changed declarations, owners and conflicting bindings.
pub fn bind(request: &Request<'_>, binding: Binding) -> Result<()> {
    let (store, mut state) = request.load()?;
    let declaration = request
        .context
        .declarations
        .get(request.alias)
        .ok_or(Error::Invalid("Companion declaration is missing"))?;
    binding.validate(request.owner, request.alias, declaration)?;
    if binding.origin() == intent::Origin::Reserved {
        let target = request.target_store(&binding)?;
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
    }
    state.intents.bind(request.owner, request.alias, binding)?;
    request.authorize(&state)?;
    store.save(&state)
}

/// Bind a companion the owner checked on the source cloud's card to its existing
/// cloud, using the checkout that cloud's deployment records. Already-bound aliases
/// are left unchanged, so a request never rebinds a target the owner selected.
/// # Errors
/// Refuses an unchecked companion, and one without a deployment record: creating a
/// cloud needs the owner's confirmation on the card.
pub fn bind_selected(request: &Request<'_>) -> Result<()> {
    let (_, state) = request.load()?;
    if state.intents.binding(request.alias).is_some() {
        return Ok(());
    }
    let target = state
        .grants
        .get(request.alias)
        .filter(|grant| grant.selected)
        .map(|grant| grant.target.clone())
        .ok_or(Error::Invalid("Check this companion on the source cloud's card first"))?;
    let deployed = Store::lock(&crate::cloud_runtime::state::cloud_directory(
        request.root,
        &target.cloud_id,
    )?)?
    .load()?
    .ok_or(Error::Invalid(
        "The companion cloud has not been created; start it from its card",
    ))?;
    bind(
        request,
        Binding::new(
            request.owner,
            request.alias,
            target,
            deployed.repository,
            intent::Origin::Existing,
        )?,
    )
}

/// Durably submit an explicit request, without provider or SSH calls. Run `execute`
/// on a background worker after returning this ID to the caller; poll `status`.
/// # Errors
/// A different source's pending operation returns Busy, never starts a second action.
/// That caller must explicitly retry after the operation is reconciled.
pub fn submit(request: &Request<'_>, action: Action, id: OperationId) -> Result<Operation> {
    let (store, mut state) = request.load()?;
    let binding = request.authorize(&state)?;
    let mut candidate = state.intents.clone();
    if let Ok(intent) = candidate.submit(request.alias, action, id)
        && state.intents.operation(intent.operation_id).is_some()
    {
        state.intents = candidate;
        store.save(&state)?;
        return operation(request, &binding, intent);
    }
    let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
    let _execution = receipt::execution_lock(&root)?;
    let target = request.target_store(&binding)?;
    let settled = if let Some(prior) = receipt::load(target.root())? {
        if prior.owner.cloud_id == request.owner.cloud_id {
            settle_access(&store, &mut state, &prior)?;
            state.intents.settled(prior.id)
        } else {
            let other = journal::Store::open(request.root, &prior.owner)?;
            let mut other_state = other.load()?;
            settle_access(&other, &mut other_state, &prior)?;
            other_state.intents.settled(prior.id)
        }
    } else {
        true
    };
    let previous = state.intents.clone();
    let intent = state.intents.submit(request.alias, action, id)?;
    if previous.operation(intent.operation_id).is_some() {
        store.save(&state)?;
        return operation(request, &binding, intent);
    }
    if !settled {
        return Err(Error::Busy);
    }
    // A crash between these writes leaves a submitted operation without a target
    // claim. Cancel that unstarted submission explicitly, never infer allocation.
    store.save(&state)?;
    receipt::save(&target, request.owner, id, Phase::Submitted)?;
    Ok(Operation {
        intent,
        phase: Phase::Submitted,
    })
}

fn settle_access(store: &journal::Store, state: &mut journal::State, prior: &receipt::Receipt) -> Result<()> {
    if matches!(prior.phase, Phase::VerifyingAccess | Phase::Settling)
        && state
            .intents
            .operation(prior.id)
            .is_some_and(|intent| matches!(intent.state, State::Executing | State::Uncertain))
    {
        // execution.lock proves the previous executor is gone. The durable phase
        // proves provider work completed, so a new explicit Stop can proceed.
        state.intents.transition(prior.id, State::RetryRequired)?;
        store.save(state)?;
    }
    Ok(())
}

/// Record the owner's confirmation, given on the source cloud's card, that a submitted
/// Ensure Ready may create its reserved companion cloud. The card first creates that
/// cloud and durably prepares its deployment record with the reserved ID and the bound
/// checkout; `execute` then allocates its first worker through the ordinary deployment.
/// Confirming the same unstarted operation again, as the card's Retry does after a
/// failed start, is idempotent: it records nothing new and never runs anything twice.
/// # Errors
/// Refuses anything but an unstarted Ensure Ready for a reserved binding whose target
/// record is prepared, workerless and matches the binding.
pub fn confirm_creation(request: &Request<'_>, id: OperationId) -> Result<()> {
    // Held through the write, in execution's lock order (source, execution, target),
    // so an uncheck cannot land between the checks and the recorded confirmation.
    let (_source, state) = request.load()?;
    let binding = request.authorize(&state)?;
    if binding.origin() != intent::Origin::Reserved {
        return Err(Error::Invalid("Only a reserved companion is created on confirmation"));
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
            "The companion cloud does not match its reserved binding",
        ));
    }
    receipt::confirm(&target, request.owner, id)
}

/// Read status only. No reconciliation, resume, deployment or grant refresh occurs.
/// # Errors
/// Refuses unknown IDs, changed authorization and corrupt durable state.
pub fn status(request: &Request<'_>, id: OperationId) -> Result<Operation> {
    let (_, state) = request.load()?;
    let binding = request.authorize(&state)?;
    let intent = state
        .intents
        .operation(id)
        .filter(|op| op.target_cloud_id == binding.target().cloud_id)
        .ok_or(Error::Invalid("Unknown companion operation"))?
        .clone();
    operation(request, &binding, intent)
}

/// Cancel a submission that has never begun provider execution, including a declined
/// creation confirmation or an interrupted target-claim write. Never clears uncertainty.
/// # Errors
/// Refuses an executing, uncertain or terminal operation.
pub fn cancel_submission(request: &Request<'_>, id: OperationId) -> Result<()> {
    let (store, mut state) = request.load()?;
    let binding = request.authorize(&state)?;
    let intent = state
        .intents
        .operation(id)
        .filter(|intent| intent.state == State::Submitted && intent.target_cloud_id == binding.target().cloud_id)
        .ok_or(Error::Invalid("Only an unstarted submission can be cancelled"))?
        .clone();
    let target = request.target_store(&binding)?;
    state.intents.transition(intent.operation_id, State::Failed)?;
    store.save(&state)?;
    if receipt::load(target.root())?
        .is_some_and(|claim| claim.owner == *request.owner && claim.id == intent.operation_id)
    {
        receipt::save(&target, request.owner, intent.operation_id, Phase::Refused)?;
    }
    Ok(())
}

fn operation(request: &Request<'_>, binding: &Binding, intent: Intent) -> Result<Operation> {
    let phase = match intent.state {
        State::Succeeded if intent.action == Action::Stop => Phase::Stopped,
        State::Succeeded => Phase::Ready,
        State::Failed => Phase::Refused,
        State::RetryRequired => Phase::RetryRequired,
        _ => {
            let root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
            receipt::load(&root)?
                .filter(|record| record.owner == *request.owner && record.id == intent.operation_id)
                .map_or(Phase::ReconcileRequired, |record| record.phase)
        }
    };
    Ok(Operation { intent, phase })
}

/// Execute one submitted operation on a background thread. Re-entering an executing
/// or uncertain operation performs reconciliation only, never repeats its mutation.
/// Missing/new allocations require the later UI-confirmed creation adapter.
/// # Errors
/// Returns actionable provider errors while retaining uncertainty durably. Busy
/// means another controller owns a required lock; callers may poll without replay.
pub fn execute(
    request: &Request<'_>,
    id: OperationId,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Operation> {
    execute_with(
        request,
        id,
        &mut execution::Live {
            settings,
            cancel,
            emit,
            pending: false,
            inspecting: false,
            settled: false,
        },
    )
}

fn execute_with(request: &Request<'_>, id: OperationId, backend: &mut impl execution::Backend) -> Result<Operation> {
    let (source, mut journal) = request.load()?;
    let binding = request.authorize(&journal)?;
    let intent = journal
        .intents
        .operation(id)
        .filter(|op| op.target_cloud_id == binding.target().cloud_id)
        .ok_or(Error::Invalid("Unknown companion operation"))?
        .clone();
    if !intent.state.pending() {
        return operation(request, &binding, intent);
    }
    let target_root = crate::cloud_runtime::state::cloud_directory(request.root, &binding.target().cloud_id)?;
    let _execution = receipt::execution_lock(&target_root)?;
    let target = request.target_store(&binding)?;
    let claim = receipt::load(target.root())?.ok_or(Error::Invalid("Operation has no durable target claim"))?;
    if claim.owner != *request.owner || claim.id != intent.operation_id {
        return Err(Error::Invalid(
            "Operation no longer owns the target; reconcile it explicitly",
        ));
    }
    if matches!(claim.phase, Phase::VerifyingAccess | Phase::Settling)
        || (intent.state == State::Executing
            && matches!(
                claim.phase,
                Phase::Submitted | Phase::ConfirmationRequired | Phase::RetryRequired
            ))
    {
        journal.intents.transition(intent.operation_id, State::RetryRequired)?;
        source.save(&journal)?;
        return operation(
            request,
            &binding,
            journal
                .intents
                .operation(intent.operation_id)
                .ok_or(Error::Invalid("Companion operation disappeared"))?
                .clone(),
        );
    }
    let observed = target.load()?;
    let confirmed = confirmed(request, &journal, &binding, &intent, &claim)?;
    let decision = execution::plan(&target, &binding, &intent, observed.as_ref(), confirmed)?;
    if decision == intent::Decision::Provision {
        receipt::save(&target, request.owner, intent.operation_id, Phase::ConfirmationRequired)?;
        return Ok(Operation {
            intent,
            phase: Phase::ConfirmationRequired,
        });
    }
    let inspecting = claim.phase == Phase::Inspecting
        || (decision == intent::Decision::ReconcileOnly && intent.state == State::Submitted);
    if let Err(error) = backend.preflight(&target, decision) {
        if intent.state == State::Submitted || inspecting {
            let next = if intent.state == State::Submitted {
                State::Failed
            } else {
                State::RetryRequired
            };
            journal.intents.transition(intent.operation_id, next)?;
            source.save(&journal)?;
        }
        return Err(error);
    }
    if observed.is_some() {
        journal.intents.mark_existing(&intent.target_cloud_id);
    }
    if intent.state == State::Submitted {
        journal.intents.transition(intent.operation_id, State::Executing)?;
        source.save(&journal)?;
    }
    receipt::save(
        &target,
        request.owner,
        intent.operation_id,
        match decision {
            intent::Decision::Reuse | intent::Decision::VerifyAccess => Phase::VerifyingAccess,
            intent::Decision::AlreadyStopped | intent::Decision::Refuse(_) => Phase::Settling,
            _ if inspecting => Phase::Inspecting,
            _ => Phase::Running,
        },
    )?;
    drop(source);
    let first_run = intent.state == State::Submitted;
    let result = run(
        backend,
        &target,
        (request.owner, &intent),
        decision,
        (inspecting, first_run),
    )?;
    // Provider execution held operation.lock throughout. Release before the grant
    // transport takes the same lock; the target claim still fences other sources.
    drop(target);
    finish(
        request,
        &binding,
        intent.operation_id,
        result,
        (inspecting, first_run),
        backend,
    )
}

/// Whether the owner confirmed this operation's creation. A confirmation stands only
/// while the owner's selection does; the caller holds the source journal lock, so an
/// uncheck after the confirmation withdraws it before any paid allocation.
fn confirmed(
    request: &Request<'_>,
    journal: &journal::State,
    binding: &Binding,
    intent: &Intent,
    claim: &receipt::Receipt,
) -> Result<bool> {
    let confirmed = claim.confirmed == Some(intent.operation_id);
    if confirmed && binding.origin() == intent::Origin::Reserved {
        let declaration = request
            .context
            .declarations
            .get(request.alias)
            .ok_or(Error::Invalid("Companion declaration is missing"))?;
        request
            .require_selected(journal, binding, declaration)
            .map_err(|_| Error::Invalid("The new companion cloud is no longer selected; nothing was created"))?;
    }
    Ok(confirmed)
}

/// Runs the provider work under the held target and records what the target must
/// know before it is released: a ready worker's access check, or a definite
/// failure, so losing the source journal to contention later cannot discard it and
/// re-entering the operation reads it as retryable instead of reconciling it forever.
fn run(
    backend: &mut impl execution::Backend,
    target: &Store,
    (owner, intent): (&Owner, &Intent),
    decision: intent::Decision,
    (inspecting, first_run): (bool, bool),
) -> Result<Result<Phase>> {
    backend.inspecting(inspecting);
    let result = backend.run(target, decision, intent.action).map(|phase| {
        if inspecting && phase == Phase::ReconcileRequired {
            Phase::RetryRequired
        } else if phase == Phase::RetryRequired && !inspecting {
            Phase::ReconcileRequired
        } else {
            phase
        }
    });
    if matches!(result, Ok(Phase::Ready)) {
        receipt::save(target, owner, intent.operation_id, Phase::VerifyingAccess)?;
    }
    // An inspection may reconnect, which can itself leave a mutation pending.
    if result.is_err() && (inspecting || first_run || backend.uncertainty_settled()) && !backend.mutation_uncertain() {
        receipt::save(target, owner, intent.operation_id, Phase::RetryRequired)?;
    }
    Ok(result)
}

fn finish(
    request: &Request<'_>,
    binding: &Binding,
    id: OperationId,
    result: Result<Phase>,
    (inspecting, first_run): (bool, bool),
    backend: &mut impl execution::Backend,
) -> Result<Operation> {
    let access_only = matches!(result, Ok(Phase::Ready));
    let result = result.and_then(|phase| {
        if phase == Phase::Ready {
            backend.verify_access(request).map(|phase| {
                if phase == Phase::Ready {
                    phase
                } else {
                    Phase::RetryRequired
                }
            })
        } else {
            Ok(phase)
        }
    });
    // An operation's first run that failed with no provider mutation left pending,
    // as in a local check or a rolled-back refusal, is definite: a fresh explicit
    // request may retry it. Reconciling an uncertain run never clears its fence.
    let definite = (inspecting || first_run || backend.uncertainty_settled()) && !backend.mutation_uncertain();
    let (source, mut journal) = request.load()?;
    let phase = result.as_ref().copied().unwrap_or(if access_only || definite {
        Phase::RetryRequired
    } else {
        Phase::ReconcileRequired
    });
    let state = match phase {
        Phase::Ready | Phase::Stopped => State::Succeeded,
        Phase::Refused => State::Failed,
        Phase::RetryRequired => State::RetryRequired,
        _ => State::Uncertain,
    };
    journal.intents.transition(id, state)?;
    source.save(&journal)?;
    // The source result is authoritative if another controller acquired this lock
    // or the process dies before the presentation phase is updated.
    if let Ok(target) = request.target_store(binding) {
        receipt::save(&target, request.owner, id, phase)?;
    }
    result?;
    operation(
        request,
        binding,
        journal
            .intents
            .operation(id)
            .ok_or(Error::Invalid("Companion operation disappeared"))?
            .clone(),
    )
}
