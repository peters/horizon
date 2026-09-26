//! Portable worker lifecycle. The caller durably persists `CreateState` before I/O.
use crate::{Profile, Reason, valid_id, valid_image};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// # Errors
    /// Returns `Cancelled` when cancellation was requested.
    pub fn check(&self) -> Result<(), CloudError> {
        if self.is_cancelled() {
            Err(CloudError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Only caller-supplied credentials; intentionally neither serializable nor printable.
pub struct Credential(zeroize::Zeroizing<String>);
impl Credential {
    /// # Errors
    /// Rejects empty keys and invalid HTTP header characters.
    pub fn new(value: String) -> Result<Self, CloudError> {
        if value.is_empty() || value.bytes().any(|b| b <= 32 || b >= 127) {
            return Err(CloudError::Invalid("Invalid credential"));
        }
        Ok(Self(zeroize::Zeroizing::new(value)))
    }
    pub(crate) fn value(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WorkerSpec {
    pub operation_id: String,
    pub image_digest: String,
    pub profile: Profile,
    pub public_key: String,
    pub registry_auth_id: Option<String>,
    pub gpu_types: Vec<String>,
    pub cpu_flavors: Vec<String>,
    pub data_centers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_metadata: Option<crate::StartupMetadata>,
}
impl WorkerSpec {
    #[must_use]
    pub fn name(&self) -> String {
        format!("horizon-cloud-{}", self.operation_id)
    }
    /// The idle period a dedicated worker carries in its environment. Workers
    /// started through initialization never run the watcher.
    #[must_use]
    pub fn idle_stop_environment(&self) -> Option<String> {
        self.startup_metadata
            .is_none()
            .then_some(self.profile.idle_stop_minutes)
            .flatten()
            .map(|minutes| minutes.to_string())
    }
    /// # Errors
    /// Requires immutable images, supported profiles and explicit GPU selection.
    pub fn validate(&self) -> Result<(), CloudError> {
        self.profile
            .validate(false)
            .map_err(|_| CloudError::Invalid("Invalid worker profile"))?;
        if !valid_id(&self.operation_id) || !valid_image(&self.image_digest) || !self.image_digest.contains("@sha256:")
        {
            return Err(CloudError::Invalid(
                "Worker requires a stable operation ID and immutable image digest",
            ));
        }
        if !valid_public_key(&self.public_key) {
            return Err(CloudError::Invalid("Worker requires an Ed25519 public key"));
        }
        if (self.profile.gpu && self.gpu_types.is_empty()) || (!self.profile.gpu && self.cpu_flavors.is_empty()) {
            return Err(CloudError::Invalid(
                "Select an explicit CPU flavor or GPU type in machine settings",
            ));
        }
        Ok(())
    }
    /// # Errors
    /// Also rejects CPU flavors that cannot offer the profile. Only new requests
    /// are checked, so saved workers stay reconcilable when flavor limits change.
    pub fn validate_request(&self) -> Result<(), CloudError> {
        self.validate()?;
        if !self.profile.gpu {
            crate::runpod::volumes::validate_request_size(u32::from(self.profile.storage.volume_gb))?;
        }
        if !self.profile.gpu
            && self.cpu_flavors.iter().any(|flavor| {
                !crate::runpod::flavors::Flavor::get(flavor).is_some_and(|flavor| {
                    flavor.fits(
                        self.profile.cpu,
                        self.profile.memory_gb,
                        self.profile.storage.container_gb,
                    )
                })
            })
        {
            return Err(CloudError::Invalid(
                "Selected CPU flavors cannot offer the profile's vCPU, memory and container disk",
            ));
        }
        Ok(())
    }
    /// The size, capabilities, placement, key and operation are bound to the
    /// worker; only the image and the credential that pulls it may change.
    /// # Errors
    /// Refuses any other difference and a replacement with the same image.
    pub fn verify_replacement(&self, next: &Self) -> Result<(), CloudError> {
        let Self {
            operation_id,
            image_digest,
            profile,
            public_key,
            registry_auth_id: _,
            gpu_types,
            cpu_flavors,
            data_centers,
            startup_metadata,
        } = next;
        if *operation_id != self.operation_id
            || *profile != self.profile
            || *public_key != self.public_key
            || *gpu_types != self.gpu_types
            || *cpu_flavors != self.cpu_flavors
            || *data_centers != self.data_centers
            || *startup_metadata != self.startup_metadata
        {
            return Err(CloudError::IdentityChange);
        }
        if *image_digest == self.image_digest {
            return Err(CloudError::Invalid("Replacement image is the worker's current image"));
        }
        Ok(())
    }
}
/// Which image of a replacement a worker reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSide {
    Previous,
    Next,
}
impl ImageSide {
    pub(crate) fn of(image: &str, current: &WorkerSpec, next: &WorkerSpec) -> Option<Self> {
        if image == current.image_digest {
            Some(Self::Previous)
        } else if image == next.image_digest {
            Some(Self::Next)
        } else {
            None
        }
    }
}
/// Whether a public identity satisfies the worker SSH key contract.
#[must_use]
pub fn valid_public_key(value: &str) -> bool {
    if value.len() > 4096 || value.contains(['\n', '\r', '\0']) {
        return false;
    }
    let mut fields = value.split_ascii_whitespace();
    if fields.next() != Some("ssh-ed25519") {
        return false;
    }
    let Some(encoded) = fields.next() else {
        return false;
    };
    let mut wire = [0; 51];
    base64::engine::general_purpose::STANDARD.decode_slice(encoded, &mut wire) == Ok(wire.len())
        && wire.starts_with(b"\0\0\0\x0bssh-ed25519\0\0\0\x20")
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CreateState {
    Prepared,
    /// Persist before POST. Empty reconciliation never clears this fence.
    Requested,
    Bound {
        worker_id: String,
    },
    Terminated {
        worker_id: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WorkerStatus {
    Starting,
    Running,
    Stopped,
    Lost,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkVolume {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(default)]
    pub data_center_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worker {
    pub id: String,
    pub name: String,
    #[serde(alias = "image")]
    pub image_name: String,
    pub desired_status: String,
    #[serde(default, deserialize_with = "optional_address")]
    pub public_ip: Option<IpAddr>,
    #[serde(default)]
    pub port_mappings: Option<BTreeMap<String, u16>>,
    #[serde(default)]
    #[serde(deserialize_with = "optional_number")]
    pub cost_per_hr: Option<f64>,
    /// Effective hourly rate after the account's savings plans. Omitted when
    /// absent, so records saved before this field existed re-encode byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "optional_number")]
    pub adjusted_cost_per_hr: Option<f64>,
    /// RFC 3339 time of the latest start or resume, kept verbatim for lossless
    /// records and omitted when absent like the adjusted rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started_at: Option<String>,
    #[serde(default)]
    pub memory_in_gb: Option<u32>,
    #[serde(default)]
    pub vcpu_count: Option<u32>,
    #[serde(default)]
    pub gpu_count: Option<u32>,
    #[serde(default)]
    pub container_disk_in_gb: Option<u32>,
    #[serde(default)]
    pub volume_in_gb: Option<u32>,
    #[serde(default)]
    pub volume_mount_path: Option<String>,
    #[serde(default)]
    pub network_volume: Option<NetworkVolume>,
    /// The data center the provider placed the worker in. Omitted when absent, so
    /// records saved before this field existed re-encode byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_center_id: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
impl Worker {
    /// The data center the worker landed in, from the pod or its workspace volume.
    #[must_use]
    pub fn data_center(&self) -> Option<&str> {
        self.data_center_id
            .as_deref()
            .or_else(|| self.network_volume.as_ref()?.data_center_id.as_deref())
    }

    #[must_use]
    pub fn ssh_address(&self) -> Option<SocketAddr> {
        Some(SocketAddr::new(
            self.public_ip?,
            *self.port_mappings.as_ref()?.get("22")?,
        ))
    }
    #[must_use]
    pub fn status(&self) -> WorkerStatus {
        match self.desired_status.as_str() {
            "RUNNING" if self.ssh_address().is_some() => WorkerStatus::Running,
            "RUNNING" => WorkerStatus::Starting,
            "EXITED" => WorkerStatus::Stopped,
            _ => WorkerStatus::Lost,
        }
    }
    /// # Errors
    /// Prevents adopting or deleting resources with mismatching identities.
    pub fn verify(&self, spec: &WorkerSpec) -> Result<(), CloudError> {
        if !valid_id(&self.id) || self.name != spec.name() || self.image_name != spec.image_digest {
            return Err(CloudError::IdentityMismatch);
        }
        if self
            .env
            .get("HORIZON_CLOUD_OPERATION")
            .is_some_and(|id| id != &spec.operation_id)
        {
            return Err(CloudError::IdentityMismatch);
        }
        if self.env.get(crate::startup::ENVIRONMENT_KEY).map(String::as_str)
            != spec.startup_metadata.as_ref().map(crate::StartupMetadata::as_str)
            || (spec.startup_metadata.is_some() && self.env.get("HORIZON_CLOUD_OPERATION") != Some(&spec.operation_id))
        {
            return Err(CloudError::IdentityMismatch);
        }
        // A worker without its recorded idle period would never stop, or stop at the wrong time.
        if self.env.get(crate::IDLE_STOP_ENVIRONMENT_KEY).cloned() != spec.idle_stop_environment() {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
    /// As `verify`, while the worker's image is being replaced: it may report
    /// either image. Only replacement code accepts a worker this way.
    /// # Errors
    /// Refuses mismatching identities, any third image and replacements that
    /// change more than the image and its registry credential.
    pub fn verify_either(&self, current: &WorkerSpec, next: &WorkerSpec) -> Result<ImageSide, CloudError> {
        current.verify_replacement(next)?;
        let side = ImageSide::of(&self.image_name, current, next).ok_or(CloudError::IdentityMismatch)?;
        self.verify(match side {
            ImageSide::Previous => current,
            ImageSide::Next => next,
        })?;
        Ok(side)
    }
    /// # Errors
    /// Inspect actual assigned resources separately after persisting the worker identity.
    pub fn verify_resources(&self, spec: &WorkerSpec) -> Result<(), CloudError> {
        self.verify_resources_with_volume(spec, None)
    }
    /// # Errors
    /// Requires the exact owned volume and mount before source or credentials may be transferred.
    pub fn verify_resources_with_volume(
        &self,
        spec: &WorkerSpec,
        expected_volume: Option<&crate::runpod::volumes::Volume>,
    ) -> Result<(), CloudError> {
        if self.vcpu_count.is_none() || self.memory_in_gb.is_none() || (spec.profile.gpu && self.gpu_count.is_none()) {
            return Err(CloudError::Invalid(
                "Provider has not confirmed the assigned worker resources",
            ));
        }
        if self.vcpu_count.is_some_and(|v| v < u32::from(spec.profile.cpu))
            || self.memory_in_gb.is_some_and(|v| v < u32::from(spec.profile.memory_gb))
            || (spec.profile.gpu && self.gpu_count == Some(0))
        {
            return Err(CloudError::Invalid(
                "Assigned worker does not meet the requested resource profile",
            ));
        }
        if let Some(expected) = expected_volume {
            expected.verify_worker_spec(spec)?;
            if !self.network_volume.as_ref().is_some_and(|assigned| {
                assigned.id.as_deref() == Some(&expected.id)
                    && assigned.size == Some(expected.size)
                    && assigned.data_center_id.as_deref() == Some(&expected.data_center_id)
            }) || self.volume_in_gb != Some(0)
            {
                return Err(CloudError::Invalid(
                    "Assigned workspace volume differs from the recorded storage allocation",
                ));
            }
        } else if self.network_volume.is_some() {
            return Err(CloudError::Invalid(
                "Assigned worker has an unsupported network volume; inspect or explicitly delete the worker",
            ));
        }
        if self.container_disk_in_gb.is_none() || self.volume_in_gb.is_none() || self.volume_mount_path.is_none() {
            return Err(CloudError::Invalid(
                "Provider has not confirmed the assigned worker storage; inspect or explicitly delete the worker",
            ));
        }
        if self
            .container_disk_in_gb
            .is_some_and(|size| size < u32::from(spec.profile.storage.container_gb))
            || (expected_volume.is_none()
                && self
                    .volume_in_gb
                    .is_some_and(|size| size < u32::from(spec.profile.storage.volume_gb)))
            || self.volume_mount_path.as_deref() != Some("/workspace")
        {
            return Err(CloudError::Invalid(
                "Assigned worker storage does not meet the requested profile; inspect or explicitly delete the worker",
            ));
        }
        Ok(())
    }
}
/// Deletion steps are reported before their request is sent, so a stalled
/// request is named while it waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Reconciling,
    Requesting,
    WorkerFound(String),
    ConfirmingWorker,
    /// The delete request cannot be recalled once sent.
    Terminating,
    ConfirmingTermination,
    ConfirmingVolume,
    /// Listing the workers that could still mount the workspace volume.
    CheckingAttachments,
    /// Inspecting the mounts of listed worker `worker` of `workers`, counted from 1.
    InspectingMounts {
        worker: usize,
        workers: usize,
    },
    DeletingVolume,
    ConfirmingVolumeDeletion,
}
impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reconciling => f.write_str("Reconciling the worker request"),
            Self::Requesting => f.write_str("Requesting a worker"),
            Self::WorkerFound(id) => write!(f, "Found worker {id}"),
            Self::ConfirmingWorker => f.write_str("Confirming worker identity"),
            Self::Terminating => f.write_str("Requesting worker deletion"),
            Self::ConfirmingTermination => f.write_str("Confirming worker removal"),
            Self::ConfirmingVolume => f.write_str("Confirming workspace storage identity"),
            Self::CheckingAttachments => f.write_str("Checking workspace storage attachments"),
            Self::InspectingMounts { worker, workers } => {
                write!(
                    f,
                    "Checking workspace storage attachments · worker {worker} of {workers}"
                )
            }
            Self::DeletingVolume => f.write_str("Requesting workspace storage deletion"),
            Self::ConfirmingVolumeDeletion => f.write_str("Confirming workspace storage removal"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Operation cancelled; any existing worker remains allocated")]
    Cancelled,
    #[error("Provider transport failed; reconcile before retrying")]
    Transport,
    #[error("Provider returned HTTP {0}{reason}", reason = .1.suffix())]
    Http(u16, Reason),
    #[error("Provider authentication failed; check the machine-local credential")]
    Unauthorized,
    #[error("Provider rejected the request or capacity is unavailable{reason}", reason = .0.suffix())]
    Rejected(Reason),
    #[error("Provider response is invalid")]
    InvalidResponse,
    #[error(
        "No host with the requested GPU types in the allowed data centers has capacity with CUDA {0} or newer; choose other GPU types or data centers, try again later or lower min_cuda_version"
    )]
    CudaUnavailable(String),
    #[error("Worker creation is unresolved; no second allocation was attempted")]
    CreationUnresolved,
    #[error("Multiple workers match the operation; reconcile manually before continuing")]
    DuplicateWorkers,
    #[error("Worker identity does not match the persisted operation")]
    IdentityMismatch,
    #[error("An image replacement may change only the worker image and its registry credential")]
    IdentityChange,
    #[error("Worker no longer exists; its running processes cannot be recovered")]
    WorkerLost,
    #[error("Cannot persist worker operation before provider I/O")]
    Persistence,
}

fn optional_address<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<IpAddr>, D::Error> {
    let value: Option<String> = Deserialize::deserialize(deserializer)?;
    value
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse()
                .map_err(|_| serde::de::Error::custom("Invalid worker address"))
        })
        .transpose()
}

fn optional_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value: Option<serde_json::Value> = Deserialize::deserialize(deserializer)?;
    value
        .map(|value| match value {
            serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| serde::de::Error::custom("Invalid rate")),
            serde_json::Value::String(s) => s.parse().map_err(|_| serde::de::Error::custom("Invalid rate")),
            _ => Err(serde::de::Error::custom("Invalid rate")),
        })
        .transpose()
        .and_then(|rate: Option<f64>| {
            if rate.is_some_and(|v| !v.is_finite() || v < 0.0) {
                Err(serde::de::Error::custom("Invalid rate"))
            } else {
                Ok(rate)
            }
        })
}
