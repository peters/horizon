//! The release of an operation's VM for a rebuild on a new VM. Azure's stop
//! deallocates the VM and keeps it, so a rebuild cannot release the VM as a stop
//! does, and a delete removes the whole group. This release deletes only the VM:
//! the template gives its OS disk `deleteOption: Delete` and its workspace disk
//! `Detach`, so ARM removes the OS disk with the VM and keeps the workspace disk,
//! the network interface, the static address, the network and the security group.
//! The next deployment of the same template makes a new VM that attaches the same
//! workspace disk on LUN 0.
//!
//! The release is proven by what ARM reports afterwards: the VM instance and the OS
//! disk are gone, and the workspace disk is present, carries the operation's tag and
//! is attached to nothing. A delete is sent only while the VM is present and not
//! already being deleted, so a lost answer is reconciled by reading the VM again.
use super::{
    ACTION_TIMEOUT, Azure, BACKOFF_MS, COMPUTE_API_VERSION, Failure, LongRunning, Method, OPERATION_TAG,
    WORKER_VM_NAME,
    operation::{operation_ref, remaining_at},
    provision::holds_workspace,
    resource::{VirtualMachine, tags, text},
    resource_group_name,
    template::{DATA_DISK_NAME, DISK_API_VERSION, OS_DISK_NAME},
    url::{resource_id_parts, valid_resource_group_name},
};
use crate::{Cancellation, CloudError};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// The fields of a managed disk that the release proves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    /// The resource id of the VM that holds the disk, or `None` when it is attached to nothing.
    pub managed_by: Option<String>,
    pub tags: BTreeMap<String, String>,
}

impl Azure {
    /// Asks ARM to delete VM `name` in `group`. A 404 means the VM is already gone.
    /// # Errors
    /// `Invalid` for a bad segment (nothing is sent), `InvalidResponse` for an
    /// operation URL outside this subscription, or the provider failure.
    pub fn delete_vm(&self, group: &str, name: &str, cancel: &Cancellation) -> Result<LongRunning, Failure> {
        let url = self.resource_url(group, "Microsoft.Compute/virtualMachines", name, COMPUTE_API_VERSION)?;
        match self.send_within(Method::Delete, &url, None, cancel, None) {
            Ok(reply) => match operation_ref(&reply.headers) {
                Some(operation) if self.owns_operation_url(operation.url()) => Ok(LongRunning::Accepted(operation)),
                Some(_) => Err(Failure::Local(CloudError::InvalidResponse)),
                None => Ok(LongRunning::Completed),
            },
            Err(failure) if failure.not_found() => Ok(LongRunning::Completed),
            Err(failure) => Err(failure),
        }
    }

    /// Reads managed disk `name` in `group`, or `None` when ARM has no such disk.
    /// # Errors
    /// `Invalid` for a bad segment (nothing is sent), `IdentityMismatch` for a body
    /// that names another disk, `InvalidResponse` for an unusable body, or the
    /// provider failure.
    pub fn get_disk(&self, group: &str, name: &str, cancel: &Cancellation) -> Result<Option<Disk>, Failure> {
        let url = self.resource_url(group, "Microsoft.Compute/disks", name, DISK_API_VERSION)?;
        let Some(value) = self.get_json_within(&url, cancel, None)? else {
            return Ok(None);
        };
        let id = text(&value, "/id").ok_or(Failure::Local(CloudError::InvalidResponse))?;
        let owned = resource_id_parts(&id, &self.subscription_id).is_some_and(|parts| {
            parts.group.eq_ignore_ascii_case(group)
                && parts.provider.eq_ignore_ascii_case("Microsoft.Compute")
                && parts.kind.eq_ignore_ascii_case("disks")
                && parts.name.eq_ignore_ascii_case(name)
        });
        if !owned {
            return Err(Failure::Local(CloudError::IdentityMismatch));
        }
        Ok(Some(Disk {
            managed_by: text(&value, "/managedBy").filter(|holder| !holder.is_empty()),
            tags: tags(&value),
        }))
    }
}

/// Whether deleting `vm` keeps its workspace disk: the VM holds exactly the group's
/// workspace disk, and ARM detaches it rather than deletes it with the VM.
#[must_use]
pub fn keeps_workspace(vm: &VirtualMachine, subscription_id: &str, group: &str) -> bool {
    holds_workspace(vm, subscription_id, group)
        && vm
            .data_disks
            .iter()
            .all(|disk| disk.delete_option.eq_ignore_ascii_case("Detach"))
}

/// Deletes VM `instance` of `operation_id` and keeps its workspace disk, then waits
/// until ARM reports the VM and its OS disk gone and the workspace disk free.
/// `announce` is called once, just before the delete request, so the caller records
/// the release only for a VM that this call deletes; a VM it refuses stays untouched.
/// # Errors
/// `Invalid` for an id that cannot name a group (nothing is sent),
/// `IdentityMismatch` for a VM of another instance under the name, which is never
/// deleted, and for a VM or disk without the operation's tag, `Invalid` for a
/// VM whose delete would take its workspace disk with it (nothing is sent), for a
/// workspace disk that is missing, and while ARM still deletes at `ACTION_TIMEOUT`,
/// and the provider and announcement failures. Each leaves the release to a retry.
pub fn release(
    client: &Azure,
    operation_id: &str,
    instance: &str,
    cancel: &Cancellation,
    announce: &mut dyn FnMut() -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    let group = resource_group_name(operation_id);
    if !crate::valid_id(operation_id) || !valid_resource_group_name(&group) {
        return Err(CloudError::Invalid("Invalid operation id"));
    }
    let started = Instant::now();
    let deadline = started + ACTION_TIMEOUT;
    let mut waited = Duration::ZERO;
    let mut sent = false;
    let steps = BACKOFF_MS
        .iter()
        .copied()
        .chain(BACKOFF_MS.last().copied().into_iter().cycle());
    for step in steps {
        if let Some(vm) = client.get_vm(&group, WORKER_VM_NAME, cancel)? {
            // Nothing makes a VM while the release runs, so another instance under the
            // name is not the cloud's; it would hold the workspace disk.
            if vm.instance_id.as_deref() != Some(instance) {
                return Err(CloudError::IdentityMismatch);
            }
            if vm.tags.get(OPERATION_TAG).map(String::as_str) != Some(operation_id) {
                return Err(CloudError::IdentityMismatch);
            }
            if !keeps_workspace(&vm, client.subscription_id(), &group) {
                return Err(CloudError::Invalid(
                    "The Azure VM would delete its workspace disk with it, so it is not released; examine the VM's data disk in the Azure portal",
                ));
            }
            if !sent && !vm.provisioning_state.eq_ignore_ascii_case("Deleting") {
                announce()?;
                if let LongRunning::Accepted(operation) = client.delete_vm(&group, WORKER_VM_NAME, cancel)? {
                    client.await_operation(&operation, cancel)?;
                }
                sent = true;
                continue;
            }
        } else if kept(client, operation_id, &group, cancel)? {
            return Ok(());
        }
        let Some(left) = remaining_at(Instant::now().max(started + waited), deadline) else {
            break;
        };
        let delay = Duration::from_millis(step).min(left);
        client.wait(delay, cancel)?;
        waited += delay;
    }
    Err(CloudError::Invalid(
        "Azure is still deleting the cloud's VM; continue the rebuild later to finish its release",
    ))
}

/// Whether the released VM's OS disk is gone and the workspace disk is free for a
/// new VM. A missing workspace disk is never made again empty: it is refused.
fn kept(client: &Azure, operation_id: &str, group: &str, cancel: &Cancellation) -> Result<bool, CloudError> {
    if client.get_disk(group, OS_DISK_NAME, cancel)?.is_some() {
        return Ok(false);
    }
    let workspace = client
        .get_disk(group, DATA_DISK_NAME, cancel)?
        .ok_or(CloudError::Invalid(
            "The cloud's workspace disk is missing on Azure, so no new VM is started on an empty one; delete the cloud",
        ))?;
    if workspace.tags.get(OPERATION_TAG).map(String::as_str) != Some(operation_id) {
        return Err(CloudError::IdentityMismatch);
    }
    Ok(workspace.managed_by.is_none())
}
