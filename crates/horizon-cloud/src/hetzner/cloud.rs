//! Hetzner Cloud as a host for workers, without any storage of its own: the
//! placement rules, the host plan, the worker view of a server and the records a
//! caller keeps for a cloud. Callers persist the [`Journal`] themselves, before
//! each provider request it guards.
use super::{
    catalog::Offer,
    servers::{Placement, Server},
    volumes::Volume,
};
use crate::{CloudError, CreateState, Worker, WorkerSpec, WorkerStatus, host};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod lifecycle;
mod provision;
#[cfg(test)]
mod tests;

pub use lifecycle::{Check, Cloud, Stop, StopRecords, UNRESOLVED_GRACE, check, delete, released, resumable, stop};
pub use provision::{Records, Request, provision};

/// The Hetzner app image with Docker preinstalled that the host plan expects.
pub const HOST_IMAGE: &str = "docker-ce";
/// A device path in Hetzner's form with the longest possible volume ID, for
/// checking a host plan before its volume exists: the real path can only be
/// shorter, so the user data cannot outgrow its limit after allocation.
pub const PROBE_DEVICE: &str = "/dev/disk/by-id/scsi-0HC_Volume_18446744073709551615";

/// What a cloud owns on Hetzner besides its server. The server is fenced by the
/// caller's own operation record; every change here must be durable before the
/// provider request it guards.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    /// The location of the workspace volume, which fixes where servers can run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The fence for the workspace volume.
    #[serde(default = "prepared")]
    pub volume: CreateState,
    /// A public key registered only so Hetzner generates no root password. Its
    /// private half is never kept, and the host's sshd is masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The server a stop deleted. While the cloud is still bound to it, the cloud
    /// is stopped rather than lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    /// Set when a delete starts. Provisioning refuses until the delete has
    /// finished, then starts the redeployed cloud afresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleting: bool,
    /// The recorded workspace volume was requested for the cloud's first server and
    /// no server has held it, so it is empty: a retry may delete it and place the
    /// cloud afresh, in another location if this one is sold out. Saved with the
    /// location before the volume request and cleared before a server is recorded
    /// as bound, so a volume that ever held a workspace never carries it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unused: bool,
}

fn prepared() -> CreateState {
    CreateState::Prepared
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            location: None,
            volume: CreateState::Prepared,
            key: None,
            released: None,
            deleting: false,
            unused: false,
        }
    }
}

impl Journal {
    /// Whether anything recorded here may still exist on Hetzner: the workspace
    /// volume, or the SSH key, which is recorded before it is registered.
    #[must_use]
    pub fn retains(&self) -> bool {
        self.key.is_some() || !matches!(self.volume, CreateState::Prepared | CreateState::Terminated { .. })
    }
}

/// Allowed locations and fallback server types under the current settings.
/// An exact persisted placement replaces fallback types and keeps its location
/// only while that location remains allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub locations: Vec<String>,
    pub server_types: Vec<String>,
}

impl Policy {
    /// Keep a saved exact server type and intersect its location with machine policy.
    /// # Errors
    /// Refuses malformed exact choices and locations no longer allowed by current settings.
    pub fn for_spec(&self, spec: &WorkerSpec) -> Result<Self, CloudError> {
        if !spec.exact_placement {
            return Ok(self.clone());
        }
        if spec.cpu_flavors.len() != 1
            || spec.data_centers.len() != 1
            || !spec
                .cpu_flavors
                .iter()
                .chain(&spec.data_centers)
                .all(|name| valid_placement_name(name))
        {
            return Err(CloudError::Invalid(
                "An exact Hetzner placement needs one valid server type and location",
            ));
        }
        let selected = |allowed: &[String], saved: &[String]| -> Vec<String> {
            if saved.is_empty() {
                allowed.to_vec()
            } else {
                saved.iter().filter(|value| allowed.contains(value)).cloned().collect()
            }
        };
        let policy = Self {
            locations: selected(&self.locations, &spec.data_centers),
            server_types: spec.cpu_flavors.clone(),
        };
        if policy.locations.is_empty() {
            return Err(CloudError::Invalid("The saved worker location is no longer allowed"));
        }
        Ok(policy)
    }
}

/// Whether a server type or location has Hetzner's machine-readable name format.
#[must_use]
pub fn valid_placement_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The location of an existing workspace volume. The policy can change while a
/// cloud has a volume, so a location it no longer allows is refused rather than used.
/// # Errors
/// Refuses a missing or no longer allowed location.
pub fn location(recorded: Option<&str>, policy: &Policy) -> Result<String, CloudError> {
    match recorded {
        Some(location) if policy.locations.iter().any(|name| name == location) => Ok(location.to_owned()),
        Some(_) => Err(CloudError::Invalid(
            "This cloud's workspace volume is in a location the Hetzner settings no longer allow",
        )),
        None => Err(CloudError::Invalid(
            "This cloud's workspace volume has no recorded location",
        )),
    }
}

/// For a cloud without a volume: the first allowed location, in order, where an
/// allowed server type fits, with those types. Nothing is recorded until a
/// volume is requested there.
/// # Errors
/// Refuses when no allowed location has a fitting type.
pub fn first_fit(offers: &[Offer], spec: &WorkerSpec, policy: &Policy) -> Result<(String, Vec<Placement>), CloudError> {
    let policy = policy.for_spec(spec)?;
    if policy.locations.is_empty() {
        return Err(CloudError::Invalid("Hetzner settings list no location"));
    }
    policy
        .locations
        .iter()
        .find_map(|location| {
            fit(offers, spec, &policy.server_types, location)
                .ok()
                .map(|placements| (location.clone(), placements))
        })
        .ok_or(CloudError::Invalid(
            "No configured Hetzner server type has the profile's CPU, memory and container disk in any allowed location",
        ))
}

/// Every allowed server type in the location, in order, for reconciling a
/// server that was already requested.
#[must_use]
pub fn allowed(server_types: &[String], location: &str) -> Vec<Placement> {
    server_types
        .iter()
        .map(|server_type| Placement {
            server_type: server_type.clone(),
            location: location.to_owned(),
        })
        .collect()
}

/// The configured server types, in order, whose CPU, memory and local disk fit
/// the profile in the location. Hetzner's availability flag is advisory, so it
/// is not used to skip a type.
/// # Errors
/// Refuses when no configured type fits.
pub fn fit(
    offers: &[Offer],
    spec: &WorkerSpec,
    server_types: &[String],
    location: &str,
) -> Result<Vec<Placement>, CloudError> {
    let fits = |server_type: &String| {
        offers.iter().any(|offer| {
            &offer.server_type == server_type
                && offer.location == location
                && offer.cores >= u32::from(spec.profile.cpu)
                && offer.memory_gb >= f64::from(spec.profile.memory_gb)
                && offer.disk_gb >= u32::from(spec.profile.storage.container_gb)
        })
    };
    let placements: Vec<Placement> = server_types
        .iter()
        .filter(|server_type| (!spec.exact_placement || spec.cpu_flavors.contains(server_type)) && fits(server_type))
        .map(|server_type| Placement {
            server_type: server_type.clone(),
            location: location.to_owned(),
        })
        .collect();
    if placements.is_empty() {
        return Err(CloudError::Invalid(
            "No configured Hetzner server type has the profile's CPU, memory and container disk in the workspace's location",
        ));
    }
    Ok(placements)
}

/// A shared worker's startup data has no Hetzner path yet.
/// # Errors
/// Refuses a spec with startup metadata.
pub fn supported(spec: &WorkerSpec) -> Result<(), CloudError> {
    if spec.startup_metadata.is_some() {
        return Err(CloudError::Invalid("Shared workers are not available on Hetzner yet"));
    }
    Ok(())
}

/// The host plan for this worker. The environment matches what the worker image
/// expects from any provider; nothing secret is in it but the registry login.
/// # Errors
/// Fails only if the capabilities cannot be serialized.
pub fn plan(spec: &WorkerSpec, device: &str, registry: Option<host::RegistryLogin>) -> Result<host::Plan, CloudError> {
    let mut environment = BTreeMap::from([
        ("PUBLIC_KEY".to_owned(), spec.public_key.clone()),
        ("HORIZON_CLOUD_OPERATION".to_owned(), spec.operation_id.clone()),
        (
            "HORIZON_WORKER_CAPABILITIES".to_owned(),
            serde_json::to_string(&spec.profile.capabilities)
                .map_err(|_| CloudError::Invalid("Worker capabilities cannot be described"))?,
        ),
    ]);
    // The watcher only records idle time here; Horizon stops the server from that record.
    if let Some(minutes) = spec.idle_stop_environment() {
        environment.insert(crate::IDLE_STOP_ENVIRONMENT_KEY.to_owned(), minutes);
    }
    Ok(host::Plan {
        image: spec.image_digest.clone(),
        environment,
        registry,
        workspace_device: device.to_owned(),
        shm_gb: shared_memory_gb(spec.profile.memory_gb),
    })
}

/// A quarter of the worker's memory for Chromium's shared memory, within the host's limits.
fn shared_memory_gb(memory_gb: u16) -> u8 {
    u8::try_from((memory_gb / 4).clamp(1, 16)).unwrap_or(1)
}

/// Checks a pull login against the image it pulls. `private` says whether the
/// caller holds a private registry binding for the image.
/// # Errors
/// Refuses a private image without a login, a login for another registry and
/// a spelling Docker would look up under a key the host does not write.
pub fn check_login(login: Option<&host::RegistryLogin>, private: bool, image: &str) -> Result<(), CloudError> {
    match login {
        None if private => Err(CloudError::Invalid(
            "This image is private; add hetzner.registry_pull with a read-only pull token before deploying on Hetzner",
        )),
        // Docker keeps this spelling in the image reference and looks its login up
        // under that host, while the host configuration stores Docker Hub logins
        // under Docker Hub's key, so the pull would find no credentials.
        Some(_) if image.to_ascii_lowercase().starts_with("registry-1.docker.io/") => Err(CloudError::Invalid(
            "Name a private Docker Hub image docker.io/... rather than registry-1.docker.io/... for Hetzner",
        )),
        Some(login) if registry_host(&login.server) != image_registry(image) => Err(CloudError::Invalid(
            "hetzner.registry_pull names a different registry than the profile's image",
        )),
        _ => Ok(()),
    }
}

/// The registry an image reference pulls from, as Docker resolves it: the first
/// path component when it names a host, otherwise Docker Hub.
fn image_registry(image: &str) -> String {
    match image.split_once('/') {
        Some((first, _)) if first.contains(['.', ':']) || first == "localhost" => registry_host(first),
        _ => registry_host("docker.io"),
    }
}

/// A registry host in one spelling, folding together the Docker Hub aliases the
/// host configuration stores under Docker Hub's credential key.
fn registry_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "index.docker.io" | "registry-1.docker.io" => "docker.io".into(),
        _ => host,
    }
}

/// The server as a worker. Hetzner reports the host image, not the container
/// image, so the digest is the one the cloud's user data pins; the server's
/// identity is checked through its name and label.
/// # Errors
/// Fails only if the server cannot be described.
pub fn worker(server: &Server, spec: &WorkerSpec, volume: &Volume) -> Result<Worker, CloudError> {
    let address = server.ssh_address();
    // Transitional states read as starting; an unknown one stays lost.
    let desired = match server.status() {
        WorkerStatus::Stopped => "EXITED",
        WorkerStatus::Lost => "UNKNOWN",
        _ => "RUNNING",
    };
    // The host plan gave the container exactly this environment from the same spec.
    let mut env = serde_json::json!({"HORIZON_CLOUD_OPERATION": spec.operation_id});
    if let Some(minutes) = spec.idle_stop_environment() {
        env[crate::IDLE_STOP_ENVIRONMENT_KEY] = serde_json::json!(minutes);
    }
    serde_json::from_value(serde_json::json!({
        "id": server.id.to_string(),
        "name": server.name,
        "imageName": spec.image_digest,
        "desiredStatus": desired,
        "publicIp": address.map_or_else(String::new, |address| address.ip().to_string()),
        "portMappings": address.map(|address| serde_json::json!({"22": address.port()})),
        "vcpuCount": server.server_type.cores,
        "memoryInGb": memory_gb(server.server_type.memory),
        "containerDiskInGb": server.server_type.disk,
        "volumeInGb": volume.size,
        "volumeMountPath": "/workspace",
        "dataCenterId": server.location.name,
        "privateIp": crate::hetzner::networks::horizon_address(&server.private_net).map(|ip| ip.to_string()),
        "networkZone": server.location.network_zone,
        "env": env,
    }))
    .map_err(|_| CloudError::Invalid("Hetzner server could not be described as a worker"))
}

/// Hetzner reports memory in gigabytes as a decimal; whole gigabytes are enough here.
fn memory_gb(memory: f64) -> u32 {
    // A float's display form of a whole number has no fraction, such as `4`.
    format!("{}", memory.floor()).parse().unwrap_or(0)
}

/// Whether the server and the volume hold each other in one location.
#[must_use]
pub fn holds(server: &Server, volume: &Volume) -> bool {
    server.volumes == [volume.id] && volume.server == Some(server.id) && server.location.name == volume.location.name
}

/// Whether the server's type and location are allowed now, and its location is
/// the one its volume fixed.
#[must_use]
pub fn admitted(server: &Server, policy: &Policy, location: Option<&str>) -> bool {
    policy.server_types.contains(&server.server_type.name)
        && policy.locations.contains(&server.location.name)
        && location == Some(server.location.name.as_str())
}

/// An Ed25519 public key in OpenSSH form whose private half is discarded at once.
/// # Errors
/// Fails without system randomness.
pub fn throwaway_public_key() -> Result<String, CloudError> {
    use base64::Engine as _;
    use ring::rand::SecureRandom as _;
    let mut seed = zeroize::Zeroizing::new([0_u8; 32]);
    ring::rand::SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| CloudError::Invalid("No randomness for a Hetzner SSH key"))?;
    let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let mut wire = Vec::with_capacity(51);
    for field in [b"ssh-ed25519".as_slice(), public.as_slice()] {
        wire.extend_from_slice(&u32::try_from(field.len()).unwrap_or(0).to_be_bytes());
        wire.extend_from_slice(field);
    }
    Ok(format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(wire)
    ))
}
