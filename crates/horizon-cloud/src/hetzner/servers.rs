//! Servers. A worker is one server named and labelled after its operation; the
//! caller durably persists `CreateState` before every create request.
use super::{Action, Failure, Hetzner, Method, OPERATION_LABEL, resource_name, valid_name, volumes::Volume};
use crate::{Cancellation, CloudError, CreateState, Progress, Reason, WorkerStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

/// Hetzner's limit for server user data.
const USER_DATA_LIMIT: usize = 32 * 1024;

/// One server type in one location, tried in the caller's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub server_type: String,
    pub location: String,
}

pub struct ServerRequest<'a> {
    pub operation_id: &'a str,
    /// Tried in order; only a capacity refusal moves on to the next one.
    pub placements: &'a [Placement],
    /// The host operating system image, such as `docker-ce`.
    pub image: &'a str,
    pub user_data: &'a str,
    /// The operation's workspace volume, attached at creation without automount.
    pub volume: Option<&'a Volume>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Server {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub public_net: PublicNet,
    pub server_type: ServerType,
    pub location: Location,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub volumes: Vec<u64>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PublicNet {
    pub ipv4: Option<Ipv4>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Ipv4 {
    pub ip: Ipv4Addr,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ServerType {
    pub name: String,
    pub cores: u32,
    /// Gigabytes.
    pub memory: f64,
    /// Gigabytes of local disk.
    pub disk: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Location {
    pub name: String,
}

impl Server {
    #[must_use]
    pub fn ssh_address(&self) -> Option<SocketAddr> {
        Some(SocketAddr::new(IpAddr::V4(self.public_net.ipv4.as_ref()?.ip), 22))
    }
    #[must_use]
    pub fn status(&self) -> WorkerStatus {
        match self.status.as_str() {
            "running" if self.ssh_address().is_some() => WorkerStatus::Running,
            "running" | "initializing" | "starting" | "migrating" | "rebuilding" => WorkerStatus::Starting,
            "stopping" | "off" => WorkerStatus::Stopped,
            _ => WorkerStatus::Lost,
        }
    }
    /// # Errors
    /// Prevents adopting, powering or deleting a server another operation owns.
    pub fn verify(&self, operation_id: &str) -> Result<(), CloudError> {
        if self.name != resource_name(operation_id)?
            || self.labels.get(OPERATION_LABEL).map(String::as_str) != Some(operation_id)
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Created {
    server: Server,
    action: Action,
}
#[derive(Deserialize)]
struct Single {
    server: Server,
}
#[derive(Deserialize)]
struct Acted {
    action: Action,
}

impl Hetzner {
    /// The caller must hold its operation lock throughout this call. `persist`
    /// must durably commit each transition before returning. A Requested
    /// operation only reconciles by label; finding nothing never permits a second POST.
    /// # Errors
    /// Reports invalid requests, capacity, ambiguous creation and identity conflicts.
    pub fn ensure_server(
        &self,
        request: &ServerRequest<'_>,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
    ) -> Result<Server, CloudError> {
        validate(request)?;
        cancel.check()?;
        progress(Progress::Reconciling);
        match state {
            CreateState::Bound { worker_id } => {
                let id = worker_id
                    .parse()
                    .map_err(|_| CloudError::Invalid("Invalid worker ID"))?;
                let server = self.inspect_server(id, cancel)?.ok_or(CloudError::WorkerLost)?;
                server.verify(request.operation_id)?;
                return Ok(server);
            }
            CreateState::Terminated { .. } => return Err(CloudError::WorkerLost),
            CreateState::Requested => {
                let server = self.reconcile(request.operation_id, state, cancel, &mut persist)?;
                progress(Progress::WorkerFound(server.id.to_string()));
                return Ok(server);
            }
            CreateState::Prepared => {}
        }
        let mut found = self.find_servers(request.operation_id, cancel)?;
        if found.len() > 1 {
            return Err(CloudError::DuplicateWorkers);
        }
        if let Some(server) = found.pop() {
            server.verify(request.operation_id)?;
            bind(state, &server, &mut persist)?;
            progress(Progress::WorkerFound(server.id.to_string()));
            return Ok(server);
        }
        let mut refusal = Reason::default();
        for placement in request.placements {
            cancel.check()?;
            persist(&CreateState::Requested)?;
            *state = CreateState::Requested;
            progress(Progress::Requesting);
            match self.send(Method::Post, "/servers", Some(create_body(request, placement)?), cancel) {
                Ok(value) => {
                    let created: Created = serde_json::from_value(value).map_err(|_| CloudError::CreationUnresolved)?;
                    created.server.verify(request.operation_id)?;
                    bind(state, &created.server, &mut persist)?;
                    progress(Progress::WorkerFound(created.server.id.to_string()));
                    // The server stays bound if its creation fails later, so it can be deleted.
                    self.wait(&created.action, cancel)?;
                    return Ok(created.server);
                }
                Err(failure) if failure.name_taken() => {
                    let server = self.reconcile(request.operation_id, state, cancel, &mut persist)?;
                    progress(Progress::WorkerFound(server.id.to_string()));
                    return Ok(server);
                }
                Err(failure) if failure.capacity() || failure.definite() => {
                    persist(&CreateState::Prepared)?;
                    *state = CreateState::Prepared;
                    if !failure.capacity() {
                        return Err(failure.into());
                    }
                    if let Failure::Provider { reason, .. } = failure {
                        refusal = reason;
                    }
                }
                Err(failure) => return Err(failure.into()),
            }
        }
        Err(CloudError::Rejected(refusal))
    }

    /// # Errors
    /// Returns transport, authentication or response errors. HTTP 404 is a missing server.
    pub fn inspect_server(&self, id: u64, cancel: &Cancellation) -> Result<Option<Server>, CloudError> {
        match self.send(Method::Get, &format!("/servers/{id}"), None, cancel) {
            Err(failure) if failure.not_found() => Ok(None),
            result => {
                let single: Single = serde_json::from_value(result?).map_err(|_| CloudError::InvalidResponse)?;
                if single.server.id != id {
                    return Err(CloudError::IdentityMismatch);
                }
                Ok(Some(single.server))
            }
        }
    }

    /// Servers carrying the operation's label, whatever their name.
    /// # Errors
    /// Refuses invalid operation IDs and reports provider failures.
    pub fn find_servers(&self, operation_id: &str, cancel: &Cancellation) -> Result<Vec<Server>, CloudError> {
        resource_name(operation_id)?;
        Ok(self.list_all(
            "/servers",
            &format!("label_selector={OPERATION_LABEL}%3D{operation_id}"),
            "servers",
            cancel,
        )?)
    }

    /// Starts a stopped server of this operation and waits for the provider to confirm.
    /// # Errors
    /// Refuses other operations' servers and reports provider failures.
    pub fn power_on(&self, operation_id: &str, id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
        self.act(operation_id, id, "poweron", cancel)
    }

    /// Asks the operating system to shut down. The provider confirms only that it
    /// sent the request; the guest may ignore it, so watch the status and fall back
    /// to `power_off`. A powered-off server is still billed.
    /// # Errors
    /// As `power_on`.
    pub fn shutdown(&self, operation_id: &str, id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
        self.act(operation_id, id, "shutdown", cancel)
    }

    /// Cuts power immediately, like pulling the plug; unsynced writes can be lost.
    /// # Errors
    /// As `power_on`.
    pub fn power_off(&self, operation_id: &str, id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
        self.act(operation_id, id, "poweroff", cancel)
    }

    /// Deletes only the server recorded for this operation and proves it is gone.
    /// Attached volumes are detached by the provider and kept.
    /// # Errors
    /// Refuses unbound or mismatching servers and reports unfinished deletion.
    pub fn delete_server(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
    ) -> Result<(), CloudError> {
        let recorded = match state {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            _ => return Err(CloudError::CreationUnresolved),
        };
        let id: u64 = recorded.parse().map_err(|_| CloudError::Invalid("Invalid worker ID"))?;
        progress(Progress::ConfirmingWorker);
        if let Some(server) = self.inspect_server(id, cancel)? {
            server.verify(operation_id)?;
            progress(Progress::Terminating);
            match self.send(Method::Delete, &format!("/servers/{id}"), None, cancel) {
                Ok(value) => {
                    let acted: Acted = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
                    self.wait(&acted.action, cancel)?;
                }
                Err(failure) if failure.not_found() => {}
                Err(failure) => return Err(failure.into()),
            }
            progress(Progress::ConfirmingTermination);
            if self.inspect_server(id, cancel)?.is_some() {
                return Err(CloudError::Invalid("Termination pending; reconcile again"));
            }
        }
        let next = CreateState::Terminated { worker_id: recorded };
        persist(&next)?;
        *state = next;
        Ok(())
    }

    fn act(&self, operation_id: &str, id: u64, action: &str, cancel: &Cancellation) -> Result<(), CloudError> {
        self.inspect_server(id, cancel)?
            .ok_or(CloudError::WorkerLost)?
            .verify(operation_id)?;
        let value = self.send(Method::Post, &format!("/servers/{id}/actions/{action}"), None, cancel)?;
        let acted: Acted = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
        self.wait(&acted.action, cancel)
    }

    fn reconcile(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<Server, CloudError> {
        let mut found = self.find_servers(operation_id, cancel)?;
        match found.len() {
            0 => Err(CloudError::CreationUnresolved),
            1 => {
                let server = found.remove(0);
                server.verify(operation_id)?;
                bind(state, &server, persist)?;
                Ok(server)
            }
            _ => Err(CloudError::DuplicateWorkers),
        }
    }
}

fn validate(request: &ServerRequest<'_>) -> Result<(), CloudError> {
    resource_name(request.operation_id)?;
    if request.placements.is_empty()
        || request
            .placements
            .iter()
            .any(|placement| !valid_name(&placement.server_type) || !valid_name(&placement.location))
    {
        return Err(CloudError::Invalid(
            "Select at least one Hetzner server type and location",
        ));
    }
    if !valid_name(request.image) {
        return Err(CloudError::Invalid("Invalid Hetzner host image"));
    }
    if request.user_data.is_empty() || request.user_data.len() > USER_DATA_LIMIT {
        return Err(CloudError::Invalid(
            "Hetzner user data must be between 1 byte and 32 KiB",
        ));
    }
    if let Some(volume) = request.volume {
        volume.verify(request.operation_id)?;
        if request
            .placements
            .iter()
            .any(|placement| placement.location != volume.location.name)
        {
            return Err(CloudError::Invalid(
                "A server can only attach a volume from its own location",
            ));
        }
    }
    Ok(())
}

fn create_body(request: &ServerRequest<'_>, placement: &Placement) -> Result<Value, CloudError> {
    let mut body = json!({
        "name": resource_name(request.operation_id)?,
        "server_type": placement.server_type,
        "location": placement.location,
        "image": request.image,
        "user_data": request.user_data,
        "labels": {OPERATION_LABEL: request.operation_id},
        "start_after_create": true,
        "public_net": {"enable_ipv4": true, "enable_ipv6": true},
    });
    if let Some(volume) = request.volume {
        body["volumes"] = json!([volume.id]);
        body["automount"] = json!(false);
    }
    Ok(body)
}

fn bind(
    state: &mut CreateState,
    server: &Server,
    persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    let next = CreateState::Bound {
        worker_id: server.id.to_string(),
    };
    persist(&next)?;
    *state = next;
    Ok(())
}
