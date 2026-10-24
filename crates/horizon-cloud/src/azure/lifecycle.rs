//! Stop, start and delete of an operation's worker, each proven by what ARM reports
//! afterwards. A stop deallocates the VM and never only powers it off: a VM that is
//! powered off keeps its compute allocation, and Azure still bills it. A start
//! powers the same VM on again with the disks and the static address it kept.
//!
//! A delete removes the operation's resource group, which holds everything the
//! worker owns and is Azure's billing and delete scope. The caller's [`Journal`]
//! records each step before the next: `deleting` before the first delete request,
//! the group `Terminated` once ARM answers 404 for it, and `swept` once a
//! subscription-wide `horizon-operation` tag sweep after that 404 lists nothing.
//! A delete request is sent only while the group is present and not already being
//! deleted, so a lost answer is reconciled by reading the group, not by a blind repeat.
use super::{
    ACTION_TIMEOUT, Azure, BACKOFF_MS, COMPUTE_API_VERSION, Failure, LongRunning, Method, OPERATION_TAG,
    WORKER_VM_NAME,
    operation::{operation_ref, remaining_at},
    provision::{Journal, Records, address, holds_workspace, worker},
    resource::{VirtualMachine, text},
    resource_group_name,
    url::valid_resource_group_name,
};
use crate::{Cancellation, CloudError, CreateState, Worker, WorkerSpec};
use std::time::{Duration, Instant};

/// The power state of a VM that keeps its disks and address but no compute.
pub const DEALLOCATED: &str = "PowerState/deallocated";
pub const RUNNING: &str = "PowerState/running";
/// The API version of the subscription's resource list, which the tag sweep reads.
pub const RESOURCES_API_VERSION: &str = "2021-04-01";
/// How long a delete waits before it trusts the 404 of a group whose create answer
/// was lost, so a create still on its way cannot make the group after the proof.
pub const UNRESOLVED_GRACE: Duration = Duration::from_secs(30);
/// The tag sweep is read again on these waits, in milliseconds, while ARM's list
/// still shows resources of a group it has deleted.
const SWEEP_BACKOFF_MS: [u64; 5] = [1_000, 2_000, 4_000, 8_000, 15_000];

/// A power change of the cloud's VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Releases the compute and keeps the disks and the static address.
    Deallocate,
    /// Powers the same VM on again.
    Start,
}

impl Power {
    fn action(self) -> &'static str {
        match self {
            Self::Deallocate => "deallocate",
            Self::Start => "start",
        }
    }

    /// The power state that ends the change.
    #[must_use]
    pub fn target(self) -> &'static str {
        match self {
            Self::Deallocate => DEALLOCATED,
            Self::Start => RUNNING,
        }
    }

    /// The power state of the change while ARM still carries it out.
    fn transition(self) -> &'static str {
        match self {
            Self::Deallocate => "PowerState/deallocating",
            Self::Start => "PowerState/starting",
        }
    }
}

/// The cloud's bound VM and the worker it is.
#[derive(Clone, Debug)]
pub struct Bound {
    pub vm: VirtualMachine,
    pub worker: Worker,
}

impl Bound {
    /// The VM holds no compute, so Azure bills only its disks and address.
    #[must_use]
    pub fn deallocated(&self) -> bool {
        self.vm.power_state.as_deref() == Some(DEALLOCATED)
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.vm.power_state.as_deref() == Some(RUNNING)
    }
}

/// What the `horizon-operation` tag sweep listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// The resource ids ARM listed. They are untrusted and never printed.
    pub ids: Vec<String>,
    /// ARM has a further page, so the list is not complete.
    pub more: bool,
}

impl Sweep {
    /// The sweep proves that no resource carries the tag.
    #[must_use]
    pub fn empty(&self) -> bool {
        self.ids.is_empty() && !self.more
    }
}

impl Azure {
    /// Asks ARM to change the power of VM `name` in `group`.
    /// # Errors
    /// `Invalid` for a bad segment (nothing is sent), `InvalidResponse` for an
    /// operation URL outside this subscription, or the provider failure.
    pub fn vm_power(
        &self,
        group: &str,
        name: &str,
        power: Power,
        cancel: &Cancellation,
    ) -> Result<LongRunning, Failure> {
        let url = self.resource_url(group, "Microsoft.Compute/virtualMachines", name, COMPUTE_API_VERSION)?;
        let url = url.replacen('?', &format!("/{}?", power.action()), 1);
        let reply = self.send_within(Method::Post, &url, None, cancel, None)?;
        match operation_ref(&reply.headers) {
            Some(operation) if self.owns_operation_url(operation.url()) => Ok(LongRunning::Accepted(operation)),
            Some(_) => Err(Failure::Local(CloudError::InvalidResponse)),
            None => Ok(LongRunning::Completed),
        }
    }

    /// Asks ARM to delete resource group `name` and everything in it. A 404 means the
    /// group is already gone.
    /// # Errors
    /// `Invalid` for a bad group name (nothing is sent), or the provider failure.
    pub fn delete_resource_group(&self, name: &str, cancel: &Cancellation) -> Result<(), Failure> {
        let url = self.group_url(name)?;
        match self.send_within(Method::Delete, &url, None, cancel, None) {
            Ok(_) => Ok(()),
            Err(failure) if failure.not_found() => Ok(()),
            Err(failure) => Err(failure),
        }
    }

    /// Lists the resources of this subscription whose `horizon-operation` tag names
    /// `operation_id`, wherever they are.
    /// # Errors
    /// `Invalid` for an id that cannot be a tag value (nothing is sent),
    /// `InvalidResponse` for an answer without a list, or the provider failure.
    pub fn tagged_resources(&self, operation_id: &str, cancel: &Cancellation) -> Result<Sweep, Failure> {
        if !crate::valid_id(operation_id) {
            return Err(Failure::Local(CloudError::Invalid("Invalid operation id")));
        }
        let url = format!(
            "{}/subscriptions/{}/resources?$filter=tagName%20eq%20'{OPERATION_TAG}'%20and%20tagValue%20eq%20'{operation_id}'&api-version={RESOURCES_API_VERSION}",
            self.endpoint, self.subscription_id
        );
        let reply = self.send_within(Method::Get, &url, None, cancel, None)?;
        let listed = reply
            .value
            .get("value")
            .and_then(serde_json::Value::as_array)
            .ok_or(Failure::Local(CloudError::InvalidResponse))?;
        Ok(Sweep {
            ids: listed
                .iter()
                .map(|resource| text(resource, "/id").unwrap_or_default())
                .collect(),
            more: reply.value.get("nextLink").is_some_and(|link| !link.is_null()),
        })
    }
}

/// The bound VM `instance` as the worker it is, checked by ownership only, so a
/// changed size or region policy never blocks a stop, a start or a check. Sends GET
/// requests only.
/// # Errors
/// `IdentityMismatch` for a VM without the operation's tag, `Invalid` for one that
/// does not hold the workspace disk or cannot be described, and the provider failures.
/// `Ok(None)` when the group has no VM, or only another instance under the name.
pub fn inspect_bound(
    client: &Azure,
    spec: &WorkerSpec,
    instance: &str,
    cancel: &Cancellation,
) -> Result<Option<Bound>, CloudError> {
    let group = resource_group_name(&spec.operation_id);
    let Some(vm) = client.get_vm(&group, WORKER_VM_NAME, cancel)? else {
        return Ok(None);
    };
    if vm.instance_id.as_deref() != Some(instance) {
        return Ok(None);
    }
    if vm.tags.get(OPERATION_TAG) != Some(&spec.operation_id) {
        return Err(CloudError::IdentityMismatch);
    }
    if !holds_workspace(&vm, client.subscription_id(), &group) {
        return Err(CloudError::Invalid(
            "The Azure VM does not hold this cloud's workspace disk",
        ));
    }
    let described = worker(&vm, address(client, &group, cancel)?, spec)?;
    described.verify(spec)?;
    Ok(Some(Bound { vm, worker: described }))
}

/// Deallocates or starts the bound VM and waits until ARM reports the change done.
/// `announce` is called once, just before the request, and also when an earlier
/// request of the same change is still under way; then nothing is sent again.
/// # Errors
/// `WorkerLost` when the VM is gone or is another instance, `Rejected` for a change
/// ARM reports failed, `Invalid` when it is still under way at `ACTION_TIMEOUT`, and
/// the provider and announcement failures.
pub fn power(
    client: &Azure,
    spec: &WorkerSpec,
    instance: &str,
    power: Power,
    cancel: &Cancellation,
    announce: &mut dyn FnMut() -> Result<(), CloudError>,
) -> Result<Bound, CloudError> {
    let bound = inspect_bound(client, spec, instance, cancel)?.ok_or(CloudError::WorkerLost)?;
    match bound.vm.power_state.as_deref() {
        Some(state) if state == power.target() => return Ok(bound),
        Some(state) if state == power.transition() => announce()?,
        _ => {
            announce()?;
            let group = resource_group_name(&spec.operation_id);
            if let LongRunning::Accepted(operation) = client.vm_power(&group, WORKER_VM_NAME, power, cancel)? {
                client.await_operation(&operation, cancel)?;
            }
        }
    }
    await_power(client, spec, instance, power, cancel)
}

/// Reads the VM until it reports `power`'s target state, within `ACTION_TIMEOUT`.
fn await_power(
    client: &Azure,
    spec: &WorkerSpec,
    instance: &str,
    power: Power,
    cancel: &Cancellation,
) -> Result<Bound, CloudError> {
    let started = Instant::now();
    let deadline = started + ACTION_TIMEOUT;
    let mut waited = Duration::ZERO;
    let steps = BACKOFF_MS
        .iter()
        .copied()
        .chain(BACKOFF_MS.last().copied().into_iter().cycle());
    for step in steps {
        let bound = inspect_bound(client, spec, instance, cancel)?.ok_or(CloudError::WorkerLost)?;
        if bound.vm.power_state.as_deref() == Some(power.target()) {
            return Ok(bound);
        }
        let Some(left) = remaining_at(Instant::now().max(started + waited), deadline) else {
            break;
        };
        let delay = Duration::from_millis(step).min(left);
        client.wait(delay, cancel)?;
        waited += delay;
    }
    Err(CloudError::Invalid(
        "Provider operation is still running; check again later",
    ))
}

/// Deletes everything `operation_id` made on Azure and proves it absent. On success
/// the journal proves the absence ([`Journal::proves_absent`]) and the VM fence is
/// `Terminated` with its instance id, or `Prepared` when no VM was bound.
/// # Errors
/// `Invalid` for an id that cannot name a group (nothing is sent),
/// `IdentityMismatch` for a group of the name without the operation's tag, which is
/// never deleted, `Invalid` while ARM still deletes the group or still lists tagged
/// resources, and the provider and persistence failures. Each leaves records that a
/// retry resumes from.
pub fn delete(
    client: &Azure,
    operation_id: &str,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    let group = resource_group_name(operation_id);
    if !crate::valid_id(operation_id) || !valid_resource_group_name(&group) {
        return Err(CloudError::Invalid("Invalid operation id"));
    }
    if !journal.deleting {
        let next = Journal {
            deleting: true,
            ..journal.clone()
        };
        records.journal(&next)?;
        *journal = next;
    }
    if !journal.proves_absent(operation_id) {
        let unresolved = journal.group == CreateState::Requested;
        let id = match &journal.group {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            CreateState::Prepared | CreateState::Requested => {
                format!("/subscriptions/{}/resourceGroups/{group}", client.subscription_id())
            }
        };
        absent(client, operation_id, &group, unresolved, records, cancel)?;
        // Nothing of a group that was never requested can be named by a sweep that
        // fails, so its fence moves only together with the sweep.
        if matches!(journal.group, CreateState::Requested | CreateState::Bound { .. }) {
            let next = Journal {
                group: CreateState::Terminated { worker_id: id.clone() },
                ..journal.clone()
            };
            records.journal(&next)?;
            *journal = next;
        }
        sweep(client, operation_id, cancel)?;
        let next = Journal {
            group: CreateState::Terminated { worker_id: id },
            swept: true,
            ..journal.clone()
        };
        records.journal(&next)?;
        *journal = next;
    }
    let ended = match operation {
        CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => CreateState::Terminated {
            worker_id: worker_id.clone(),
        },
        // The group held any VM a lost create made, so none can exist now.
        CreateState::Prepared | CreateState::Requested => CreateState::Prepared,
    };
    if *operation != ended {
        records.operation(&ended)?;
        *operation = ended;
    }
    Ok(())
}

/// Reads the operation's group until ARM answers 404, sending one delete request
/// when the group is present and not already being deleted.
fn absent(
    client: &Azure,
    operation_id: &str,
    group: &str,
    unresolved: bool,
    records: &mut impl Records,
    cancel: &Cancellation,
) -> Result<(), CloudError> {
    let started = Instant::now();
    let deadline = started + ACTION_TIMEOUT;
    let mut waited = Duration::ZERO;
    let (mut graced, mut sent) = (!unresolved, false);
    let steps = BACKOFF_MS
        .iter()
        .copied()
        .chain(BACKOFF_MS.last().copied().into_iter().cycle());
    for step in steps {
        match client.get_resource_group(group, cancel)? {
            None if graced => return Ok(()),
            None => {
                // A create whose answer was lost may still make the group.
                client.wait(UNRESOLVED_GRACE, cancel)?;
                waited += UNRESOLVED_GRACE;
                graced = true;
                continue;
            }
            Some(found) => {
                if found.tags.get(OPERATION_TAG).map(String::as_str) != Some(operation_id) {
                    return Err(CloudError::IdentityMismatch);
                }
                if !sent && !found.provisioning_state.eq_ignore_ascii_case("Deleting") {
                    records.before_mutation()?;
                    client.delete_resource_group(group, cancel)?;
                    sent = true;
                }
            }
        }
        let Some(left) = remaining_at(Instant::now().max(started + waited), deadline) else {
            break;
        };
        let delay = Duration::from_millis(step).min(left);
        client.wait(delay, cancel)?;
        waited += delay;
    }
    Err(CloudError::Invalid(
        "Azure is still deleting the cloud's resource group; delete the cloud again later to finish",
    ))
}

/// Reads the tag sweep until it lists nothing. ARM's list can show the resources of
/// a deleted group for a short time, so it is read again a few times.
fn sweep(client: &Azure, operation_id: &str, cancel: &Cancellation) -> Result<(), CloudError> {
    for step in std::iter::once(None).chain(SWEEP_BACKOFF_MS.iter().copied().map(Some)) {
        if let Some(step) = step {
            client.wait(Duration::from_millis(step), cancel)?;
        }
        if client.tagged_resources(operation_id, cancel)?.empty() {
            return Ok(());
        }
    }
    Err(CloudError::Invalid(
        "Azure still lists resources tagged with this cloud; examine them, then delete the cloud again",
    ))
}
