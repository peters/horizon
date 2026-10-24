//! Creates an operation's worker: its resource group, then the one deployment that
//! makes everything in it. Each create is fenced by a durable record saved before
//! the request: the group's fence lives in the caller's [`Journal`], the worker's in
//! the caller's operation record. A create whose answer was lost is reconciled by
//! name and the `horizon-operation` tag and is never sent again; only an answer
//! that proves ARM made nothing, or a deployment that ARM reports ended without a
//! VM, opens the fence for another create.
use super::{
    Azure, LongRunning, OPERATION_TAG, WORKER_VM_NAME,
    catalog::{VM_SIZES, VmSize},
    resource::{Group, VirtualMachine},
    template::{self, ADDRESS_OUTPUT, DATA_DISK_NAME, DEPLOYMENT_NAME, Planned},
    url::resource_id_parts,
};
use crate::{Cancellation, CloudError, CreateState, Reason, Worker, WorkerSpec, host};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// The status of a VM that is powered off but still allocated, and so still billed.
pub const STOPPED_ALLOCATED: &str = "STOPPED_ALLOCATED";

/// What a cloud owns on Azure besides its VM, whose fence is the caller's operation
/// record. Every change must be durable before the request it guards.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    /// The region of the resource group, saved with its fence; it fixes where the worker runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The fence for the resource group. A bound group records its resource id, which
    /// a deleted group keeps as `Terminated` once ARM answers 404 for it.
    #[serde(default = "prepared")]
    pub group: CreateState,
    /// Saved before the first delete request and never cleared, so nothing creates
    /// in a group that may be going away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleting: bool,
    /// The `horizon-operation` tag sweep after the group's 404 listed nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub swept: bool,
}

fn prepared() -> CreateState {
    CreateState::Prepared
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            region: None,
            group: CreateState::Prepared,
            deleting: false,
            swept: false,
        }
    }
}

impl Journal {
    /// Whether anything the operation made may exist on Azure: a requested or bound
    /// group, or a deleted one whose tag sweep has not yet listed nothing.
    #[must_use]
    pub fn retains(&self) -> bool {
        match self.group {
            CreateState::Prepared => false,
            CreateState::Requested | CreateState::Bound { .. } => true,
            CreateState::Terminated { .. } => !self.swept,
        }
    }

    /// Whether this journal proves that `operation_id` has nothing left on Azure: its
    /// deletion started, ARM answered 404 for the operation's own group in a
    /// subscription, and the tag sweep after it listed nothing.
    #[must_use]
    pub fn proves_absent(&self, operation_id: &str) -> bool {
        let CreateState::Terminated { worker_id } = &self.group else {
            return false;
        };
        let subscription = worker_id.split('/').nth(2).unwrap_or_default();
        self.deleting
            && self.swept
            && crate::valid_id(operation_id)
            && super::url::valid_subscription_id(subscription)
            && super::url::valid_resource_group_id(worker_id, subscription, &super::resource_group_name(operation_id))
    }
}

/// The VM sizes, in the order they are tried, and the regions a cloud may use
/// under the current settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub sizes: Vec<String>,
    pub regions: Vec<String>,
}

/// The records a caller keeps for a cloud. Each call returns only once the record is
/// durable, because the provider request it guards follows at once.
pub trait Records {
    /// Called at a durable request boundary, just before a provider mutation.
    /// # Errors
    /// A failure prevents the mutation.
    fn before_mutation(&mut self) -> Result<(), CloudError> {
        Ok(())
    }

    /// Called when the request after the last boundary was refused outright, so it
    /// changed nothing.
    fn mutation_refused(&mut self) {}

    /// # Errors
    /// Reports a record that could not be made durable.
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError>;

    /// The fence for the cloud's VM.
    /// # Errors
    /// Reports a record that could not be made durable.
    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError>;
}

/// What a worker is provisioned from.
pub struct Request<'a> {
    pub spec: &'a WorkerSpec,
    pub policy: &'a Policy,
    /// The pull login for a private image. A requested or bound worker is only
    /// reconciled, so its custom data is never sent again.
    pub login: Option<host::RegistryLogin>,
}

/// Provisions the cloud's worker, or reconnects to the one it has, and returns it
/// once the VM is the operation's, in its region, of an allowed size and holding its
/// workspace disk. Everything is checked and the bootstrap rendered before the first
/// request; every fence is saved before the create it guards.
/// # Errors
/// Refuses invalid requests before any request. `CreationUnresolved` when a create's
/// answer was lost and nothing proves what ARM did; `WorkerLost` when the bound VM is
/// gone; `IdentityMismatch` for a group or VM that is not the operation's; and the
/// provider, persistence and verification failures.
pub fn provision(
    client: &Azure,
    request: Request<'_>,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<Worker, CloudError> {
    let Request { spec, policy, login } = request;
    check_login(login.as_ref(), &spec.image_digest)?;
    let sizes = template::fitting_sizes(&policy.sizes, spec)?;
    let region = match &journal.region {
        Some(region) if policy.regions.contains(region) => region.clone(),
        Some(_) => return Err(CloudError::Invalid("The cloud's Azure region is no longer allowed")),
        None => policy
            .regions
            .first()
            .cloned()
            .ok_or(CloudError::Invalid("Add an Azure region to the settings"))?,
    };
    let first = *sizes
        .first()
        .ok_or(CloudError::Invalid("No allowed Azure VM size fits this worker"))?;
    let mut planned = Planned::new(spec, &region, first, login)?;
    if journal.deleting {
        return Err(CloudError::Invalid(
            "The cloud is being deleted; finish deleting it before deploying",
        ));
    }
    if matches!(operation, CreateState::Terminated { .. }) || matches!(journal.group, CreateState::Terminated { .. }) {
        return Err(CloudError::WorkerLost);
    }
    let created_group = ensure_group(client, &spec.operation_id, &planned, journal, records, cancel)?;
    let vm = match operation.clone() {
        CreateState::Bound { worker_id } => inspect(client, &planned.group, &worker_id, cancel)?,
        CreateState::Requested => reconcile(client, &planned.group, operation, records, cancel)?,
        CreateState::Prepared => create(client, &mut planned, &sizes, created_group, operation, records, cancel)?,
        CreateState::Terminated { .. } => return Err(CloudError::WorkerLost),
    };
    settle(client, spec, policy, &planned.region, &vm, cancel)
}

/// The bound worker as ARM reports it now, with the same checks as [`provision`],
/// for a caller that waits for it to run. Sends GET requests only.
/// # Errors
/// `WorkerLost` when the VM is gone or is another instance, and the failures of
/// [`provision`]'s checks.
pub fn inspect_worker(
    client: &Azure,
    spec: &WorkerSpec,
    policy: &Policy,
    region: &str,
    instance: &str,
    cancel: &Cancellation,
) -> Result<Worker, CloudError> {
    let vm = inspect(
        client,
        &super::resource_group_name(&spec.operation_id),
        instance,
        cancel,
    )?;
    settle(client, spec, policy, region, &vm, cancel)
}

/// Refuses a pull login for another registry than the image's, which would only
/// fail the host's pull once the VM is billed, and a Docker Hub spelling whose
/// login the host would store under another key.
/// # Errors
/// `Invalid` for either.
pub fn check_login(login: Option<&host::RegistryLogin>, image: &str) -> Result<(), CloudError> {
    use crate::hetzner::cloud::{image_registry, registry_host};
    match login {
        Some(_) if image.to_ascii_lowercase().starts_with("registry-1.docker.io/") => Err(CloudError::Invalid(
            "Name a private Docker Hub image docker.io/... rather than registry-1.docker.io/... for Azure",
        )),
        Some(login) if registry_host(&login.server) != image_registry(image) => Err(CloudError::Invalid(
            "The Azure pull login names a different registry than the profile's image",
        )),
        _ => Ok(()),
    }
}

/// Binds the operation's resource group, creating it only while its fence is
/// prepared and no group has its name. Returns whether this call created it, so
/// it is known to be empty.
fn ensure_group(
    client: &Azure,
    operation_id: &str,
    planned: &Planned,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<bool, CloudError> {
    match journal.group {
        CreateState::Bound { .. } => return Ok(false),
        CreateState::Requested => {
            let group = client
                .get_resource_group(&planned.group, cancel)?
                .ok_or(CloudError::CreationUnresolved)?;
            bind_group(&group, operation_id, &planned.region, journal, records)?;
            return Ok(false);
        }
        CreateState::Prepared => {}
        CreateState::Terminated { .. } => return Err(CloudError::WorkerLost),
    }
    // A group found by name was made by an earlier attempt whose record was lost, or
    // belongs to someone else; a PUT would replace its tags, so it is never written.
    if let Some(group) = client.get_resource_group(&planned.group, cancel)? {
        bind_group(&group, operation_id, &planned.region, journal, records)?;
        return Ok(false);
    }
    let next = Journal {
        region: Some(planned.region.clone()),
        group: CreateState::Requested,
        ..journal.clone()
    };
    records.journal(&next)?;
    *journal = next;
    records.before_mutation()?;
    match client.put_resource_group(&planned.group, &planned.region, planned.tags(), cancel) {
        Ok(group) => {
            bind_group(&group, operation_id, &planned.region, journal, records)?;
            Ok(true)
        }
        Err(failure) if failure.definite() => {
            records.mutation_refused();
            let next = Journal {
                group: CreateState::Prepared,
                ..journal.clone()
            };
            records.journal(&next)?;
            *journal = next;
            Err(failure.into())
        }
        Err(failure) => Err(failure.into()),
    }
}

/// Binds `group` once it carries the operation's tag and sits in `region`, which the
/// journal then records: a group found by name fixes where the cloud runs.
fn bind_group(
    group: &Group,
    operation_id: &str,
    region: &str,
    journal: &mut Journal,
    records: &mut impl Records,
) -> Result<(), CloudError> {
    if group.tags.get(OPERATION_TAG).map(String::as_str) != Some(operation_id) {
        return Err(CloudError::IdentityMismatch);
    }
    if !group.location.eq_ignore_ascii_case(region) {
        return Err(CloudError::Invalid(
            "The cloud's Azure resource group is in a region the settings do not allow for it",
        ));
    }
    let next = Journal {
        region: Some(region.to_owned()),
        group: CreateState::Bound {
            worker_id: group.id.clone(),
        },
        ..journal.clone()
    };
    records.journal(&next)?;
    *journal = next;
    Ok(())
}

/// Submits the deployment for each fitting size until one is accepted. Only a
/// capacity refusal, which proves ARM made nothing, moves on to the next size.
fn create(
    client: &Azure,
    planned: &mut Planned,
    sizes: &[&'static VmSize],
    created_group: bool,
    operation: &mut CreateState,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<VirtualMachine, CloudError> {
    // A group an earlier attempt made may hold its worker, or a deployment that is
    // still making it, though the record of that attempt was lost.
    if !created_group {
        if let Some(vm) = client.get_vm(&planned.group, WORKER_VM_NAME, cancel)? {
            bind(&vm, operation, records)?;
            return Ok(vm);
        }
        if client
            .get_deployment(&planned.group, DEPLOYMENT_NAME, cancel)?
            .is_some_and(|state| !state.is_terminal())
        {
            set(operation, CreateState::Requested, records)?;
            return reconcile(client, &planned.group, operation, records, cancel);
        }
    }
    let template = template::template();
    let mut sold_out = Reason::default();
    for size in sizes {
        planned.size = size;
        set(operation, CreateState::Requested, records)?;
        records.before_mutation()?;
        match client.put_deployment(
            &planned.group,
            DEPLOYMENT_NAME,
            &template,
            &planned.parameters(),
            cancel,
        ) {
            Ok(deployed) => {
                // From here ARM may make the VM whatever this call sees, so the
                // fence stays requested until the VM is read.
                match &deployed.operation {
                    LongRunning::Accepted(running) => client.await_operation(running, cancel)?,
                    LongRunning::Completed if deployed.state.provisioning_state == "Succeeded" => {}
                    LongRunning::Completed => return Err(CloudError::Rejected(Reason::default())),
                }
                let vm = client
                    .get_vm(&planned.group, WORKER_VM_NAME, cancel)?
                    .ok_or(CloudError::WorkerLost)?;
                bind(&vm, operation, records)?;
                return Ok(vm);
            }
            Err(failure) if failure.definite() => {
                records.mutation_refused();
                set(operation, CreateState::Prepared, records)?;
                match CloudError::from(failure) {
                    CloudError::Capacity(reason) => sold_out = reason,
                    error => return Err(error),
                }
            }
            Err(failure) => return Err(failure.into()),
        }
    }
    Err(CloudError::Capacity(sold_out))
}

/// A requested worker whose create answer was lost or did not end: the VM is
/// adopted when it exists, and the deployment is followed to its end. Nothing is
/// sent but reads. A deployment that ended without a VM made none, so the fence
/// opens again for the next attempt; one that cannot be found stays unresolved.
fn reconcile(
    client: &Azure,
    group: &str,
    operation: &mut CreateState,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<VirtualMachine, CloudError> {
    if let Some(vm) = client.get_vm(group, WORKER_VM_NAME, cancel)? {
        bind(&vm, operation, records)?;
        return Ok(vm);
    }
    let state = client
        .get_deployment(group, DEPLOYMENT_NAME, cancel)?
        .ok_or(CloudError::CreationUnresolved)?;
    let state = if state.is_terminal() {
        state
    } else {
        client.await_deployment(group, DEPLOYMENT_NAME, cancel)?
    };
    match client.get_vm(group, WORKER_VM_NAME, cancel)? {
        Some(vm) => {
            bind(&vm, operation, records)?;
            Ok(vm)
        }
        // It ended and made no VM, so none can appear later.
        None if state.provisioning_state != "Succeeded" => {
            set(operation, CreateState::Prepared, records)?;
            Err(CloudError::Rejected(Reason::default()))
        }
        None => Err(CloudError::WorkerLost),
    }
}

/// The bound VM, which must still be the same instance.
fn inspect(client: &Azure, group: &str, instance: &str, cancel: &Cancellation) -> Result<VirtualMachine, CloudError> {
    let vm = client
        .get_vm(group, WORKER_VM_NAME, cancel)?
        .ok_or(CloudError::WorkerLost)?;
    // A VM made again under the same name is not the cloud's worker.
    if vm.instance_id.as_deref() != Some(instance) {
        return Err(CloudError::WorkerLost);
    }
    Ok(vm)
}

/// Binds the cloud to `vm`'s instance id once the VM is the operation's.
fn bind(vm: &VirtualMachine, operation: &mut CreateState, records: &mut impl Records) -> Result<(), CloudError> {
    let instance = vm.instance_id.clone().ok_or(CloudError::InvalidResponse)?;
    set(operation, CreateState::Bound { worker_id: instance }, records)
}

fn set(operation: &mut CreateState, next: CreateState, records: &mut impl Records) -> Result<(), CloudError> {
    records.operation(&next)?;
    *operation = next;
    Ok(())
}

/// The VM as a worker once it carries the operation's tag, runs in the cloud's
/// region on an allowed size, and holds exactly its workspace disk.
fn settle(
    client: &Azure,
    spec: &WorkerSpec,
    policy: &Policy,
    region: &str,
    vm: &VirtualMachine,
    cancel: &Cancellation,
) -> Result<Worker, CloudError> {
    let group = super::resource_group_name(&spec.operation_id);
    if vm.tags.get(OPERATION_TAG) != Some(&spec.operation_id) {
        return Err(CloudError::IdentityMismatch);
    }
    if !policy.sizes.contains(&vm.vm_size) || !vm.location.eq_ignore_ascii_case(region) {
        return Err(CloudError::Invalid(
            "The Azure VM's size or region is not one the settings allow for this cloud",
        ));
    }
    if !holds_workspace(vm, client.subscription_id(), &group) {
        return Err(CloudError::Invalid(
            "The Azure VM does not hold this cloud's workspace disk",
        ));
    }
    let described = worker(vm, address(client, &group, cancel)?, spec)?;
    described.verify(spec)?;
    described.verify_resources(spec)?;
    Ok(described)
}

/// The static public address the deployment reports for the group's worker.
pub(super) fn address(client: &Azure, group: &str, cancel: &Cancellation) -> Result<Option<IpAddr>, CloudError> {
    Ok(client
        .get_deployment(group, DEPLOYMENT_NAME, cancel)?
        .and_then(|state| {
            state
                .outputs
                .get(ADDRESS_OUTPUT)
                .and_then(|ip| ip.parse::<IpAddr>().ok())
        }))
}

/// Whether the VM's only data disk is the group's workspace disk on LUN 0.
#[must_use]
pub fn holds_workspace(vm: &VirtualMachine, subscription_id: &str, group: &str) -> bool {
    vm.data_disk_count == 1
        && vm.data_disks.iter().all(|disk| {
            disk.lun == Some(0)
                && resource_id_parts(&disk.id, subscription_id).is_some_and(|parts| {
                    parts.group.eq_ignore_ascii_case(group)
                        && parts.provider.eq_ignore_ascii_case("Microsoft.Compute")
                        && parts.kind.eq_ignore_ascii_case("disks")
                        && parts.name.eq_ignore_ascii_case(DATA_DISK_NAME)
                })
        })
        && !vm.data_disks.is_empty()
}

/// The VM as a worker. ARM reports the host, not the container, so the image is the
/// one the custom data pins; the VM's size gives its CPU and memory, and its
/// `vmId` is the worker id. The rate is left out: Horizon reports no Azure rate yet.
/// Only a deallocating or deallocated VM is a stopped worker: a VM that is only
/// powered off keeps its compute allocation, which Azure still bills, so it is
/// reported as neither running nor stopped.
/// # Errors
/// `Invalid` for a size outside the catalog or a VM without an instance id.
pub fn worker(vm: &VirtualMachine, address: Option<IpAddr>, spec: &WorkerSpec) -> Result<Worker, CloudError> {
    let size = VM_SIZES
        .iter()
        .find(|size| size.name == vm.vm_size)
        .ok_or(CloudError::Invalid("The Azure VM size is not one Horizon offers"))?;
    let id = vm.instance_id.as_deref().ok_or(CloudError::InvalidResponse)?;
    let desired = match (vm.provisioning_state.as_str(), vm.power_state.as_deref()) {
        ("Succeeded", Some("PowerState/running")) => "RUNNING",
        (_, Some("PowerState/deallocating" | super::lifecycle::DEALLOCATED)) => "EXITED",
        (_, Some("PowerState/stopping" | "PowerState/stopped")) => STOPPED_ALLOCATED,
        ("Failed", _) => "UNKNOWN",
        _ => "PROVISIONING",
    };
    let volume_gb = vm.data_disks.first().and_then(|disk| disk.size_gb);
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": spec.name(),
        "imageName": spec.image_digest,
        "desiredStatus": desired,
        "publicIp": address.map_or_else(String::new, |address| address.to_string()),
        "portMappings": address.map(|_| serde_json::json!({"22": 22})),
        "vcpuCount": size.vcpu,
        "memoryInGb": size.memory_gb,
        "containerDiskInGb": vm.os_disk_gb,
        "volumeInGb": volume_gb,
        "volumeMountPath": "/workspace",
        "dataCenterId": vm.location,
        "env": {"HORIZON_CLOUD_OPERATION": spec.operation_id},
    }))
    .map_err(|_| CloudError::Invalid("Azure VM could not be described as a worker"))
}
