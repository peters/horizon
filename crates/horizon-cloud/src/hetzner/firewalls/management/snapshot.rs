use super::super::{Firewall, Scope, Single, valid_id, valid_rules};
use crate::{
    Cancellation, CloudError,
    hetzner::{Hetzner, Method},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub foreign: BTreeMap<u64, Firewall>,
    pub owned: Option<Firewall>,
    pub attached: bool,
    pub ssh: SocketAddr,
}

impl Snapshot {
    pub fn read(
        client: &Hetzner,
        scope: &Scope,
        owned: Option<u64>,
        cancel: &Cancellation,
    ) -> Result<Self, CloudError> {
        let first = attachments(client, scope, cancel)?;
        let mut foreign = BTreeMap::new();
        let mut current = None;
        for id in &first.ids {
            let mut firewall = read_firewall(client, *id, cancel)?;
            if Some(*id) == owned {
                firewall.verify(scope)?;
                for resource in &mut firewall.applied_to {
                    resource.applied_to_resources = None;
                }
                current = Some(firewall);
            } else {
                validate_foreign(&firewall, scope.server)?;
                foreign.insert(*id, firewall);
            }
        }
        if let Some(id) = owned.filter(|_| current.is_none()) {
            let firewall = read_firewall(client, id, cancel)?;
            firewall.verify(scope)?;
            if !firewall.applied_to.is_empty() {
                return Err(CloudError::IdentityMismatch);
            }
            current = Some(firewall);
        }
        if first != attachments(client, scope, cancel)? {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(Self {
            foreign,
            owned: current,
            attached: owned.is_some_and(|id| first.ids.contains(&id)),
            ssh: first.ssh,
        })
    }
}

#[derive(PartialEq, Eq)]
struct AttachmentSet {
    ids: BTreeSet<u64>,
    ssh: SocketAddr,
}

fn attachments(client: &Hetzner, scope: &Scope, cancel: &Cancellation) -> Result<AttachmentSet, CloudError> {
    let server = client
        .inspect_server(scope.server, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    server.verify(&scope.allocation)?;
    if server.id != scope.server || server.status != "running" {
        return Err(CloudError::IdentityMismatch);
    }
    let ssh = server.ssh_address().ok_or(CloudError::InvalidResponse)?;
    let entries = server.public_net.firewalls.ok_or(CloudError::InvalidResponse)?;
    let mut ids = BTreeSet::new();
    if entries.len() > 5 {
        return Err(CloudError::InvalidResponse);
    }
    for entry in entries {
        valid_id(entry.id)?;
        if entry.status != "applied" || !ids.insert(entry.id) {
            return Err(CloudError::IdentityMismatch);
        }
    }
    Ok(AttachmentSet { ids, ssh })
}

fn read_firewall(client: &Hetzner, id: u64, cancel: &Cancellation) -> Result<Firewall, CloudError> {
    let single: Single = serde_json::from_value(client.send(Method::Get, &format!("/firewalls/{id}"), None, cancel)?)
        .map_err(|_| CloudError::InvalidResponse)?;
    if single.firewall.id != id {
        return Err(CloudError::IdentityMismatch);
    }
    valid_rules(&single.firewall.rules)?;
    Ok(single.firewall)
}

fn validate_foreign(firewall: &Firewall, server: u64) -> Result<(), CloudError> {
    let mut ids = BTreeSet::new();
    if firewall.applied_to.is_empty() || firewall.applied_to.len() > 100 {
        return Err(CloudError::IdentityMismatch);
    }
    for resource in &firewall.applied_to {
        let id = resource
            .server
            .as_ref()
            .map(|s| s.id)
            .ok_or(CloudError::IdentityMismatch)?;
        valid_id(id)?;
        if !resource.exact_server(id) || !ids.insert(id) {
            return Err(CloudError::IdentityMismatch);
        }
    }
    if !ids.contains(&server) {
        return Err(CloudError::IdentityMismatch);
    }
    Ok(())
}
