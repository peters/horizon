//! What the provider does with a running worker: attest its SSH host key through the
//! control plane rather than trusting whatever answers on the network address, and stop
//! it by deallocating the VM with retained disks.
use super::super::{
    AzureError, AzureLifecycle, AzureManagementTransport, AzureRunCommand, AzureVmView, AzureWorker, WORKER_VM_NAME,
    deployment::{SSH_PORT, identity_tags},
    transport::remaining_at,
};
use super::{AzureClient, Observation, Placement, SSH_USERNAME, vm_lifecycle};
use crate::cloud_run::{
    WorkerTarget,
    interactive_worker::{InteractiveWorker, InteractiveWorkerSshEndpoint, valid_ssh_public_key},
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider},
};
use std::time::Duration;

/// Polling schedule for a power transition (deallocation or start) to be observed after
/// Azure accepted it; D-series transitions commonly take one to three minutes. The last
/// step repeats until the absolute bound ends the wait.
const POWER_BACKOFF_MS: [u64; 7] = [0, 1_000, 2_000, 4_000, 8_000, 15_000, 30_000];
/// Absolute bound on that wait, sleeps and requests included.
const POWER_BOUND: Duration = Duration::from_secs(300);
/// Budget kept back from the last sleep so one more poll fits before the deadline: the
/// final observation happens this close to the deadline, which is as near as a bounded
/// request allows (an ARM lookup normally answers in well under a second).
const FINAL_POLL_RESERVE: Duration = Duration::from_secs(5);
/// The non-billed state the worker must hold after a stop.
const RETAINED: AzureLifecycle = AzureLifecycle::Deallocated;

/// Trusted source for the runtime SSH host key of one exact worker; `None` keeps a
/// running worker in `Provisioning`. Implementations must obtain the key through a
/// channel authenticated to the named VM resource (an unauthenticated network scan is
/// not attestation). The client re-proves the group, VM, client-key tags and address
/// once the key is back, so a source need not repeat those checks; `host` and the
/// client key are passed for sources that can bind more tightly.
pub trait AzureHostKeySource: Send + Sync {
    /// The worker's OpenSSH Ed25519 host public key, or `None` when it is not yet
    /// available for this exact worker.
    #[must_use]
    fn host_key(&self, worker: &AzureWorker, host: &str, expected_client_key: &str) -> Option<String>;
}

impl<F> AzureHostKeySource for F
where
    F: Fn(&AzureWorker, &str, &str) -> Option<String> + Send + Sync,
{
    fn host_key(&self, worker: &AzureWorker, host: &str, expected_client_key: &str) -> Option<String> {
        self(worker, host, expected_client_key)
    }
}

/// Host keys read through the ARM run-command channel: the control plane executes a
/// fixed script inside the exact VM, so the returned key is bound to the authenticated
/// resource rather than to whatever answers on the network address.
pub struct AzureRunCommandHostKeys {
    transport: std::sync::Arc<dyn AzureManagementTransport>,
}

impl AzureRunCommandHostKeys {
    #[must_use]
    pub fn new(transport: std::sync::Arc<dyn AzureManagementTransport>) -> Self {
        Self { transport }
    }
}

impl AzureHostKeySource for AzureRunCommandHostKeys {
    fn host_key(&self, worker: &AzureWorker, _host: &str, _expected_client_key: &str) -> Option<String> {
        // The command must run in the worker's own subscription; a transport bound
        // elsewhere would read a same-named VM that is not this resource.
        if self.transport.subscription_id() != worker.subscription_id {
            return None;
        }
        let output = self
            .transport
            .run_command(&worker.resource_group, WORKER_VM_NAME, AzureRunCommand::HostKey)
            .ok()
            .flatten()?;
        let mut lines = output.lines().filter(|line| !line.trim().is_empty());
        let (line, extra) = (lines.next()?, lines.next());
        let mut fields = line.split_ascii_whitespace();
        let key = format!("{} {}", fields.next()?, fields.next()?);
        (extra.is_none() && valid_ssh_public_key(&key)).then_some(key)
    }
}

/// Whether `live` is the same VM instance as `baseline`: an instance identity observed
/// once must be observed again, so a VM deleted and recreated under the same name and
/// tags, while a wait or a key read was in flight, is never mistaken for this worker.
pub(super) fn same_instance(baseline: Option<&AzureVmView>, live: Option<&AzureVmView>) -> bool {
    match baseline.and_then(|vm| vm.instance_id.as_deref()) {
        Some(pinned) => live.and_then(|vm| vm.instance_id.as_deref()) == Some(pinned),
        None => true,
    }
}

impl AzureClient {
    /// The SSH endpoint, only with a host key attested for this exact worker. Reading
    /// the key can take a while, so the key is paired with the address only if the
    /// same owned group, VM and address are observed again once it is back; a group or
    /// VM replaced under the same name is an identity error, anything else is simply
    /// not attested yet.
    pub(super) fn attested_endpoint(
        &self,
        worker: &AzureWorker,
        instance: Option<&AzureVmView>,
        host: &str,
        target: &WorkerTarget,
        ssh_public_key: &str,
        placement: Placement,
    ) -> Result<Option<InteractiveWorkerSshEndpoint>, AzureError> {
        let Some(host_key) = self.host_keys.host_key(worker, host, ssh_public_key) else {
            return Ok(None);
        };
        let expected = identity_tags(worker.workflow_id, worker.job_id, target, ssh_public_key);
        let Some(group) = self.owned_group(&worker.resource_group, &expected)? else {
            return Ok(None);
        };
        if !group.id.eq_ignore_ascii_case(&worker.group_id) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        let fresh = self.observe(worker.clone(), &group, &expected, None, placement)?;
        if !same_instance(instance, fresh.as_ref().and_then(|fresh| fresh.vm.as_ref())) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        let unchanged = fresh
            .is_some_and(|fresh| fresh.lifecycle == AzureLifecycle::Running && fresh.host.as_deref() == Some(host));
        Ok(unchanged
            .then(|| InteractiveWorkerSshEndpoint {
                host: host.to_string(),
                port: SSH_PORT,
                username: SSH_USERNAME.to_string(),
                host_key,
            })
            .filter(InteractiveWorkerSshEndpoint::is_complete))
    }
}

impl InteractiveWorkerStopProvider for AzureClient {
    /// Deallocate the VM: compute is released and no longer billed, both disks and the
    /// static address are retained. Verified by observing `PowerState/deallocated`
    /// within the bound; a VM already deallocating is only awaited, never re-posted.
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        let Some((handle, group, expected)) = self.owned_handle(worker)? else {
            return Ok(InteractiveWorkerStop::AlreadyAbsent);
        };
        // The group was just proven present; losing it mid-observation is a race in
        // which no retained state was verified, not an absent worker.
        let Some(current) = self.observe(handle, &group, &expected, None, Placement::Ignore)? else {
            return Err(AzureError::StopUnverified);
        };
        let Some(vm) = current.vm.as_ref() else {
            return Err(AzureError::StopUnverified);
        };
        if current.lifecycle == AzureLifecycle::Deleting {
            return Err(AzureError::StopUnverified);
        }
        // Stoppability follows the VM itself: a running VM is billed whether or not its
        // deployment or address is usable, and Stop is the cost-control action.
        let deallocating = vm.power_state.as_deref() == Some("PowerState/deallocating");
        match vm_lifecycle(vm) {
            AzureLifecycle::Deallocated => return Ok(InteractiveWorkerStop::Stopped),
            AzureLifecycle::Running | AzureLifecycle::StoppedAllocated => {}
            AzureLifecycle::Transitioning if deallocating => {}
            AzureLifecycle::Transitioning
            | AzureLifecycle::Failed
            | AzureLifecycle::Deleting
            | AzureLifecycle::Unknown => return Err(AzureError::StopUnverified),
        }
        if !deallocating {
            match self
                .transport
                .deallocate_vm(&current.worker.resource_group, WORKER_VM_NAME)
            {
                // Compute answers 409 while a deallocation is already in flight.
                Ok(Some(_)) | Err(AzureError::UnexpectedStatus { status: 409, .. }) => {}
                // Vanished between the observation and the POST: nothing was verified.
                Ok(None) => return Err(AzureError::StopUnverified),
                Err(error) => return Err(error),
            }
        }
        match self.await_power(&current, &expected, RETAINED)? {
            Some(_) => Ok(InteractiveWorkerStop::Stopped),
            None => Err(AzureError::StopUnverified),
        }
    }
}

impl AzureClient {
    /// Wait for the exact worker's VM to reach `target`, under an absolute deadline that
    /// covers the sleeps with each request handed only what is left of it. The wait spans
    /// minutes, so every poll re-proves the group (identity tags, recorded ID, not
    /// deleting) and the VM (identity tags and instance identity): a recreated or
    /// retagged worker at the same path is an identity error, never this worker's
    /// transition. `None` means the
    /// transition was not verified within the bound (vanished, failed, deleting, or late).
    fn await_power(
        &self,
        current: &Observation,
        expected: &std::collections::BTreeMap<String, String>,
        target: AzureLifecycle,
    ) -> Result<Option<AzureVmView>, AzureError> {
        let group = &current.worker.resource_group;
        let deadline = self.clock.now() + POWER_BOUND;
        let last = POWER_BACKOFF_MS[POWER_BACKOFF_MS.len() - 1];
        let schedule = POWER_BACKOFF_MS.into_iter().chain(std::iter::repeat(last));
        for delay_ms in schedule {
            // Only the absolute deadline ends the wait; a transition that completes late
            // in the bound is still observed.
            let Some(left) = remaining_at(self.clock.now(), deadline) else {
                break;
            };
            // Never sleep into the deadline: the sleep stops a small reserve short of
            // it, and the poll made inside that reserve, a few seconds before the
            // deadline, is the last one, so a transition that completes at the very end
            // of the bound is still observed without busy-polling.
            let sleep_for = Duration::from_millis(delay_ms).min(left.saturating_sub(FINAL_POLL_RESERVE));
            self.clock.sleep(sleep_for);
            let Some(budget) = remaining_at(self.clock.now(), deadline) else {
                break;
            };
            let final_poll = budget <= FINAL_POLL_RESERVE;
            let Some(live) = self.transport.get_resource_group_within(group, budget)? else {
                break;
            };
            if expected.iter().any(|(key, value)| live.tags.get(key) != Some(value))
                || !live.id.eq_ignore_ascii_case(&current.worker.group_id)
            {
                return Err(AzureError::ResourceIdentityMismatch);
            }
            // Disks and address are going away with the group: no state is retained.
            if live.provisioning_state == "Deleting" {
                break;
            }
            let Some(budget) = remaining_at(self.clock.now(), deadline) else {
                break;
            };
            let Some(vm) = self.transport.get_vm_within(group, WORKER_VM_NAME, budget)? else {
                break;
            };
            if expected.iter().any(|(key, value)| vm.tags.get(key) != Some(value))
                || !same_instance(current.vm.as_ref(), Some(&vm))
            {
                return Err(AzureError::ResourceIdentityMismatch);
            }
            match vm_lifecycle(&vm) {
                lifecycle if lifecycle == target => return Ok(Some(vm)),
                AzureLifecycle::Failed | AzureLifecycle::Deleting => break,
                _ if final_poll => break,
                _ => {}
            }
        }
        Ok(None)
    }
}

impl InteractiveWorkerStartProvider for AzureClient {
    /// Start the VM's compute again after an explicit stop: the retained disks and the
    /// static address come back under the same identity. A running worker is never
    /// re-posted; a VM already starting is only awaited. Once running, the same worker
    /// is observed again through the readiness path, so `Started` carries `Ready` only
    /// with a freshly attested host key and `Provisioning` otherwise.
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        let Some((handle, group, expected)) = self.owned_handle(worker)? else {
            return Ok(InteractiveWorkerStart::AlreadyAbsent);
        };
        let Some(current) = self.observe(handle.clone(), &group, &expected, None, Placement::Ignore)? else {
            return Err(AzureError::StartUnverified);
        };
        let Some(vm) = current.vm.as_ref() else {
            return Err(AzureError::StartUnverified);
        };
        // A worker whose group is being deleted or whose deployment failed is not a
        // stopped worker, whatever its leftover VM reports: nothing is started.
        if matches!(current.lifecycle, AzureLifecycle::Deleting | AzureLifecycle::Failed) {
            return Err(AzureError::StartUnverified);
        }
        let starting = vm.power_state.as_deref() == Some("PowerState/starting");
        match vm_lifecycle(vm) {
            AzureLifecycle::Running => {
                let status = self.status(current, &worker.target, &worker.ssh_public_key, Placement::Ignore)?;
                return Ok(InteractiveWorkerStart::AlreadyRunning(status));
            }
            AzureLifecycle::Deallocated | AzureLifecycle::StoppedAllocated => {}
            AzureLifecycle::Transitioning if starting => {}
            AzureLifecycle::Transitioning
            | AzureLifecycle::Failed
            | AzureLifecycle::Deleting
            | AzureLifecycle::Unknown => return Err(AzureError::StartUnverified),
        }
        if !starting {
            match self.transport.start_vm(&current.worker.resource_group, WORKER_VM_NAME) {
                // Compute answers 409 while a start is already in flight.
                Ok(Some(_)) | Err(AzureError::UnexpectedStatus { status: 409, .. }) => {}
                // Vanished between the observation and the POST: nothing to start, and
                // never something to allocate.
                Ok(None) => return Err(AzureError::StartUnverified),
                Err(error) => return Err(error),
            }
        }
        if self
            .await_power(&current, &expected, AzureLifecycle::Running)?
            .is_none()
        {
            return Err(AzureError::StartUnverified);
        }
        // Running is not ready: the same worker goes through the full observation and
        // attestation path before anything is claimed about it.
        let Some(live) = self.owned_group(&handle.resource_group, &expected)? else {
            return Err(AzureError::StartUnverified);
        };
        if !live.id.eq_ignore_ascii_case(&handle.group_id) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        let Some(fresh) = self.observe(handle, &live, &expected, None, Placement::Ignore)? else {
            return Err(AzureError::StartUnverified);
        };
        if !same_instance(current.vm.as_ref(), fresh.vm.as_ref()) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        // Only a worker whose VM is physically running is a started worker; anything
        // else observed after the wait (a failure, a deletion begun meanwhile, a new
        // transition, an unreadable power state) stays unverified. A running VM without
        // a usable address is observed as Unknown; it is started, just not reachable, and
        // is reported as Provisioning rather than Ready.
        let running_vm = fresh
            .vm
            .as_ref()
            .is_some_and(|vm| vm_lifecycle(vm) == AzureLifecycle::Running);
        if !running_vm || !matches!(fresh.lifecycle, AzureLifecycle::Running | AzureLifecycle::Unknown) {
            return Err(AzureError::StartUnverified);
        }
        let fresh = Observation {
            lifecycle: AzureLifecycle::Running,
            ..fresh
        };
        let status = self.status(fresh, &worker.target, &worker.ssh_public_key, Placement::Ignore)?;
        Ok(InteractiveWorkerStart::Started(status))
    }
}
