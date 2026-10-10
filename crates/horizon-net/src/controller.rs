use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use iroh::{EndpointId, endpoint::Connection};
use serde::Serialize;

use crate::{
    ApplyOutcome, AuthorizedService, Change, ChangeKind, Error, GrantStatus, NodeStatus, Plan, PolicyState, Result,
    Status, Topology, model::parse_key, store::Store,
};

/// Clones share policy and retain the persistent state's exclusive writer lock.
/// Drop all owners before reopening the same agent state directory.
#[derive(Clone)]
pub struct Controller {
    state: Arc<Mutex<State>>,
}

struct State {
    topology: Topology,
    awaiting_confirmation: bool,
    store: Option<Store>,
    sessions: BTreeMap<u64, Session>,
    next_session: u64,
    reachable: BTreeMap<EndpointId, Instant>,
}

struct Session {
    source: EndpointId,
    destination: EndpointId,
    service: String,
    port: u16,
    connection: Connection,
    socket: Option<std::net::TcpStream>,
    confirmed: bool,
}

impl Controller {
    /// Create a non-durable controller for trusted embedded host policy.
    /// The host must persist policy before calling `apply` or `revoke`.
    /// For remote agents, use `Agent::bind_persistent` or `Agent::bind_with_store`;
    /// their controllers commit each policy mutation before changing live state.
    ///
    /// # Errors
    /// Returns an error if the initial topology is invalid.
    pub fn new(topology: Topology) -> Result<Self> {
        topology.validate()?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                topology,
                awaiting_confirmation: false,
                store: None,
                sessions: BTreeMap::new(),
                next_session: 0,
                reachable: BTreeMap::new(),
            })),
        })
    }

    pub(crate) fn bind_store(&self, store: Store) -> Result<()> {
        let mut state = self.lock();
        if state.store.is_some() {
            return Err(Error::InvalidConfiguration(
                "controller already owns persistent state".into(),
            ));
        }
        state.awaiting_confirmation = store.awaiting_confirmation();
        state.store = Some(store);
        close_invalid(&mut state);
        Ok(())
    }

    #[must_use]
    pub fn topology(&self) -> Topology {
        self.lock().topology.clone()
    }

    /// # Errors
    /// Returns an error for invalid, foreign or older topology documents.
    pub fn plan(&self, topology: Topology) -> Result<Plan> {
        make_plan(self.topology(), topology)
    }

    /// # Errors
    /// Returns an error if the plan is invalid, modified or stale, or persistence fails.
    pub fn apply(&self, plan: &Plan) -> Result<ApplyOutcome> {
        let mut state = self.lock();
        if make_plan(plan.base.clone(), plan.proposed.clone())? != *plan {
            return Err(Error::StalePlan);
        }
        if state.topology == plan.proposed {
            if state.awaiting_confirmation {
                state.store.as_ref().ok_or(Error::Denied)?.persist(&plan.proposed)?;
                state.awaiting_confirmation = false;
            }
            return Ok(ApplyOutcome {
                changed: false,
                revision: state.topology.revision,
                closed_sessions: 0,
            });
        }
        if state.topology != plan.base {
            return Err(Error::StalePlan);
        }
        if let Some(store) = &state.store {
            store.persist(&plan.proposed)?;
        }
        state.awaiting_confirmation = false;
        state.topology.clone_from(&plan.proposed);
        let closed_sessions = close_invalid(&mut state);
        Ok(ApplyOutcome {
            changed: true,
            revision: state.topology.revision,
            closed_sessions,
        })
    }

    /// # Errors
    /// Returns an error if the revision counter is exhausted or persistence fails.
    pub fn revoke(&self, grant_id: &str) -> Result<bool> {
        let mut state = self.lock();
        if !state.topology.grants.contains_key(grant_id) {
            return Ok(false);
        }
        let revision = state
            .topology
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::InvalidTopology("revision counter exhausted".into()))?;
        let mut proposed = state.topology.clone();
        proposed.grants.remove(grant_id);
        proposed.revision = revision;
        if let Some(store) = &state.store {
            store.persist(&proposed)?;
        }
        state.topology = proposed;
        close_invalid(&mut state);
        Ok(true)
    }

    /// # Errors
    /// Returns an error for unknown identities, services, or expired grants.
    pub fn authorize(&self, source_key: &str, service: &str) -> Result<AuthorizedService> {
        let state = self.lock();
        if state.awaiting_confirmation {
            return Err(Error::Denied);
        }
        authorize(&state.topology, parse_key(source_key)?, service)
    }

    #[must_use]
    pub fn status(&self) -> Status {
        let mut state = self.lock();
        close_invalid(&mut state);
        let now = unix_now();
        Status {
            policy_state: if state.awaiting_confirmation {
                PolicyState::AwaitingConfirmation
            } else {
                PolicyState::Confirmed
            },
            network: state.topology.network.clone(),
            revision: state.topology.revision,
            nodes: state
                .topology
                .nodes
                .iter()
                .map(|(name, node)| NodeStatus {
                    node: name.clone(),
                    key: node.key.clone(),
                    reachable: parse_key(&node.key).is_ok_and(|key| {
                        state
                            .reachable
                            .get(&key)
                            .is_some_and(|seen| seen.elapsed() < Duration::from_secs(30))
                            || state.sessions.values().any(|session| {
                                session.confirmed && (session.source == key || session.destination == key)
                            })
                    }),
                })
                .collect(),
            grants: state
                .topology
                .grants
                .iter()
                .map(|(id, grant)| GrantStatus {
                    id: id.clone(),
                    from: grant.from.clone(),
                    to: grant.to.clone(),
                    expires_at: grant.expires_at,
                    active: !state.awaiting_confirmation && grant.expires_at > now,
                })
                .collect(),
            sessions: state.sessions.len(),
        }
    }

    pub(crate) fn register(
        &self,
        source: EndpointId,
        destination: EndpointId,
        service: &str,
        expected_port: u16,
        connection: Connection,
    ) -> Result<u64> {
        let mut state = self.lock();
        if state.awaiting_confirmation {
            return Err(Error::Denied);
        }
        let authorized = authorize(&state.topology, source, service)?;
        if authorized.port != expected_port || connection.close_reason().is_some() {
            return Err(Error::Denied);
        }
        if state
            .topology
            .nodes
            .get(&authorized.node)
            .and_then(|node| parse_key(&node.key).ok())
            != Some(destination)
        {
            return Err(Error::Denied);
        }
        let id = state.next_session;
        state.next_session = id
            .checked_add(1)
            .ok_or_else(|| Error::Transport("session counter exhausted".into()))?;
        state.sessions.insert(
            id,
            Session {
                source,
                destination,
                service: service.into(),
                port: authorized.port,
                connection,
                socket: None,
                confirmed: false,
            },
        );
        Ok(id)
    }

    pub(crate) fn confirmed(&self, id: u64) {
        let mut state = self.lock();
        if let Some(session) = state.sessions.get_mut(&id) {
            session.confirmed = true;
        }
    }

    pub(crate) fn unregister(&self, id: u64) {
        self.lock().sessions.remove(&id);
    }

    pub(crate) fn attach_socket(&self, id: u64, socket: tokio::net::TcpStream) -> Result<tokio::net::TcpStream> {
        let socket = socket.into_std()?;
        let mut state = self.lock();
        close_invalid(&mut state);
        let Some(session) = state.sessions.get_mut(&id) else {
            let _ = socket.shutdown(std::net::Shutdown::Both);
            return Err(Error::Denied);
        };
        session.socket = Some(socket.try_clone()?);
        Ok(tokio::net::TcpStream::from_std(socket)?)
    }

    pub(crate) fn expire(&self) {
        close_invalid(&mut self.lock());
    }

    pub(crate) fn observed(&self, key: EndpointId, reachable: bool) {
        let mut state = self.lock();
        if reachable {
            state.reachable.insert(key, Instant::now());
        } else {
            state.reachable.remove(&key);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn make_plan(base: Topology, proposed: Topology) -> Result<Plan> {
    proposed.validate()?;
    if base.network != proposed.network || (base != proposed && proposed.revision <= base.revision) {
        return Err(Error::InvalidTopology(
            "network is immutable and revision must increase".into(),
        ));
    }
    let mut changes = Vec::new();
    diff(ChangeKind::Node, &base.nodes, &proposed.nodes, &mut changes)?;
    diff(ChangeKind::Service, &base.services, &proposed.services, &mut changes)?;
    diff(ChangeKind::Grant, &base.grants, &proposed.grants, &mut changes)?;
    // A changed key or target can redirect existing grants. Conservatively show a
    // widening even if the grant text itself is unchanged.
    let widens_access = changes.iter().any(|change| match change.kind {
        ChangeKind::Node | ChangeKind::Service => change.after.is_some(),
        ChangeKind::Grant => {
            let Some(next) = proposed.grants.get(&change.id) else {
                return false;
            };
            base.grants.get(&change.id).is_none_or(|old| {
                next.to != old.to
                    || next.expires_at > old.expires_at
                    || next.from.iter().any(|source| !old.from.contains(source))
            })
        }
    });
    Ok(Plan {
        base,
        proposed,
        changes,
        widens_access,
    })
}

fn diff<T: Serialize + PartialEq>(
    kind: ChangeKind,
    base: &BTreeMap<String, T>,
    next: &BTreeMap<String, T>,
    changes: &mut Vec<Change>,
) -> Result<()> {
    for id in base.keys().chain(next.keys()).collect::<BTreeSet<_>>() {
        if base.get(id) != next.get(id) {
            changes.push(Change {
                kind,
                id: id.clone(),
                before: base.get(id).map(serde_json::to_value).transpose()?,
                after: next.get(id).map(serde_json::to_value).transpose()?,
            });
        }
    }
    Ok(())
}

fn authorize(topology: &Topology, source: EndpointId, service: &str) -> Result<AuthorizedService> {
    let target = topology.services.get(service).ok_or(Error::Denied)?;
    let source_name = topology
        .nodes
        .iter()
        .find_map(|(name, node)| (parse_key(&node.key).ok() == Some(source)).then_some(name))
        .ok_or(Error::Denied)?;
    let expires_at = topology
        .grants
        .values()
        .filter(|grant| grant.to == service && grant.from.contains(source_name) && grant.expires_at > unix_now())
        .map(|grant| grant.expires_at)
        .max()
        .ok_or(Error::Denied)?;
    Ok(AuthorizedService {
        node: target.node.clone(),
        port: target.port,
        expires_at,
    })
}

fn close_invalid(state: &mut State) -> usize {
    let before = state.sessions.len();
    state.sessions.retain(|_, session| {
        let valid = !state.awaiting_confirmation
            && authorize(&state.topology, session.source, &session.service).is_ok_and(|grant| {
                grant.port == session.port
                    && state
                        .topology
                        .nodes
                        .get(&grant.node)
                        .and_then(|node| parse_key(&node.key).ok())
                        == Some(session.destination)
            })
            && session.connection.close_reason().is_none();
        if !valid {
            if let Some(socket) = &session.socket {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            session.connection.close(0_u32.into(), b"workspace access revoked");
        }
        valid
    });
    before - state.sessions.len()
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |time| time.as_secs())
}

#[cfg(test)]
mod tests;
