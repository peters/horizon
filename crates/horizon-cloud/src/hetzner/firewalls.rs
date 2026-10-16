//! Exact owned-firewall requests, never exposure approval or a management policy.
//! Persist each intent before a mutation. Reconcile uncertain responses by exact
//! identity; never repeat a create because a response was lost. Provider rules
//! combine with other firewalls and do not terminate established connections.
pub mod management;

use super::{Action, Hetzner, Method, OPERATION_LABEL, resource_name};
use crate::{Cancellation, CloudError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const SERVER_LABEL: &str = "horizon-exposure-server";
const ALLOCATION_LABEL: &str = "horizon-exposure-allocation";

/// Current expected resource ownership. These values are not a saved permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    operation: String,
    allocation: String,
    server: u64,
}
impl Scope {
    /// # Errors
    /// Refuses invalid operation names and absent numeric server identities.
    pub fn new(operation: &str, allocation: &str, server: u64) -> Result<Self, CloudError> {
        resource_name(operation)?;
        resource_name(allocation)?;
        valid_id(server)?;
        Ok(Self {
            operation: operation.into(),
            allocation: allocation.into(),
            server,
        })
    }
    fn labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (OPERATION_LABEL.into(), self.operation.clone()),
            (SERVER_LABEL.into(), self.server.to_string()),
            (ALLOCATION_LABEL.into(), self.allocation.clone()),
        ])
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    Esp,
    Gre,
}

/// A provider rule, not Horizon's shared source-admission policy. Unknown rule
/// fields refuse instead of disappearing during a subsequent rules replacement.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub direction: Direction,
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub source_ips: Vec<String>,
    pub destination_ips: Vec<String>,
}

impl Rule {
    fn valid(&self) -> bool {
        let (sources, other) = match self.direction {
            Direction::In => (&self.source_ips, &self.destination_ips),
            Direction::Out => (&self.destination_ips, &self.source_ips),
        };
        !sources.is_empty()
            && sources.len() <= 100
            && other.is_empty()
            && sources.iter().all(|ip| cidr(ip).is_some())
            && self
                .description
                .as_ref()
                .is_none_or(|s| s.len() <= 255 && !s.chars().any(char::is_control))
            && match self.protocol {
                Protocol::Tcp | Protocol::Udp => self.port.as_deref().is_some_and(valid_port),
                Protocol::Icmp | Protocol::Esp | Protocol::Gre => self.port.is_none(),
            }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Firewall {
    pub id: u64,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub rules: Vec<Rule>,
    pub applied_to: Vec<Resource>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    #[serde(rename = "type")]
    pub kind: String,
    pub server: Option<Server>,
    pub label_selector: Option<serde_json::Value>,
    pub applied_to_resources: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub id: u64,
}

impl Firewall {
    fn verify(&self, scope: &Scope) -> Result<(), CloudError> {
        valid_id(self.id)?;
        let exact_target = self.applied_to.is_empty()
            || matches!(self.applied_to.as_slice(), [resource] if resource.exact_server(scope.server));
        if self.name != resource_name(&scope.operation)? || self.labels != scope.labels() || !exact_target {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}

impl Resource {
    fn exact_server(&self, expected: u64) -> bool {
        self.kind == "server"
            && self.server.as_ref().is_some_and(|server| server.id == expected)
            && self.label_selector.is_none()
            && self
                .applied_to_resources
                .as_ref()
                .is_none_or(|resources| resources.as_array().is_some_and(Vec::is_empty))
    }
}

#[derive(Deserialize)]
struct Single {
    firewall: Firewall,
}
/// The ID must be saved before the caller waits for any returned actions.
#[derive(Debug, Deserialize)]
pub struct Created {
    pub firewall: Firewall,
    pub actions: Vec<Action>,
}
#[derive(Deserialize)]
struct Actions {
    actions: Vec<Action>,
}

impl Hetzner {
    /// Read an exact owned firewall. A 404 is absence, not permission to recreate.
    /// # Errors
    /// Refuses mismatched ownership, extra targets and label selectors.
    pub fn inspect_exposure_firewall(
        &self,
        scope: &Scope,
        id: u64,
        cancel: &Cancellation,
    ) -> Result<Option<Firewall>, CloudError> {
        valid_id(id)?;
        match self.send(Method::Get, &format!("/firewalls/{id}"), None, cancel) {
            Err(failure) if failure.not_found() => Ok(None),
            result => {
                let single: Single = serde_json::from_value(result?).map_err(|_| CloudError::InvalidResponse)?;
                if single.firewall.id != id {
                    return Err(CloudError::IdentityMismatch);
                }
                single.firewall.verify(scope)?;
                Ok(Some(single.firewall))
            }
        }
    }

    /// Reconcile a create without repeating it. Every labelled candidate must
    /// have the exact original scope; ambiguity is retained as an error.
    /// # Errors
    /// Reports foreign candidates, duplicate matches and provider failures.
    pub fn find_exposure_firewall(&self, scope: &Scope, cancel: &Cancellation) -> Result<Option<Firewall>, CloudError> {
        let mut found: Vec<Firewall> = self.list_all(
            "/firewalls",
            &format!("label_selector={OPERATION_LABEL}%3D{}", scope.operation),
            "firewalls",
            cancel,
        )?;
        if found.len() > 1 {
            return Err(CloudError::IdentityMismatch);
        }
        if let Some(firewall) = found.pop() {
            firewall.verify(scope)?;
            Ok(Some(firewall))
        } else {
            Ok(None)
        }
    }

    /// One POST, always unattached. The caller must persist Requested before
    /// this call, save the returned ID, and reconcile any uncertain result.
    /// Applying it later requires a separate reviewed management-preserving plan.
    /// # Errors
    /// Refuses invalid rules or a server not owned by the original allocation.
    pub fn create_exposure_firewall_once(
        &self,
        scope: &Scope,
        rules: &[Rule],
        cancel: &Cancellation,
    ) -> Result<Created, CloudError> {
        valid_rules(rules)?;
        self.firewall_server(scope, cancel)?;
        let value = self.send(
            Method::Post,
            "/firewalls",
            Some(json!({"name":resource_name(&scope.operation)?,"labels":scope.labels(),"rules":rules})),
            cancel,
        )?;
        let created: Created = serde_json::from_value(value).map_err(|_| CloudError::CreationUnresolved)?;
        created.firewall.verify(scope)?;
        if !created.firewall.applied_to.is_empty() {
            return Err(CloudError::IdentityMismatch);
        }
        valid_actions(&created.actions)?;
        Ok(created)
    }

    /// Replace only an exact freshly inspected owned firewall's rules. This is
    /// not an additive operation. The caller supplies the complete approved set.
    /// # Errors
    /// Refuses changed ownership/targets, invalid rules and provider failures.
    pub fn set_exposure_firewall_rules(
        &self,
        scope: &Scope,
        id: u64,
        rules: &[Rule],
        cancel: &Cancellation,
    ) -> Result<Vec<Action>, CloudError> {
        valid_rules(rules)?;
        self.firewall_server(scope, cancel)?;
        self.required_firewall(scope, id, cancel)?;
        self.firewall_action(id, "set_rules", json!({"rules":rules}), cancel)
    }

    /// Apply to the exact original server only, never a label selector. The
    /// controller must compare all existing firewalls and preserve management
    /// access before requesting this mutation. A success still needs fresh reads.
    /// # Errors
    /// Refuses another allocation, changed firewall targets and provider failure.
    pub fn apply_exposure_firewall(
        &self,
        scope: &Scope,
        id: u64,
        cancel: &Cancellation,
    ) -> Result<Vec<Action>, CloudError> {
        self.firewall_server(scope, cancel)?;
        self.required_firewall(scope, id, cancel)?;
        self.firewall_action(
            id,
            "apply_to_resources",
            json!({"apply_to":[{"type":"server","server":{"id":scope.server}}]}),
            cancel,
        )
    }

    /// Remove only the captured owned firewall from its exact original target.
    /// # Errors
    /// Refuses changed ownership/targets and provider failures.
    pub fn remove_exposure_firewall(
        &self,
        scope: &Scope,
        id: u64,
        cancel: &Cancellation,
    ) -> Result<Vec<Action>, CloudError> {
        let firewall = self.required_firewall(scope, id, cancel)?;
        if firewall.applied_to.is_empty() {
            return Ok(Vec::new());
        }
        self.firewall_action(
            id,
            "remove_from_resources",
            json!({"remove_from":[{"type":"server","server":{"id":scope.server}}]}),
            cancel,
        )
    }

    /// Delete only after a fresh read proves this owned firewall is unattached.
    /// No name search, automatic recreation or foreign-resource deletion occurs.
    /// # Errors
    /// Refuses attached or changed resources and provider failures.
    pub fn delete_exposure_firewall(&self, scope: &Scope, id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
        let Some(firewall) = self.inspect_exposure_firewall(scope, id, cancel)? else {
            return Ok(());
        };
        if !firewall.applied_to.is_empty() {
            return Err(CloudError::IdentityMismatch);
        }
        match self.send(Method::Delete, &format!("/firewalls/{id}"), None, cancel) {
            Err(failure) if failure.not_found() => Ok(()),
            result => result.map(|_| ()).map_err(CloudError::from),
        }
    }

    /// Wait for returned actions after their identities are durably saved. This
    /// does not prove current rules/attachment, service reachability or gate close.
    /// # Errors
    /// Reports invalid action batches, cancellation, timeouts and action failures.
    pub fn wait_firewall_actions(&self, actions: &[Action], cancel: &Cancellation) -> Result<(), CloudError> {
        valid_actions(actions)?;
        for action in actions {
            self.wait(action, cancel)?;
        }
        Ok(())
    }

    fn firewall_server(&self, scope: &Scope, cancel: &Cancellation) -> Result<(), CloudError> {
        self.inspect_server(scope.server, cancel)?
            .ok_or(CloudError::WorkerLost)?
            .verify(&scope.allocation)
    }
    fn required_firewall(&self, scope: &Scope, id: u64, cancel: &Cancellation) -> Result<Firewall, CloudError> {
        self.inspect_exposure_firewall(scope, id, cancel)?
            .ok_or(CloudError::IdentityMismatch)
    }
    fn firewall_action(
        &self,
        id: u64,
        action: &str,
        body: serde_json::Value,
        cancel: &Cancellation,
    ) -> Result<Vec<Action>, CloudError> {
        let value = self.send(
            Method::Post,
            &format!("/firewalls/{id}/actions/{action}"),
            Some(body),
            cancel,
        )?;
        let result: Actions = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
        valid_actions(&result.actions)?;
        Ok(result.actions)
    }
}

fn valid_id(id: u64) -> Result<(), CloudError> {
    if id == 0 || id > 9_007_199_254_740_991 {
        Err(CloudError::Invalid("Invalid Hetzner resource ID"))
    } else {
        Ok(())
    }
}
fn valid_actions(actions: &[Action]) -> Result<(), CloudError> {
    let mut ids = BTreeSet::new();
    if actions.len() > 64
        || actions.iter().any(|a| {
            valid_id(a.id).is_err()
                || !ids.insert(a.id)
                || !matches!(a.status.as_str(), "running" | "success" | "error")
        })
    {
        return Err(CloudError::InvalidResponse);
    }
    Ok(())
}
fn valid_rules(rules: &[Rule]) -> Result<(), CloudError> {
    // Hetzner combines ports and protocols for each prefix within one direction.
    // See https://docs.hetzner.com/cloud/firewalls/faq/#what-is-an-effective-rule.
    let effective: BTreeSet<_> = rules
        .iter()
        .flat_map(|r| {
            let (inbound, sources) = match r.direction {
                Direction::In => (true, &r.source_ips),
                Direction::Out => (false, &r.destination_ips),
            };
            sources
                .iter()
                .filter_map(move |s| cidr(s).map(|prefix| (inbound, prefix)))
        })
        .collect();
    if rules.len() > 50 || effective.len() > 500 || rules.iter().any(|r| !r.valid()) {
        return Err(CloudError::Invalid("Invalid Hetzner firewall rules"));
    }
    Ok(())
}
fn cidr(text: &str) -> Option<(std::net::IpAddr, u8)> {
    let (address, bits) = text.split_once('/')?;
    let bits = bits.parse::<u8>().ok()?;
    let network = match address.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(address) if bits <= 32 => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            std::net::IpAddr::V4((u32::from(address) & mask).into())
        }
        std::net::IpAddr::V6(address) if bits <= 128 => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            std::net::IpAddr::V6((u128::from(address) & mask).into())
        }
        _ => return None,
    };
    Some((network, bits))
}
fn valid_port(text: &str) -> bool {
    let port = |s: &str| s.parse::<u16>().ok().filter(|v| *v > 0);
    match text.split_once('-') {
        Some((start, end)) => port(start).zip(port(end)).is_some_and(|(a, b)| a <= b),
        None => port(text).is_some(),
    }
}
