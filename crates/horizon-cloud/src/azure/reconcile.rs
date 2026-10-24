//! Read-only reconciliation by name. Each operation owns one resource group named
//! from its operation id, and the group's `horizon-operation` tag proves the
//! ownership. Reconciliation sends GET requests only: it never adopts, tags or
//! writes to a group it finds.
use super::{Azure, OPERATION_TAG, resource::Group, resource::VirtualMachine, resource_group_name};
use crate::{Cancellation, CloudError};

/// Every operation's resource group is this prefix followed by its operation id.
pub const RESOURCE_GROUP_PREFIX: &str = "horizon-ws-";
/// The one VM inside an operation's resource group; the group carries the identity.
pub const WORKER_VM_NAME: &str = "worker";

/// What ARM holds for one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// No resource group has the operation's name, so no VM of it can exist.
    Absent,
    /// The operation's group exists, without its VM.
    GroupOnly(Group),
    /// The operation's group and its VM exist. `vm.instance_id` is the identity of
    /// this instance.
    Created { group: Group, vm: Box<VirtualMachine> },
}

impl Azure {
    /// Finds the resources of `operation_id` by name. A group 404 is `Absent` and ends
    /// the reads, because a VM cannot outlive its group.
    /// # Errors
    /// `Invalid` for an operation id that cannot name a group (nothing is sent).
    /// `IdentityMismatch` when the group or VM answer names another resource, or the
    /// group's `horizon-operation` tag is missing or names another operation.
    /// Otherwise the mapped [`super::Failure`] of a failed read.
    pub fn reconcile(&self, operation_id: &str, cancel: &Cancellation) -> Result<Found, CloudError> {
        if !crate::valid_id(operation_id) {
            return Err(CloudError::Invalid("Invalid operation id"));
        }
        let name = resource_group_name(operation_id);
        let Some(group) = self.get_resource_group(&name, cancel)? else {
            return Ok(Found::Absent);
        };
        if group.tags.get(OPERATION_TAG).map(String::as_str) != Some(operation_id) {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(match self.get_vm(&name, WORKER_VM_NAME, cancel)? {
            None => Found::GroupOnly(group),
            Some(vm) => Found::Created {
                group,
                vm: Box::new(vm),
            },
        })
    }
}
