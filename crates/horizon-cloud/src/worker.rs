//! Portable worker lifecycle. The caller durably persists `CreateState` before I/O.
use crate::{Profile, valid_id, valid_image};
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
}
impl WorkerSpec {
    #[must_use]
    pub fn name(&self) -> String {
        format!("horizon-cloud-{}", self.operation_id)
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
        if !self.profile.gpu
            && self.cpu_flavors.iter().any(|flavor| {
                let per_cpu = match flavor.as_str() {
                    "cpu3c" | "cpu5c" => 2,
                    "cpu3g" | "cpu5g" => 4,
                    "cpu3m" | "cpu5m" => 8,
                    _ => 0,
                };
                u32::from(self.profile.cpu) * per_cpu < u32::from(self.profile.memory_gb)
            })
        {
            return Err(CloudError::Invalid(
                "Selected CPU flavors cannot meet the profile memory requirement",
            ));
        }
        Ok(())
    }
}
fn valid_public_key(value: &str) -> bool {
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
    #[serde(default)]
    pub memory_in_gb: Option<u32>,
    #[serde(default)]
    pub vcpu_count: Option<u32>,
    #[serde(default)]
    pub gpu_count: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
impl Worker {
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
        Ok(())
    }
    /// # Errors
    /// Inspect actual assigned resources separately after persisting the worker identity.
    pub fn verify_resources(&self, spec: &WorkerSpec) -> Result<(), CloudError> {
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
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Reconciling,
    Requesting,
    WorkerFound(String),
    Terminating,
    Terminated,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Operation cancelled; any existing worker remains allocated")]
    Cancelled,
    #[error("Provider transport failed; reconcile before retrying")]
    Transport,
    #[error("Provider returned HTTP {0}")]
    Http(u16),
    #[error("Provider authentication failed; check the machine-local credential")]
    Unauthorized,
    #[error("Provider rejected the request or capacity is unavailable")]
    Rejected,
    #[error("Provider response is invalid")]
    InvalidResponse,
    #[error("Worker creation is unresolved; no second allocation was attempted")]
    CreationUnresolved,
    #[error("Multiple workers match the operation; reconcile manually before continuing")]
    DuplicateWorkers,
    #[error("Worker identity does not match the persisted operation")]
    IdentityMismatch,
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
