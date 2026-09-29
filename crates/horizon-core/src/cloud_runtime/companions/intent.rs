//! Durable companion bindings and lifecycle intents, separate from execution.
mod decision;
pub use decision::{Decision, Observation, Refusal, decide, refuses_deployment};

use super::{Declaration, Error, Owner, Result, Selection, Target};
use horizon_cloud_protocol::OperationId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// Machine-local authorization, including a target ID minted before a cloud exists.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    selection: Selection,
    target: Target,
    checkout: PathBuf,
    origin: Origin,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// An explicitly reserved ID, never an ID recovered from a vanished cloud.
    Reserved,
    Existing,
}

impl Binding {
    /// # Errors
    /// Rejects cross-workspace, self, same-worker and malformed bindings. The owning
    /// controller must verify the checkout's repository/profile before executing a plan.
    pub fn new(owner: &Owner, alias: &str, target: Target, checkout: PathBuf, origin: Origin) -> Result<Self> {
        let source = Target {
            scope: owner.scope.clone(),
            cloud_id: owner.cloud_id.clone(),
            declaration: target.declaration.clone(),
        };
        let selection = Selection::new(&source, alias, &target)
            .map_err(|_| Error::Invalid("Invalid companion lifecycle binding"))?;
        if !checkout.is_absolute() {
            return Err(Error::Invalid("Companion checkout must be an absolute local path"));
        }
        Ok(Self {
            selection,
            target,
            checkout,
            origin,
        })
    }

    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    #[must_use]
    pub fn checkout(&self) -> &std::path::Path {
        &self.checkout
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// Once a deployment has been durably prepared, absence must never mean creation.
    pub fn mark_existing(&mut self) {
        self.origin = Origin::Existing;
    }

    /// # Errors
    /// A declaration edit, owner move or sibling placement cannot reuse authorization.
    pub fn validate(&self, owner: &Owner, alias: &str, declaration: &Declaration) -> Result<()> {
        let expected = Self::new(owner, alias, self.target.clone(), self.checkout.clone(), self.origin)?;
        if expected.selection != self.selection || !declaration.matches(&self.target.declaration) {
            return Err(Error::Invalid("Companion binding changed; select the target again"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    EnsureReady,
    Stop,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Durable submission; provider execution has not begun.
    Submitted,
    /// Persist before provider I/O. After restart, reconcile instead of replaying.
    Executing,
    Uncertain,
    Succeeded,
    /// No provider mutation remains uncertain; completion needs a new explicit request.
    RetryRequired,
    /// Use only when failure is definite; an unknown outcome stays Uncertain.
    Failed,
}

impl State {
    #[must_use]
    pub fn pending(self) -> bool {
        matches!(self, Self::Submitted | Self::Executing | Self::Uncertain)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub operation_id: OperationId,
    pub action: Action,
    pub target_cloud_id: String,
    pub state: State,
}

/// Stored inside companions.json under companions.lock, never in committed YAML.
/// Methods only change this value: the caller must save it durably before acting.
/// It deliberately exposes no automatic execution on load, select or refresh.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    bindings: BTreeMap<String, Binding>,
    intents: BTreeMap<String, Intent>,
    /// Never evict request IDs: a delayed retry must not undo a newer Stop.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    history: BTreeMap<OperationId, Intent>,
    /// A deduplicated submit's response may be lost too. Retain its request ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    retries: BTreeMap<OperationId, OperationId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired: Vec<Retired>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Retired {
    owner: Owner,
    journal: Journal,
}

impl Journal {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active_empty() && self.retired.is_empty()
    }

    fn active_empty(&self) -> bool {
        self.bindings.is_empty() && self.intents.is_empty() && self.history.is_empty() && self.retries.is_empty()
    }

    fn pending(&self) -> bool {
        self.intents.values().any(|intent| intent.state.pending())
    }

    /// Whether an operation on this cloud has settled while none is still pending on it.
    #[must_use]
    pub fn settled_on(&self, target_cloud_id: &str) -> bool {
        let on_target = |intent: &&Intent| intent.target_cloud_id == target_cloud_id;
        !self
            .intents
            .values()
            .filter(on_target)
            .any(|intent| intent.state.pending())
            && self
                .intents
                .values()
                .chain(self.history.values())
                .any(|intent| on_target(&intent))
    }

    /// Retire authorization after grant cleanup, retaining old request IDs forever.
    /// An uncertain operation must be reconciled under its original owner first.
    pub(super) fn retire(&mut self, owner: &Owner) -> Result<bool> {
        self.validate(owner)?;
        if self.pending() {
            return Ok(false);
        }
        if !self.active_empty() {
            if self.retired.len() >= 64 {
                return Err(Error::Invalid(
                    "Companion ownership history is full; retain the journal for recovery",
                ));
            }
            let journal = Self {
                bindings: std::mem::take(&mut self.bindings),
                intents: std::mem::take(&mut self.intents),
                history: std::mem::take(&mut self.history),
                retries: std::mem::take(&mut self.retries),
                retired: Vec::new(),
            };
            self.retired.push(Retired {
                owner: owner.clone(),
                journal,
            });
        }
        Ok(true)
    }

    #[must_use]
    pub fn binding(&self, alias: &str) -> Option<&Binding> {
        self.bindings.get(alias)
    }

    #[must_use]
    pub fn operation(&self, id: OperationId) -> Option<&Intent> {
        let id = self.retries.get(&id).copied().unwrap_or(id);
        self.intents
            .values()
            .find(|intent| intent.operation_id == id)
            .or_else(|| self.history.get(&id))
    }

    /// Terminal IDs remain available to target-claim recovery after owner migration.
    pub(super) fn settled(&self, id: OperationId) -> bool {
        self.operation(id).is_some_and(|intent| !intent.state.pending())
            || self.retired.iter().any(|retired| retired.journal.settled(id))
    }

    /// # Errors
    /// Refuses rebinding, malformed ownership and excessive aliases. Selection is passive.
    pub fn bind(&mut self, owner: &Owner, alias: &str, binding: Binding) -> Result<()> {
        binding.validate(owner, alias, &binding.target.declaration)?;
        if let Some(existing) = self.bindings.get(alias) {
            return if existing == &binding {
                Ok(())
            } else {
                Err(Error::Invalid("Companion alias is already bound"))
            };
        }
        if self.bindings.len() >= 64 {
            return Err(Error::Invalid("At most 64 companion bindings are supported"));
        }
        if self.bindings.values().any(|other| {
            other.target.cloud_id == binding.target.cloud_id
                && (other.target != binding.target
                    || other.checkout != binding.checkout
                    || other.origin != binding.origin)
        }) {
            return Err(Error::Invalid("Companion target has conflicting bindings"));
        }
        self.bindings.insert(alias.into(), binding);
        Ok(())
    }

    /// Records only an explicit request. Repeated requests share an active target's ID;
    /// an opposite action waits for reconciliation instead of overwriting uncertain work.
    /// Dedup here is per source journal; execution must also hold the target's operation lock.
    /// # Errors
    /// Rejects absent bindings, conflicting actions and reused IDs for another request.
    pub fn submit(&mut self, alias: &str, action: Action, operation_id: OperationId) -> Result<Intent> {
        if self
            .retired
            .iter()
            .any(|retired| retired.journal.operation(operation_id).is_some())
        {
            return Err(Error::Invalid("Operation ID belongs to retired companion ownership"));
        }
        let binding = self
            .bindings
            .get(alias)
            .ok_or(Error::Invalid("Companion is not bound"))?;
        let target_cloud_id = &binding.target.cloud_id;
        if let Some(existing) = self.operation(operation_id) {
            return if existing.action == action && &existing.target_cloud_id == target_cloud_id {
                Ok(existing.clone())
            } else {
                Err(Error::Invalid("Operation ID belongs to another request"))
            };
        }
        if let Some(existing) = self
            .intents
            .values()
            .find(|intent| &intent.target_cloud_id == target_cloud_id && intent.state.pending())
        {
            if existing.action != action {
                return Err(Error::Busy);
            }
            if self.retries.len() >= 256 {
                return Err(Error::Invalid(
                    "Companion retry history is full; poll the existing operation",
                ));
            }
            let existing = existing.clone();
            self.retries.insert(operation_id, existing.operation_id);
            return Ok(existing);
        }
        let intent = Intent {
            operation_id,
            action,
            target_cloud_id: target_cloud_id.clone(),
            state: State::Submitted,
        };
        if let Some(previous) = self.intents.get(alias) {
            if self.history.len() >= 256 {
                return Err(Error::Invalid(
                    "Companion operation history is full; retain the journal for recovery",
                ));
            }
            self.history.insert(previous.operation_id, previous.clone());
        }
        self.intents.insert(alias.into(), intent.clone());
        Ok(intent)
    }

    /// # Errors
    /// Rejects missing IDs and transitions that would replay execution or reopen a result.
    /// The service must save Executing before I/O and reserve Failed for definite failures.
    pub fn transition(&mut self, operation_id: OperationId, next: State) -> Result<()> {
        let intent = self
            .intents
            .values_mut()
            .find(|intent| intent.operation_id == operation_id)
            .ok_or(Error::Invalid("Unknown companion operation"))?;
        let allowed = intent.state == next
            || matches!(
                (intent.state, next),
                (State::Submitted, State::Executing | State::Failed)
                    | (
                        State::Executing,
                        State::Uncertain | State::Succeeded | State::Failed | State::RetryRequired
                    )
                    | (
                        State::Uncertain,
                        State::Succeeded | State::Failed | State::RetryRequired
                    )
            );
        if !allowed {
            return Err(Error::Invalid("Invalid companion operation transition"));
        }
        intent.state = next;
        Ok(())
    }

    /// Mark all aliases to this target after its deployment record is saved.
    pub fn mark_existing(&mut self, target_cloud_id: &str) {
        for binding in self
            .bindings
            .values_mut()
            .filter(|binding| binding.target.cloud_id == target_cloud_id)
        {
            binding.mark_existing();
        }
    }

    /// # Errors
    /// Rejects corrupt or transplanted state before any caller can use it.
    pub fn validate(&self, owner: &Owner) -> Result<()> {
        if self.bindings.len() > 64
            || self.intents.len() > 64
            || self.history.len() > 256
            || self.retries.len() > 256
            || self.retired.len() > 64
        {
            return Err(Error::Invalid("Companion lifecycle journal is too large"));
        }
        let mut targets = BTreeMap::new();
        for (alias, binding) in &self.bindings {
            binding.validate(owner, alias, &binding.target.declaration)?;
            if let Some(previous) = targets.insert(&binding.target.cloud_id, binding)
                && (previous.target != binding.target
                    || previous.checkout != binding.checkout
                    || previous.origin != binding.origin)
            {
                return Err(Error::Invalid("Companion target has conflicting bindings"));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut pending = std::collections::BTreeSet::new();
        for (alias, intent) in &self.intents {
            if self
                .bindings
                .get(alias)
                .is_none_or(|binding| binding.target.cloud_id != intent.target_cloud_id)
                || !ids.insert(intent.operation_id)
                || (intent.state.pending() && !pending.insert(&intent.target_cloud_id))
            {
                return Err(Error::Invalid("Invalid persisted companion operation"));
            }
        }
        for (id, intent) in &self.history {
            if *id != intent.operation_id
                || !ids.insert(*id)
                || intent.state.pending()
                || !self
                    .bindings
                    .values()
                    .any(|binding| binding.target.cloud_id == intent.target_cloud_id)
            {
                return Err(Error::Invalid("Invalid companion operation history"));
            }
        }
        for (retry, canonical) in &self.retries {
            if ids.contains(retry) || !ids.contains(canonical) {
                return Err(Error::Invalid("Invalid companion retry history"));
            }
        }
        ids.extend(self.retries.keys().copied());
        for retired in &self.retired {
            if !retired.journal.retired.is_empty()
                || retired.journal.active_empty()
                || retired.journal.pending()
                || retired.owner.cloud_id != owner.cloud_id
            {
                return Err(Error::Invalid("Invalid retired companion ownership"));
            }
            retired.journal.validate(&retired.owner)?;
            for id in retired
                .journal
                .intents
                .values()
                .map(|intent| intent.operation_id)
                .chain(retired.journal.history.keys().copied())
                .chain(retired.journal.retries.keys().copied())
            {
                if !ids.insert(id) {
                    return Err(Error::Invalid("Operation ID crosses companion owners"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
