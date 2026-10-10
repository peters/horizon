use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub schema_version: u32,
    pub network: String,
    pub revision: u64,
    #[serde(default)]
    pub nodes: BTreeMap<String, Node>,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    #[serde(default)]
    pub grants: BTreeMap<String, Grant>,
}

impl Topology {
    #[must_use]
    pub fn empty(network: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            network: network.into(),
            revision: 0,
            nodes: BTreeMap::new(),
            services: BTreeMap::new(),
            grants: BTreeMap::new(),
        }
    }

    /// # Errors
    /// Returns an error for an unsupported schema, invalid keys or dangling references.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION || !valid_name(&self.network) {
            return Err(Error::InvalidTopology("schema or network name".into()));
        }
        let mut keys = BTreeSet::new();
        for (name, node) in &self.nodes {
            let key = parse_key(&node.key)?;
            if !valid_name(name) || !keys.insert(key) {
                return Err(Error::InvalidTopology("duplicate key or invalid node name".into()));
            }
        }
        for (name, service) in &self.services {
            if !valid_name(name) || !self.nodes.contains_key(&service.node) || service.port == 0 {
                return Err(Error::InvalidTopology("invalid service or node reference".into()));
            }
        }
        for (id, grant) in &self.grants {
            let sources: BTreeSet<_> = grant.from.iter().collect();
            if !valid_name(id)
                || grant.from.is_empty()
                || sources.len() != grant.from.len()
                || !self.services.contains_key(&grant.to)
                || grant.from.iter().any(|node| !self.nodes.contains_key(node))
                || grant.expires_at == 0
            {
                return Err(Error::InvalidTopology("invalid grant reference or expiry".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub node: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub from: Vec<String>,
    pub to: String,
    /// Absolute UTC Unix seconds. Rebooting an agent cannot renew a lease.
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub base: Topology,
    pub proposed: Topology,
    pub changes: Vec<Change>,
    pub widens_access: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Node,
    Service,
    Grant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub kind: ChangeKind,
    pub id: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub changed: bool,
    pub revision: u64,
    pub closed_sessions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyState {
    Confirmed,
    AwaitingConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub policy_state: PolicyState,
    pub network: String,
    pub revision: u64,
    pub nodes: Vec<NodeStatus>,
    pub grants: Vec<GrantStatus>,
    pub sessions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node: String,
    pub key: String,
    pub reachable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantStatus {
    pub id: String,
    pub from: Vec<String>,
    pub to: String,
    pub expires_at: u64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedService {
    pub node: String,
    pub port: u16,
    pub expires_at: u64,
}

pub(crate) fn parse_key(key: &str) -> Result<iroh::EndpointId> {
    key.strip_prefix("ed25519:")
        .unwrap_or(key)
        .parse()
        .map_err(|_| Error::InvalidTopology("invalid endpoint public key".into()))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_/.:".contains(&byte))
}
