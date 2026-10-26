//! A browser VM: one rented Hetzner server that runs the image of `packaging/browser-vm`
//! with its state volume at `/state`, and keeps that volume across a stop and a resume.
//!
//! The machine is rented through the same provider code as a cloud worker: the same
//! volume, SSH key, server fence and teardown, with the browser VM host plan instead of
//! the worker host plan. The person signs in to the sites once on the display of the
//! machine, through the SSH route of its host, and the sign-ins stay in the volume.
use std::time::Duration;

use crate::{
    Cancellation, CloudError, CreateState, Profile, Progress, Worker, WorkerSpec,
    hetzner::{
        self as hz,
        cloud::{self, Journal, Policy, Records, StopRecords},
    },
    host,
};

/// The operation ID that names and labels the machine's resources. One Horizon keeps
/// one browser VM, so the ID is fixed and a second request reconciles the same machine.
pub const OPERATION_ID: &str = "browser-vm";

/// The vCPU and memory the machine's profile asks for. A dedicated `ccx` server type
/// offers at least this, so the same request fits every size the settings allow.
const CPU: u16 = 2;
const MEMORY_GB: u16 = 8;
/// The image needs room for the browser packages beside its state volume.
const CONTAINER_GB: u16 = 20;
/// How long a delete waits before trusting that an uncertain request created nothing.
#[cfg(not(test))]
const UNRESOLVED_GRACE: Duration = cloud::UNRESOLVED_GRACE;
#[cfg(test)]
const UNRESOLVED_GRACE: Duration = Duration::ZERO;

/// What the person asks Horizon to rent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// An immutable reference of the image of `packaging/browser-vm`,
    /// `[registry/]name@sha256:<64 hex>`. A tag is refused: a machine that silently
    /// changed what it runs between two rents cannot be trusted with a sign-in.
    pub image: String,
    /// The Hetzner server type, such as `ccx13`.
    pub server_type: String,
    /// The Hetzner location of the state volume, such as `fsn1`.
    pub location: String,
    /// The size of the state volume in GB.
    pub volume_gb: u16,
    /// The person's Ed25519 public key. It is the only key that opens the machine.
    pub public_key: String,
}

/// A browser VM that runs at `ssh_target` with its state volume attached.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub server_id: String,
    /// The SSH target of the machine, `root@<address>`, as `ssh` and the settings
    /// entry `browser_host` name it.
    pub ssh_target: String,
    pub volume_id: String,
    pub volume_gb: u32,
    pub server_type: String,
    pub location: String,
    /// The server type's hourly price in euros, net of VAT.
    pub hourly_net: Option<f64>,
}

impl Request {
    /// The worker spec of this machine: the image, the key and the exact server type
    /// and location, as the provider runtime records them.
    /// # Errors
    /// Refuses a request the provider would refuse, before anything is rented.
    pub fn spec(&self) -> Result<WorkerSpec, CloudError> {
        if !self.image.contains("@sha256:") {
            return Err(CloudError::Invalid(
                "Name the browser VM image with its digest, name@sha256:..., not a tag; a machine must not change what it runs between two rents",
            ));
        }
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "provider": hz::PROVIDER,
            "image": self.image,
            "cpu": CPU,
            "memory_gb": MEMORY_GB,
            "storage": {"container_gb": CONTAINER_GB, "volume_gb": self.volume_gb},
        }))
        .map_err(|_| CloudError::Invalid("Invalid browser VM profile"))?;
        let spec = WorkerSpec {
            operation_id: OPERATION_ID.to_owned(),
            image_digest: self.image.clone(),
            profile,
            public_key: self.public_key.clone(),
            registry_auth_id: None,
            gpu_types: Vec::new(),
            cpu_flavors: vec![self.server_type.clone()],
            data_centers: vec![self.location.clone()],
            exact_placement: true,
            startup_metadata: None,
        };
        spec.validate()?;
        cloud::supported(&spec)?;
        Ok(spec)
    }

    /// Where and on what this machine may run: exactly the requested type and location.
    #[must_use]
    pub fn policy(&self) -> Policy {
        Policy {
            locations: vec![self.location.clone()],
            server_types: vec![self.server_type.clone()],
        }
    }
}

/// The fences of a machine under a run: the server it is bound to, and the state volume,
/// key and stop it keeps. Each is saved before the provider request it guards.
pub struct Fences<'a> {
    pub operation: &'a mut CreateState,
    pub journal: &'a mut Journal,
}

/// Rents the machine, or reconnects to the one the records name, and returns it once the
/// server holds the state volume. A finished delete starts afresh; a stop and a resume
/// keep the state volume and its sign-ins.
/// # Errors
/// Refuses invalid requests before any request, and reports provider and persistence
/// failures. A failed create leaves a record that a retry or a delete resumes from.
pub fn deploy(
    client: &hz::Hetzner,
    request: &Request,
    login: Option<host::RegistryLogin>,
    fences: Fences<'_>,
    records: &mut impl Records,
    cancel: &Cancellation,
    progress: impl FnMut(Progress),
) -> Result<Machine, CloudError> {
    let spec = request.spec()?;
    let policy = request.policy();
    let Fences { operation, journal } = fences;
    // Only a finished delete leaves records that a new machine may replace; any other
    // run reconnects to what it has.
    let fresh = journal.deleting || matches!(journal.volume, CreateState::Terminated { .. });
    let worker = cloud::provision(
        client,
        cloud::Request {
            spec: &spec,
            policy: &policy,
            login,
            fresh,
            purpose: host::Purpose::BrowserVm,
        },
        operation,
        journal,
        records,
        cancel,
        progress,
    )?;
    machine(&worker, request, journal)
}

/// Releases the server and keeps the state volume, so the sign-ins of its profiles stay.
/// # Errors
/// Refuses an unbound or foreign server and reports provider failures.
pub fn stop(
    client: &hz::Hetzner,
    fences: &mut Fences<'_>,
    records: &mut impl StopRecords,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    cloud::stop(
        cloud::Cloud {
            client,
            operation_id: OPERATION_ID,
            cancel,
        },
        fences.operation,
        fences.journal,
        records,
    )
}

/// Clears the released server's fence so the next [`deploy`] rents a new server that
/// attaches the same state volume. `stopped` says the caller recorded a finished stop.
/// # Errors
/// Refuses anything but a finished stop.
pub fn prepare_resume(operation: &mut CreateState, journal: &mut Journal, stopped: bool) -> Result<(), CloudError> {
    cloud::resumable(operation, journal, stopped)?;
    *operation = CreateState::Prepared;
    journal.released = None;
    // A server held this volume, so it holds the sign-ins of the profiles.
    journal.unused = false;
    Ok(())
}

/// Removes the machine, its state volume and its SSH key, each proven absent.
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub fn delete(
    client: &hz::Hetzner,
    fences: &mut Fences<'_>,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    cloud::delete(
        cloud::Cloud {
            client,
            operation_id: OPERATION_ID,
            cancel,
        },
        fences.operation,
        fences.journal,
        records,
        UNRESOLVED_GRACE,
    )
}

/// The machine a placed server is, as the person's tools name it.
fn machine(worker: &Worker, request: &Request, journal: &Journal) -> Result<Machine, CloudError> {
    let address = worker
        .ssh_address()
        .map(|address| address.ip().to_string())
        .or_else(|| worker.public_ip.map(|ip| ip.to_string()))
        .ok_or(CloudError::Invalid(
            "The browser VM has no address yet; check again shortly",
        ))?;
    let volume_id = match &journal.volume {
        CreateState::Bound { worker_id } => worker_id.clone(),
        _ => return Err(CloudError::Invalid("The browser VM has no state volume")),
    };
    Ok(Machine {
        server_id: worker.id.clone(),
        // Hetzner installs the key of the request for root, and the image has no other
        // login, so the person's own identity opens the machine.
        ssh_target: format!("root@{address}"),
        volume_id,
        volume_gb: u32::from(request.volume_gb),
        server_type: request.server_type.clone(),
        location: request.location.clone(),
        hourly_net: worker.cost_per_hr,
    })
}

#[cfg(test)]
mod tests;
