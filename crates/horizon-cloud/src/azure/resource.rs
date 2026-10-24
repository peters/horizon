//! Small typed views of the ARM resources this client reads. A response body is
//! untrusted: only the fields below are kept, and an identity field that does not
//! name the requested resource is an `IdentityMismatch`, never a partial view.
use super::{
    Failure,
    operation::LongRunning,
    url::{resource_id_parts, valid_instance_id, valid_resource_group_id, valid_vm_resource_id},
};
use crate::CloudError;
use serde_json::Value;
use std::collections::BTreeMap;

/// A resource group as ARM reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// The group's resource id, checked against the requested subscription and group.
    pub id: String,
    pub name: String,
    pub location: String,
    pub provisioning_state: String,
    /// String tags only; a tag with another JSON type is dropped.
    pub tags: BTreeMap<String, String>,
}

/// The fields of a VM and its instance view that a lifecycle needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualMachine {
    /// The VM's resource id, checked against the requested subscription, group and name.
    pub id: String,
    /// `properties.vmId`, the identity of this instance. A VM that is deleted and made
    /// again under the same name keeps its resource id but gets a new `vmId`.
    pub instance_id: Option<String>,
    pub name: String,
    pub location: String,
    pub vm_size: String,
    /// `storageProfile.osDisk.diskSizeGB`, once ARM reports it.
    pub os_disk_gb: Option<u32>,
    pub provisioning_state: String,
    /// The `PowerState/...` code of the instance view, such as `PowerState/running`.
    pub power_state: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The managed data disks of the storage profile that carry a disk id.
    pub data_disks: Vec<DataDisk>,
    /// Every data disk entry of the storage profile. A count above `data_disks.len()`
    /// means storage that this view cannot name.
    pub data_disk_count: usize,
}

/// One managed data disk attached to a VM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataDisk {
    pub id: String,
    pub lun: Option<u64>,
    pub size_gb: Option<u32>,
    /// `Detach` keeps the disk when the VM is deleted; `Delete` removes it with the VM.
    pub delete_option: String,
}

/// A template deployment's provisioning state and its string outputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeploymentState {
    pub provisioning_state: String,
    pub outputs: BTreeMap<String, String>,
}

impl DeploymentState {
    /// The deployment ended, successfully or not.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.provisioning_state.as_str(), "Succeeded" | "Failed" | "Canceled")
    }
}

/// A deployment that ARM accepted, with the operation that reports its end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deployed {
    pub state: DeploymentState,
    pub operation: LongRunning,
}

pub(crate) fn text(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn tags(value: &Value) -> BTreeMap<String, String> {
    value
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// A group body for `expected` in `subscription_id`. A missing name or id is an
/// invalid response; a name or id of another group is an identity mismatch.
pub(crate) fn group_info(value: &Value, subscription_id: &str, expected: &str) -> Result<Group, Failure> {
    let name = text(value, "/name").ok_or_else(invalid)?;
    let id = text(value, "/id").ok_or_else(invalid)?;
    if !name.eq_ignore_ascii_case(expected) || !valid_resource_group_id(&id, subscription_id, expected) {
        return Err(mismatch());
    }
    Ok(Group {
        id,
        name,
        location: text(value, "/location").unwrap_or_default(),
        provisioning_state: text(value, "/properties/provisioningState").unwrap_or_default(),
        tags: tags(value),
    })
}

/// A deployment body for `name` in `group` of `subscription_id`.
pub(crate) fn deployment_state(
    value: &Value,
    subscription_id: &str,
    group: &str,
    name: &str,
) -> Result<DeploymentState, Failure> {
    let id = text(value, "/id").ok_or_else(invalid)?;
    let observed = text(value, "/name").ok_or_else(invalid)?;
    let owned = resource_id_parts(&id, subscription_id).is_some_and(|parts| {
        parts.group.eq_ignore_ascii_case(group)
            && parts.provider.eq_ignore_ascii_case("Microsoft.Resources")
            && parts.kind.eq_ignore_ascii_case("deployments")
            && parts.name == name
    });
    if !observed.eq_ignore_ascii_case(name) || !owned {
        return Err(mismatch());
    }
    let provisioning_state = text(value, "/properties/provisioningState").ok_or_else(invalid)?;
    let outputs = value
        .pointer("/properties/outputs")
        .and_then(Value::as_object)
        .map(|outputs| {
            outputs
                .iter()
                .filter_map(|(key, output)| text(output, "/value").map(|value| (key.clone(), value)))
                .collect()
        })
        .unwrap_or_default();
    Ok(DeploymentState {
        provisioning_state,
        outputs,
    })
}

/// A VM body for `name` in `group` of `subscription_id`. A `vmId` that is present but
/// not a lowercase UUID is an identity mismatch: it cannot pin this instance.
pub(crate) fn vm_view(
    value: &Value,
    subscription_id: &str,
    group: &str,
    name: &str,
) -> Result<VirtualMachine, Failure> {
    let id = text(value, "/id").ok_or_else(invalid)?;
    let observed = text(value, "/name").ok_or_else(invalid)?;
    if !observed.eq_ignore_ascii_case(name) || !valid_vm_resource_id(&id, subscription_id, group, name) {
        return Err(mismatch());
    }
    let instance_id = match value.pointer("/properties/vmId") {
        None => None,
        Some(Value::String(instance)) if valid_instance_id(instance) => Some(instance.clone()),
        Some(_) => return Err(mismatch()),
    };
    let disks: &[Value] = value
        .pointer("/properties/storageProfile/dataDisks")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let power_state = value
        .pointer("/properties/instanceView/statuses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|status| text(status, "/code"))
        .find(|code| code.starts_with("PowerState/"));
    Ok(VirtualMachine {
        id,
        instance_id,
        name: observed,
        location: text(value, "/location").unwrap_or_default(),
        vm_size: text(value, "/properties/hardwareProfile/vmSize").unwrap_or_default(),
        os_disk_gb: gb(value, "/properties/storageProfile/osDisk/diskSizeGB"),
        provisioning_state: text(value, "/properties/provisioningState").unwrap_or_default(),
        power_state,
        tags: tags(value),
        data_disks: disks
            .iter()
            .filter_map(|disk| {
                Some(DataDisk {
                    id: text(disk, "/managedDisk/id")?,
                    lun: disk.pointer("/lun").and_then(Value::as_u64),
                    size_gb: gb(disk, "/diskSizeGB"),
                    delete_option: text(disk, "/deleteOption").unwrap_or_default(),
                })
            })
            .collect(),
        data_disk_count: disks.len(),
    })
}

fn gb(value: &Value, pointer: &str) -> Option<u32> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|gb| u32::try_from(gb).ok())
}

fn invalid() -> Failure {
    Failure::Local(CloudError::InvalidResponse)
}

fn mismatch() -> Failure {
    Failure::Local(CloudError::IdentityMismatch)
}
