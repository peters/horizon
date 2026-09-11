//! Azure Linux VM worker foundations for #474: operator profile, lifecycle mapping,
//! credential source, typed errors and the bounded Resource Manager transport. The
//! worker identity and the provider are separate slices; nothing is wired into
//! configuration or UI.
use super::{CloudProvider, WorkerTarget, interactive_worker::valid_worker_target};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
mod credential;
#[cfg(test)]
mod tests;
mod transport;

pub use credential::{AzureAccessToken, AzureCliCredential, AzureCredentialSource};
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
    /// Exact VM size such as `Standard_D4s_v3`.
    pub vm_size: String,
    /// Resource ID of the user-assigned identity that may pull the worker image. It must
    /// live in `subscription_id`: workers never borrow identities across subscriptions.
    pub image_pull_identity_id: String,
    /// Declared hourly price of `vm_size` in this region, in micro-units of the billing currency.
    pub declared_hourly_cost_micros: u64,
    /// Registry login server the image must belong to, such as `example.azurecr.io`.
    pub registry_login_server: String,
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
            && valid_registry_login_server(&self.registry_login_server);
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
pub(crate) fn valid_vm_size(value: &str) -> bool {
    value.len() <= 48
        && value.strip_prefix("Standard_").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
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
