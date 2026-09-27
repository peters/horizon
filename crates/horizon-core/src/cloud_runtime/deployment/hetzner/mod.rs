//! Deployment on Hetzner Cloud: a server runs the unchanged worker image under
//! Docker and keeps the workspace on a volume in one location. The server is
//! fenced by the deployment's `operation`; the volume, the chosen location and
//! the SSH key live in this cloud's `hetzner.json` journal, so `RunPod` clouds and
//! their records are untouched.
pub(in crate::cloud_runtime) mod lifecycle;
mod provision;
mod readiness;
#[cfg(test)]
mod tests;

pub(super) use provision::provision;

/// Checks what a Hetzner cloud needs before any record, build or provider
/// request, so an unsupported request fails at once and leaves nothing behind.
pub(super) fn preflight(cloud_id: &str, profile: &horizon_cloud::Profile, settings: &Settings) -> Result<()> {
    if !horizon_cloud::hetzner::volumes::SIZE_GB.contains(&u32::from(profile.storage.volume_gb)) {
        return Err(Error::Invalid(
            "A Hetzner workspace volume must be between 10 and 10,240 GB",
        ));
    }
    horizon_cloud::hetzner::resource_name(cloud_id).map_err(|_| {
        Error::Invalid("A Hetzner cloud needs an ID of lowercase letters, digits and hyphens, at most 48 characters")
    })?;
    if profile.idle_stop_minutes.is_some() {
        return Err(Error::Invalid(
            "idle_stop_minutes is not available on Hetzner yet; a Hetzner worker cannot stop its own billing",
        ));
    }
    let hetzner = super::sizing::hetzner(settings)?;
    hetzner.credential()?;
    hetzner.locations_for(settings.placement.as_ref())?;
    Ok(())
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

/// Checks the pull login before the deployment is recorded or its image built,
/// unless the server is already requested or bound and so only reconciled.
pub(super) fn admit(settings: &Settings, image: &str, operation: &CreateState) -> Result<()> {
    if matches!(operation, CreateState::Requested | CreateState::Bound { .. }) {
        return Ok(());
    }
    pull_login(super::sizing::hetzner(settings)?, settings.registries.as_ref(), image).map(drop)
}

/// The pull login a host needs for `image`, which provisioning loads only while
/// a server can still be created.
/// # Errors
/// Refuses a private image without a login, a login for another registry and
/// a spelling Docker would look up under a key the host does not write.
pub(super) fn pull_login(
    hetzner: &crate::cloud_runtime::settings::Hetzner,
    registries: Option<&crate::cloud_runtime::registry::Config>,
    image: &str,
) -> Result<Option<horizon_cloud::host::RegistryLogin>> {
    // A private image the machine has a registry binding for needs a pull login
    // on the host; without one the host's pull would fail after allocation.
    let private = registries
        .map(|config| config.select(image))
        .transpose()?
        .flatten()
        .is_some();
    let login = hetzner.registry_login()?;
    match &login {
        None if private => {
            return Err(Error::Invalid(
                "This image is private; add hetzner.registry_pull with a read-only pull token before deploying on Hetzner",
            ));
        }
        // Docker keeps this spelling in the image reference and looks its login up
        // under that host, while the host configuration stores Docker Hub logins
        // under Docker Hub's key, so the pull would find no credentials.
        Some(_) if image.to_ascii_lowercase().starts_with("registry-1.docker.io/") => {
            return Err(Error::Invalid(
                "Name a private Docker Hub image docker.io/... rather than registry-1.docker.io/... for Hetzner",
            ));
        }
        Some(login) if registry_host(&login.server) != image_registry(image) => {
            return Err(Error::Invalid(
                "hetzner.registry_pull names a different registry than the profile's image",
            ));
        }
        _ => {}
    }
    Ok(login)
}

/// Whether anything this cloud created on Hetzner may still exist: the
/// workspace volume, or the SSH key, which is recorded before it is registered.
pub(in crate::cloud_runtime) fn retained(root: &Path) -> Result<bool> {
    let journal = Journal::load(root)?;
    Ok(journal.key.is_some() || !matches!(journal.volume, CreateState::Prepared | CreateState::Terminated { .. }))
}
pub(super) use readiness::wait;

use super::{Error, Result, Settings};
use horizon_cloud::{
    CreateState, Worker, WorkerSpec,
    hetzner::{Hetzner, servers::Server, volumes::Volume},
};
use serde::{Deserialize, Serialize};
use std::{io::Write as _, path::Path};

const JOURNAL: &str = "hetzner.json";

/// The Hetzner client and the machine's Hetzner settings for one deployment.
pub(super) struct Compute {
    pub(super) client: Hetzner,
    pub(super) settings: crate::cloud_runtime::settings::Hetzner,
    /// Where and on what this cloud may run under the current settings and its
    /// placement. Every attempt, retry and reconnect is checked against these,
    /// not against the policy recorded when the cloud was first provisioned.
    pub(super) allowed: Allowed,
    /// The machine's private registry bindings, to tell whether an image needs a pull login.
    pub(super) registries: Option<crate::cloud_runtime::registry::Config>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Allowed {
    pub(super) locations: Vec<String>,
    pub(super) server_types: Vec<String>,
}

impl Compute {
    pub(super) fn new(settings: &Settings) -> Result<Self> {
        let hetzner = super::sizing::hetzner(settings)?.clone();
        let allowed = Allowed {
            locations: hetzner.locations_for(settings.placement.as_ref())?,
            server_types: hetzner.server_types.clone(),
        };
        Ok(Self {
            client: Hetzner::new(hetzner.credential()?),
            settings: hetzner,
            allowed,
            registries: settings.registries.clone(),
        })
    }
}

/// What this cloud owns on Hetzner besides its server. Every change is durable
/// before the provider request it guards.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    /// The location of the workspace volume, which fixes where servers can run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) location: Option<String>,
    /// The fence for the workspace volume.
    #[serde(default = "prepared")]
    pub(super) volume: CreateState,
    /// A public key registered only so Hetzner generates no root password. Its
    /// private half is never kept, and the host's sshd is masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) key: Option<String>,
    /// The server a stop deleted. While the deployment is still bound to it, the
    /// cloud is stopped rather than lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) released: Option<String>,
}

fn prepared() -> CreateState {
    CreateState::Prepared
}

impl Journal {
    pub(super) fn load(root: &Path) -> Result<Self> {
        match std::fs::read(root.join(JOURNAL)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                location: None,
                volume: CreateState::Prepared,
                key: None,
                released: None,
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Syncs the file and its directory, as the deployment record does.
    pub(super) fn save(&self, root: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::Json)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join(JOURNAL)).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }
}

/// The server as the rest of Horizon sees a worker. Hetzner reports the host
/// image, not the container image, so the digest is the one this cloud's user
/// data pins; the server's identity was checked through its name and label.
pub(super) fn worker(server: &Server, spec: &WorkerSpec, volume: &Volume) -> Result<Worker> {
    let address = server.ssh_address();
    // Transitional states read as starting; an unknown one stays lost.
    let desired = match server.status() {
        horizon_cloud::WorkerStatus::Stopped => "EXITED",
        horizon_cloud::WorkerStatus::Lost => "UNKNOWN",
        _ => "RUNNING",
    };
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
        "env": {"HORIZON_CLOUD_OPERATION": spec.operation_id},
    }))
    .map_err(|_| Error::Invalid("Hetzner server could not be described as a worker"))
}

/// Hetzner reports memory in gigabytes as a decimal; whole gigabytes are enough here.
fn memory_gb(memory: f64) -> u32 {
    // A float's display form of a whole number has no fraction, such as `4`.
    format!("{}", memory.floor()).parse().unwrap_or(0)
}

/// An Ed25519 public key in OpenSSH form whose private half is discarded at once.
fn throwaway_public_key() -> Result<String> {
    use base64::Engine as _;
    use ring::rand::SecureRandom as _;
    let mut seed = zeroize::Zeroizing::new([0_u8; 32]);
    ring::rand::SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| Error::Invalid("No randomness for a Hetzner SSH key"))?;
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
