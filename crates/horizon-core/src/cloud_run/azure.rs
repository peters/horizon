//! Azure Linux VM worker foundations for #474: operator profile, exact worker
//! identity, lifecycle mapping, credential source, typed errors, the bounded Resource
//! Manager transport and the deployment plan. The provider is a separate slice;
//! nothing is wired into configuration or UI.
use super::{
    CloudJobId, CloudProvider, CloudWorkflowId, WorkerTarget,
    interactive_worker::{InteractiveWorkerLifetime, valid_worker_target},
    validation::valid_immutable_worker_image,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
mod credential;
pub mod deployment;
#[cfg(test)]
mod tests;
mod transport;

pub use credential::{AzureAccessToken, AzureCliCredential, AzureCredentialSource};
pub use deployment::AzureDeploymentPlan;
pub use transport::{
    AzureArmHttp, AzureDeploymentState, AzureGroupInfo, AzureLongRunningState, AzureManagementTransport, AzureVmView,
};

/// Public Azure Resource Manager endpoint; tokens are requested for this audience only.
pub const MANAGEMENT_ENDPOINT: &str = "https://management.azure.com";
/// Explicit ARM API versions; the transport never lets Azure pick a version.
pub const RESOURCE_GROUP_API_VERSION: &str = "2022-09-01";
pub const DEPLOYMENT_API_VERSION: &str = "2022-09-01";
pub const COMPUTE_API_VERSION: &str = "2024-03-01";
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const RESPONSE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
/// Every worker lives in its own resource group named from its exact identity.
pub const RESOURCE_GROUP_PREFIX: &str = "horizon-ws-";
/// Constant VM name inside a worker's resource group; the group carries the identity.
pub const WORKER_VM_NAME: &str = "worker";

/// Operator-declared placement for Azure CPU workers. Prices are declared, not
/// discovered: ARM does not return an hourly rate for a VM, so the cost limit in a
/// target is compared with this declared figure before any provider call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureProfile {
    pub name: String,
    /// Exact subscription UUID; never a display name and never the CLI default.
    pub subscription_id: String,
    /// Lowercase region name such as `northeurope`.
    pub location: String,
    /// Exact VM size from [`SUPPORTED_VM_SIZES`], such as `Standard_D4s_v3`.
    pub vm_size: String,
    /// Resource ID of the user-assigned identity that may pull the worker image. It must
    /// live in `subscription_id`: workers never borrow identities across subscriptions.
    pub image_pull_identity_id: String,
    /// Declared hourly price of `vm_size` in this region, in micro-units of the billing currency.
    pub declared_hourly_cost_micros: u64,
    /// Registry login server the image must belong to, such as `example.azurecr.io`.
    pub registry_login_server: String,
    /// Managed-disk SKU for the OS and data disks; every allowlisted size supports both.
    #[serde(default)]
    pub disk_sku: AzureDiskSku,
}

/// Managed-disk SKUs the adapter offers; the wire names are Azure's own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum AzureDiskSku {
    #[default]
    #[serde(rename = "StandardSSD_LRS")]
    StandardSsdLrs,
    #[serde(rename = "Premium_LRS")]
    PremiumLrs,
}

impl AzureDiskSku {
    /// Both offered SKUs are supported by every allowlisted size; kept as a hook for
    /// SKUs whose support varies (for example zone-redundant disks).
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::StandardSsdLrs | Self::PremiumLrs)
    }

    #[must_use]
    pub const fn as_azure_name(self) -> &'static str {
        match self {
            Self::StandardSsdLrs => "StandardSSD_LRS",
            Self::PremiumLrs => "Premium_LRS",
        }
    }
}

impl AzureProfile {
    /// Validate the profile shape before any provider call.
    /// # Errors
    pub fn validate(&self) -> Result<(), AzureError> {
        let valid = valid_profile_name(&self.name)
            && valid_subscription_id(&self.subscription_id)
            && valid_location(&self.location)
            && valid_vm_size(&self.vm_size)
            && valid_identity_id(&self.image_pull_identity_id, &self.subscription_id)
            && self.declared_hourly_cost_micros > 0
            && valid_registry_login_server(&self.registry_login_server)
            && self.disk_sku.is_supported();
        valid.then_some(()).ok_or(AzureError::InvalidProfile)
    }

    /// Reject a target that this profile cannot serve, before any provider I/O. The
    /// provider-neutral contract is applied first so this never accepts what it rejects.
    /// # Errors
    pub fn validate_target(&self, target: &WorkerTarget) -> Result<(), AzureError> {
        self.validate()?;
        let on_declared_registry = target
            .image
            .split_once('/')
            .is_some_and(|(server, _)| server == self.registry_login_server);
        let valid =
            valid_worker_target(target, CloudProvider::Azure) && target.profile == self.name && on_declared_registry;
        if !valid {
            return Err(AzureError::InvalidTarget);
        }
        match target.max_hourly_cost_micros {
            Some(maximum) if maximum < self.declared_hourly_cost_micros => Err(AzureError::DeclaredCostExceedsLimit {
                declared: self.declared_hourly_cost_micros,
                maximum,
            }),
            _ => Ok(()),
        }
    }
}

/// Deterministic, identity-derived resource group name for one worker.
#[must_use]
pub fn resource_group_name(workflow_id: CloudWorkflowId, job_id: CloudJobId) -> String {
    format!("{RESOURCE_GROUP_PREFIX}{workflow_id}-{job_id}")
}

/// Exact identity of one Azure worker: the resource group is the unit of ownership and
/// deletion, and its ARM resource ID is the provider handle. Decoding runs `validate`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AzureWorkerSnapshot")]
pub struct AzureWorker {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub subscription_id: String,
    pub resource_group: String,
    /// Group-level ARM resource ID, taken from Azure's creation response and never
    /// constructed from a name; the VM inside is always [`WORKER_VM_NAME`].
    pub group_id: String,
    pub image: String,
    pub lifetime: InteractiveWorkerLifetime,
}

impl AzureWorker {
    /// Validate a handle before any provider call. Only the persistent lifetime is
    /// accepted, matching the deployment plan; a well-formed lease is still refused.
    /// # Errors
    pub fn validate(&self) -> Result<(), AzureError> {
        let valid = valid_subscription_id(&self.subscription_id)
            && self.resource_group == resource_group_name(self.workflow_id, self.job_id)
            && valid_resource_group_id(&self.group_id, &self.subscription_id, &self.resource_group)
            && valid_immutable_worker_image(&self.image)
            && self.lifetime == InteractiveWorkerLifetime::Persistent;
        valid.then_some(()).ok_or(AzureError::InvalidPersistedWorker)
    }
}

/// Wire shape of [`AzureWorker`]; decoding runs [`AzureWorker::validate`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AzureWorkerSnapshot {
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    subscription_id: String,
    resource_group: String,
    group_id: String,
    image: String,
    lifetime: InteractiveWorkerLifetime,
}

impl TryFrom<AzureWorkerSnapshot> for AzureWorker {
    type Error = AzureError;

    fn try_from(value: AzureWorkerSnapshot) -> Result<Self, Self::Error> {
        let worker = Self {
            workflow_id: value.workflow_id,
            job_id: value.job_id,
            subscription_id: value.subscription_id,
            resource_group: value.resource_group,
            group_id: value.group_id,
            image: value.image,
            lifetime: value.lifetime,
        };
        worker.validate().map(|()| worker)
    }
}

/// Provider-side lifecycle observed from the VM provisioning and power states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AzureLifecycle {
    /// Being created, updated, started or deallocated.
    Transitioning,
    Running,
    /// Guest OS stopped but compute still allocated and billed; not a retained stop.
    StoppedAllocated,
    /// Compute released, disks retained; the only state that counts as stopped.
    Deallocated,
    Failed,
    Deleting,
    Unknown,
}

impl AzureLifecycle {
    /// Map ARM instance-view codes to the provider lifecycle.
    #[must_use]
    pub fn from_states(provisioning_state: Option<&str>, power_state: Option<&str>) -> Self {
        match (provisioning_state, power_state) {
            (Some("Deleting"), _) => Self::Deleting,
            (Some("Failed" | "Canceled"), _) => Self::Failed,
            (Some("Creating" | "Updating"), _)
            | (Some("Succeeded"), Some("PowerState/starting" | "PowerState/deallocating" | "PowerState/stopping")) => {
                Self::Transitioning
            }
            (Some("Succeeded"), Some("PowerState/running")) => Self::Running,
            (Some("Succeeded"), Some("PowerState/stopped")) => Self::StoppedAllocated,
            (Some("Succeeded"), Some("PowerState/deallocated")) => Self::Deallocated,
            _ => Self::Unknown,
        }
    }
}

/// Static-message errors; no Azure payload or identifier can leak through them.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AzureError {
    #[error("Azure profile is invalid")]
    InvalidProfile,
    #[error("Azure worker target does not fit the profile")]
    InvalidTarget,
    #[error("declared hourly cost {declared} exceeds the target limit {maximum}")]
    DeclaredCostExceedsLimit { declared: u64, maximum: u64 },
    #[error("persisted Azure worker identity is invalid")]
    InvalidPersistedWorker,
    #[error("Azure workers support only the persistent lifetime policy")]
    UnsupportedLifetime,
    #[error("Azure credential is unavailable: {reason}")]
    CredentialUnavailable { reason: &'static str },
    #[error("Azure request failed during {operation}")]
    RequestFailed { operation: &'static str },
    #[error("Azure returned HTTP {status} during {operation}")]
    UnexpectedStatus { operation: &'static str, status: u16 },
    #[error("Azure returned a malformed response during {operation}")]
    InvalidResponse { operation: &'static str },
    #[error("Azure returned an invalid or mismatched resource identity")]
    ResourceIdentityMismatch,
}

/// Parsed `/subscriptions/{sub}/resourceGroups/{rg}/providers/{provider}/{kind}/{name}`.
struct ResourceIdParts<'a> {
    group: &'a str,
    provider: &'a str,
    kind: &'a str,
    name: &'a str,
}

pub(crate) fn valid_subscription_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

pub(crate) fn valid_location(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Exact size names, including constrained-vCPU sizes such as `Standard_E4-2s_v3`.
/// Exact VM sizes the adapter accepts: x64, Hyper-V generation 2 and premium-storage
/// capable general-purpose, memory-optimised, compute-optimised and burstable sizes.
/// Matched by equality, so no nonexistent family, version or vCPU combination can
/// reach a paid call. `Standard_D4s_v3` is the live-validated candidate; extend the
/// table only after a live check of the new size.
pub const SUPPORTED_VM_SIZES: &[&str] = &[
    "Standard_D2s_v3",
    "Standard_D4s_v3",
    "Standard_D8s_v3",
    "Standard_D16s_v3",
    "Standard_D2s_v5",
    "Standard_D4s_v5",
    "Standard_D8s_v5",
    "Standard_D16s_v5",
    "Standard_D2as_v5",
    "Standard_D4as_v5",
    "Standard_D8as_v5",
    "Standard_D16as_v5",
    "Standard_E2s_v5",
    "Standard_E4s_v5",
    "Standard_E8s_v5",
    "Standard_E16s_v5",
    "Standard_E4-2s_v5",
    "Standard_E8-4s_v5",
    "Standard_E16-8s_v5",
    "Standard_F2s_v2",
    "Standard_F4s_v2",
    "Standard_F8s_v2",
    "Standard_F16s_v2",
    "Standard_B2s",
    "Standard_B2ms",
    "Standard_B4ms",
    "Standard_B8ms",
];

pub(crate) fn valid_vm_size(value: &str) -> bool {
    SUPPORTED_VM_SIZES.contains(&value)
}

fn valid_profile_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 191 && value.trim() == value && !value.chars().any(char::is_control)
}

pub(crate) fn valid_resource_group_name(value: &str) -> bool {
    (1..=90).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'(' | b')'))
        && !value.ends_with('.')
}

/// Leaf resource names as ARM accepts them for compute and identity resources:
/// 1 to 128 characters of alphanumerics, hyphens and underscores.
pub(crate) fn valid_resource_name(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// User-assigned identity names are 3 to 128 characters and start alphanumeric.
fn valid_identity_name(value: &str) -> bool {
    valid_resource_name(value) && value.len() >= 3 && value.as_bytes()[0].is_ascii_alphanumeric()
}

pub(crate) fn valid_identity_id(value: &str, subscription_id: &str) -> bool {
    resource_id_parts(value, subscription_id).is_some_and(|parts| {
        parts.provider.eq_ignore_ascii_case("Microsoft.ManagedIdentity")
            && parts.kind.eq_ignore_ascii_case("userAssignedIdentities")
            && valid_identity_name(parts.name)
    })
}

/// `/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Compute/virtualMachines/{name}`
/// for exactly the requested subscription, group and VM name.
pub(crate) fn valid_vm_resource_id(value: &str, subscription_id: &str, resource_group: &str, name: &str) -> bool {
    resource_id_parts(value, subscription_id).is_some_and(|parts| {
        parts.group.eq_ignore_ascii_case(resource_group)
            && parts.provider.eq_ignore_ascii_case("Microsoft.Compute")
            && parts.kind.eq_ignore_ascii_case("virtualMachines")
            && parts.name == name
    })
}

/// `/subscriptions/{sub}/resourceGroups/{rg}` for exactly the requested subscription and group.
pub(crate) fn valid_resource_group_id(value: &str, subscription_id: &str, resource_group: &str) -> bool {
    if value.len() > 512 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let mut parts = value.strip_prefix('/').unwrap_or_default().split('/');
    parts.next() == Some("subscriptions")
        && parts
            .next()
            .is_some_and(|sub| sub.eq_ignore_ascii_case(subscription_id))
        && parts
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("resourceGroups"))
        && parts
            .next()
            .is_some_and(|group| group.eq_ignore_ascii_case(resource_group))
        && parts.next().is_none()
}

/// Subscription and group segments compare case-insensitively, as ARM does.
fn resource_id_parts<'a>(value: &'a str, subscription_id: &str) -> Option<ResourceIdParts<'a>> {
    if value.len() > 512 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let mut parts = value.strip_prefix('/')?.split('/');
    (parts.next()? == "subscriptions" && parts.next()?.eq_ignore_ascii_case(subscription_id)).then_some(())?;
    parts.next()?.eq_ignore_ascii_case("resourceGroups").then_some(())?;
    let group = parts.next().filter(|group| valid_resource_group_name(group))?;
    (parts.next()? == "providers").then_some(())?;
    let provider = parts.next()?;
    let kind = parts.next()?;
    let name = parts.next().filter(|name| valid_resource_name(name))?;
    parts.next().is_none().then_some(ResourceIdParts {
        group,
        provider,
        kind,
        name,
    })
}

/// `<registry>.azurecr.io` where the registry name is 5 to 50 lowercase alphanumerics.
fn valid_registry_login_server(value: &str) -> bool {
    value.strip_suffix(".azurecr.io").is_some_and(|registry| {
        (5..=50).contains(&registry.len()) && registry.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}
