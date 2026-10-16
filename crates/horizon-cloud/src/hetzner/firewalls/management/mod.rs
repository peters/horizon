//! Same-client management policy and mutation sequencing, not native approval.
//! The caller must physically close its original worker gate on every error.
mod snapshot;
use super::{Direction, Firewall, Protocol, Rule, Scope, valid_rules};
use crate::{
    Cancellation, CloudError,
    hetzner::{Action, Hetzner},
};
use snapshot::Snapshot;
use std::collections::BTreeMap;

/// Desired policy for the first firewall, supplied by the current approved owner.
/// The final Core adapter must cover its actual pinned SSH reconnect endpoint.
/// These rules do not prove an observation or issue any approval.
pub struct ManagementRules(Vec<Rule>);
impl ManagementRules {
    /// # Errors
    /// Requires inbound-only valid rules including an explicit TCP allowance.
    pub fn new(rules: Vec<Rule>) -> Result<Self, CloudError> {
        inbound(&rules)?;
        if !rules.iter().any(|r| r.protocol == Protocol::Tcp) {
            return Err(CloudError::Invalid("Missing SSH management policy"));
        }
        Ok(Self(rules))
    }
}

/// Every callback must complete an original owner's durable save before returning.
/// A failure retains uncertainty; none of these records can restore a controller.
pub trait Journal {
    /// # Errors
    /// Return Persistence if the exact update could not be durably saved.
    fn save(&mut self, update: Update<'_>) -> Result<(), CloudError>;
}
/// Intent precedes every mutation; returned IDs precede action polling.
pub enum Update<'a> {
    Intent {
        kind: Mutation,
        firewall: Option<u64>,
        rules: &'a [Rule],
        foreign: &'a BTreeMap<u64, Firewall>,
    },
    Reconciled {
        firewall: &'a Firewall,
    },
    Actions {
        firewall: u64,
        actions: &'a [Action],
    },
    Confirmed {
        firewall: Option<u64>,
        rules: &'a [Rule],
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    Create,
    Rules,
    Apply,
    Remove,
    Delete,
}

/// Retains the original authenticated client, original scope and all live siblings.
/// There is no deserializer, restore constructor or foreign-firewall mutation.
pub struct Manager {
    client: Hetzner,
    scope: Scope,
    snapshot: Snapshot,
    management: Vec<Rule>,
    services: BTreeMap<String, Vec<Rule>>,
    uncertain: bool,
    pending: Option<Vec<Rule>>,
    original_id: Option<u64>,
    unresolved_create: bool,
    retired: bool,
}
impl Hetzner {
    /// Capture a settled baseline before any provider mutation.
    /// # Errors
    /// Refuses missing attachment facts, transitions, selectors and exhausted slots.
    pub fn prepare_exposure_management(
        self,
        scope: Scope,
        first: ManagementRules,
        cancel: &Cancellation,
    ) -> Result<Manager, CloudError> {
        let snapshot = Snapshot::read(&self, &scope, None, cancel)?;
        if snapshot.foreign.len() == 5 {
            return Err(CloudError::Invalid("No firewall attachment slot"));
        }
        let management = if snapshot.foreign.is_empty() {
            first.0
        } else {
            Vec::new()
        };
        Ok(Manager {
            client: self,
            scope,
            snapshot,
            management,
            services: BTreeMap::new(),
            uncertain: false,
            pending: None,
            original_id: None,
            unresolved_create: false,
            retired: false,
        })
    }
}
impl Manager {
    /// Fresh original provider endpoint facts, not a connection factory.
    #[must_use]
    pub fn ssh_endpoint(&self) -> std::net::SocketAddr {
        self.snapshot.ssh
    }
    #[must_use]
    pub fn uncertain(&self) -> bool {
        self.uncertain
    }
    #[must_use]
    pub fn firewall_id(&self) -> Option<u64> {
        self.snapshot.owned.as_ref().map(|f| f.id)
    }

    /// Replace one original live service contribution, retaining every sibling.
    /// Provider rules are already approved by Core's shared source/port policy.
    /// # Errors
    /// On any failure, physically close the worker gate and retain uncertainty.
    pub fn replace_service(
        &mut self,
        service: &str,
        rules: Vec<Rule>,
        journal: &mut dyn Journal,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        self.ready(cancel)?;
        if !crate::valid_id(service) {
            return Err(CloudError::Invalid("Invalid service identity"));
        }
        inbound(&rules)?;
        if rules.is_empty() {
            return Err(CloudError::Invalid("Empty exposure rules"));
        }
        let mut next = self.services.clone();
        next.insert(service.into(), rules);
        self.change(next, journal, cancel, true)
    }

    /// Caller must physically retire this service's original gate before removal.
    /// # Errors
    /// Refuses unknown services or any changed original provider baseline.
    pub fn close_service(
        &mut self,
        service: &str,
        journal: &mut dyn Journal,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        self.ready(cancel)?;
        let mut next = self.services.clone();
        if next.remove(service).is_none() {
            return Err(CloudError::IdentityMismatch);
        }
        self.change(next, journal, cancel, true)
    }

    /// Remove one physically closed service while retaining this live epoch's firewall.
    /// The remaining rules include complete management and all active siblings.
    /// # Errors
    /// Refuses unknown services, retired ownership or any provider uncertainty.
    pub fn close_service_retaining_firewall(
        &mut self,
        service: &str,
        journal: &mut dyn Journal,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        self.ready(cancel)?;
        let mut next = self.services.clone();
        if next.remove(service).is_none() {
            return Err(CloudError::IdentityMismatch);
        }
        self.change(next, journal, cancel, false)
    }
    /// Verify original current provider rules, attachment and SSH endpoint facts.
    /// # Errors
    /// Refuses uncertainty, cancellation or any original baseline drift.
    pub fn verify_current(&mut self, cancel: &Cancellation) -> Result<(), CloudError> {
        self.ready(cancel)?;
        self.compare(cancel).inspect_err(|_| self.uncertain = true)
    }
    fn ready(&self, cancel: &Cancellation) -> Result<(), CloudError> {
        if self.uncertain || self.retired {
            return Err(CloudError::CreationUnresolved);
        }
        cancel.check()
    }
    fn change(
        &mut self,
        next: BTreeMap<String, Vec<Rule>>,
        journal: &mut dyn Journal,
        cancel: &Cancellation,
        retire_empty: bool,
    ) -> Result<(), CloudError> {
        let rules: Vec<_> = self.management.iter().chain(next.values().flatten()).cloned().collect();
        valid_rules(&rules)?;
        self.pending = Some(rules.clone());
        self.uncertain = true;
        self.compare(cancel)?;
        if next.is_empty() && retire_empty {
            self.retire(journal, cancel)?;
        } else if let Some(id) = self.firewall_id() {
            journal.save(Update::Intent {
                kind: Mutation::Rules,
                firewall: Some(id),
                rules: &rules,
                foreign: &self.snapshot.foreign,
            })?;
            let actions = self
                .client
                .set_exposure_firewall_rules(&self.scope, id, &rules, cancel)?;
            self.save_wait(id, &actions, journal, cancel)?;
            self.expected_rules(rules.clone());
            self.compare(cancel)?;
        } else {
            self.create(&rules, journal, cancel)?;
        }
        journal.save(Update::Confirmed {
            firewall: self.firewall_id(),
            rules: if next.is_empty() && retire_empty { &[] } else { &rules },
        })?;
        self.services = next;
        self.pending = None;
        self.uncertain = false;
        Ok(())
    }
    fn compare(&self, cancel: &Cancellation) -> Result<(), CloudError> {
        if Snapshot::read(&self.client, &self.scope, self.firewall_id(), cancel)? != self.snapshot {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
    fn expected_rules(&mut self, rules: Vec<Rule>) {
        if let Some(firewall) = &mut self.snapshot.owned {
            firewall.rules = rules;
        }
    }
    fn save_wait(
        &self,
        id: u64,
        actions: &[Action],
        journal: &mut dyn Journal,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        journal.save(Update::Actions { firewall: id, actions })?;
        self.client.wait_firewall_actions(actions, cancel)
    }
    fn create(&mut self, rules: &[Rule], journal: &mut dyn Journal, cancel: &Cancellation) -> Result<(), CloudError> {
        journal.save(Update::Intent {
            kind: Mutation::Create,
            firewall: None,
            rules,
            foreign: &self.snapshot.foreign,
        })?;
        let mut unresolved = false;
        let result = self
            .client
            .create_exposure_firewall_attempt(&self.scope, rules, cancel, &mut unresolved);
        // A saved intent alone cannot own a pre-existing matching resource.
        self.unresolved_create = unresolved;
        if unresolved {
            self.original_id = None;
        }
        let created = result?;
        let id = created.firewall.id;
        // Retain returned identity even if the durable action save fails.
        self.original_id = Some(id);
        self.unresolved_create = false;
        self.snapshot.owned = Some(created.firewall);
        self.expected_rules(rules.to_vec());
        self.save_wait(id, &created.actions, journal, cancel)?;
        self.compare(cancel)?;
        journal.save(Update::Intent {
            kind: Mutation::Apply,
            firewall: Some(id),
            rules,
            foreign: &self.snapshot.foreign,
        })?;
        let actions = self.client.apply_exposure_firewall(&self.scope, id, cancel)?;
        self.save_wait(id, &actions, journal, cancel)?;
        self.snapshot.attached = true;
        if let Some(firewall) = &mut self.snapshot.owned {
            firewall.applied_to = vec![super::Resource {
                kind: "server".into(),
                server: Some(super::Server { id: self.scope.server }),
                label_selector: None,
                applied_to_resources: None,
            }];
        }
        self.compare(cancel)
    }
    /// Retire only the original provider resource after caller physical teardown.
    /// Never creates, reopens, changes rules or adopts a replacement resource ID.
    /// This does not prove worker retirement or release a Core owner reservation.
    /// # Errors
    /// Retains uncertainty on changed ownership, rules, targets or foreign baseline.
    pub fn cleanup_original(&mut self, journal: &mut dyn Journal, cancel: &Cancellation) -> Result<(), CloudError> {
        if self.retired && !self.uncertain {
            return Ok(());
        }
        self.uncertain = true;
        cancel.check()?;
        let found = if let Some(id) = self.original_id {
            self.client.inspect_exposure_firewall(&self.scope, id, cancel)?
        } else if self.unresolved_create {
            Some(
                self.client
                    .find_exposure_firewall(&self.scope, cancel)?
                    .ok_or(CloudError::CreationUnresolved)?,
            )
        } else {
            None
        };
        if let Some(firewall) = found {
            let expected = self
                .snapshot
                .owned
                .as_ref()
                .is_some_and(|prior| prior.rules == firewall.rules)
                || self.pending.as_ref().is_some_and(|rules| *rules == firewall.rules);
            if !expected {
                return Err(CloudError::IdentityMismatch);
            }
            let actual = Snapshot::read(&self.client, &self.scope, Some(firewall.id), cancel)?;
            let captured = actual.owned.as_ref().ok_or(CloudError::IdentityMismatch)?;
            if actual.foreign != self.snapshot.foreign
                || actual.ssh != self.snapshot.ssh
                || captured.rules != firewall.rules
            {
                return Err(CloudError::IdentityMismatch);
            }
            journal.save(Update::Reconciled { firewall: captured })?;
            self.original_id = Some(captured.id);
            self.unresolved_create = false;
            self.snapshot = actual;
            self.retire(journal, cancel)?;
        } else {
            let actual = Snapshot::read(&self.client, &self.scope, None, cancel)?;
            if actual.foreign != self.snapshot.foreign || actual.ssh != self.snapshot.ssh {
                return Err(CloudError::IdentityMismatch);
            }
            self.snapshot = actual;
        }
        journal.save(Update::Confirmed {
            firewall: None,
            rules: &[],
        })?;
        self.services.clear();
        self.pending = None;
        self.retired = true;
        self.uncertain = false;
        Ok(())
    }
    fn retire(&mut self, journal: &mut dyn Journal, cancel: &Cancellation) -> Result<(), CloudError> {
        let id = self.firewall_id().ok_or(CloudError::IdentityMismatch)?;
        journal.save(Update::Intent {
            kind: Mutation::Remove,
            firewall: Some(id),
            rules: &[],
            foreign: &self.snapshot.foreign,
        })?;
        let actions = self.client.remove_exposure_firewall(&self.scope, id, cancel)?;
        self.save_wait(id, &actions, journal, cancel)?;
        self.snapshot.attached = false;
        if let Some(firewall) = &mut self.snapshot.owned {
            firewall.applied_to.clear();
        }
        self.compare(cancel)?;
        journal.save(Update::Intent {
            kind: Mutation::Delete,
            firewall: Some(id),
            rules: &[],
            foreign: &self.snapshot.foreign,
        })?;
        self.client.delete_exposure_firewall(&self.scope, id, cancel)?;
        if self
            .client
            .inspect_exposure_firewall(&self.scope, id, cancel)?
            .is_some()
        {
            return Err(CloudError::IdentityMismatch);
        }
        self.snapshot.owned = None;
        self.compare(cancel)
    }
}
fn inbound(rules: &[Rule]) -> Result<(), CloudError> {
    valid_rules(rules)?;
    if rules.iter().any(|rule| rule.direction != Direction::In) {
        return Err(CloudError::Invalid("Exposure cannot replace outbound policy"));
    }
    Ok(())
}
