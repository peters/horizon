use super::super::service_admission::OwnerScope;
use crate::{cloud_panel::CloudLaunch, cloud_runtime::allocation::legacy::unique_json::UniqueValue};
use horizon_cloud::WorkerSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
const INVALID: &str = "Invalid retained exposure binding";

/// Exact non-secret facts captured by the original desktop controller. These
/// values, including pinned SSH options, are comparison data, never a connection factory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "BindingFields")]
pub struct Binding {
    pub owner: OwnerScope,
    pub launch: CloudLaunch,
    pub spec: WorkerSpec,
    pub worker_id: String,
    pub pinned_ssh_args: Vec<String>,
    pub provider_credential: ProviderBinding,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingFields {
    owner: OwnerScope,
    launch: UniqueValue,
    spec: UniqueValue,
    worker_id: String,
    pinned_ssh_args: Vec<String>,
    provider_credential: ProviderBinding,
}

impl TryFrom<BindingFields> for Binding {
    type Error = &'static str;
    fn try_from(fields: BindingFields) -> Result<Self, Self::Error> {
        let launch: CloudLaunch = serde_json::from_value(fields.launch.0.clone()).map_err(|_| INVALID)?;
        let spec: WorkerSpec = serde_json::from_value(fields.spec.0.clone()).map_err(|_| INVALID)?;
        // Store snapshots exactly as this writer encodes them. Nested launch/spec
        // decoders must not silently erase a newer field, default or image choice.
        if serde_json::to_value(&launch).map_err(|_| INVALID)? != fields.launch.0
            || serde_json::to_value(&spec).map_err(|_| INVALID)? != fields.spec.0
        {
            return Err(INVALID);
        }
        let binding = Self {
            owner: fields.owner,
            launch,
            spec,
            worker_id: fields.worker_id,
            pinned_ssh_args: fields.pinned_ssh_args,
            provider_credential: fields.provider_credential,
        };
        if binding.valid() { Ok(binding) } else { Err(INVALID) }
    }
}

impl Binding {
    pub(super) fn valid(&self) -> bool {
        OwnerScope::new(
            &self.owner.host_instance,
            &self.owner.session_id.to_string(),
            &self.owner.workspace_id,
            &self.owner.cloud_id,
        )
        .is_ok_and(|owner| owner == self.owner)
            && self.launch.id == self.owner.cloud_id
            && opaque(&self.worker_id)
            && opaque(&self.spec.operation_id)
            && immutable(&self.spec.image_digest)
            && self.provider_credential.valid(&self.spec.profile.provider)
            && !self.pinned_ssh_args.is_empty()
            && self.pinned_ssh_args.len() <= 96
            && self.pinned_ssh_args.iter().all(|arg| text(arg, 4_096))
            && self
                .pinned_ssh_args
                .iter()
                .any(|arg| arg == "StrictHostKeyChecking=yes")
            && serde_json::to_value(&self.launch)
                .and_then(serde_json::from_value::<CloudLaunch>)
                .is_ok_and(|launch| launch == self.launch)
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 65_536)
    }
}

fn text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn opaque(value: &str) -> bool {
    text(value, 256) && value.bytes().all(|b| b.is_ascii_graphic())
}
fn immutable(value: &str) -> bool {
    value.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
        text(name, 256)
            && digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Exact provider comparison binding. Legacy API-file strings remain readable;
/// Docker has no API credential and records its full actual configured host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum ProviderBinding {
    ApiFile(PathBuf),
    DockerHost {
        docker_host: horizon_cloud::docker_host::Host,
    },
}
impl From<PathBuf> for ProviderBinding {
    fn from(path: PathBuf) -> Self {
        Self::ApiFile(path)
    }
}
impl ProviderBinding {
    fn valid(&self, provider: &str) -> bool {
        match self {
            Self::ApiFile(path) => path.is_absolute() && path.to_str().is_some_and(|path| text(path, 4096)),
            Self::DockerHost { docker_host } => {
                provider == horizon_cloud::docker_host::PROVIDER && docker_host.validate().is_ok()
            }
        }
    }
}
