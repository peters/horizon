//! The Azure provider behind the common interactive-worker contract: create once
//! behind a durable fence, recover and inspect without creating, delete exactly the
//! owned resource group; readiness attestation and Stop live in `running`.
use super::{
    AzureArmHttp, AzureCredentialSource, AzureDeploymentPlan, AzureError, AzureGroupInfo, AzureLifecycle,
    AzureManagementTransport, AzureProfile, AzureVmView, AzureWorker, WORKER_VM_NAME,
    deployment::{DEPLOYMENT_NAME, identity_tags, worker_tags},
    resource_group_name,
};
use crate::cloud_run::{
    CloudJobId, CloudProvider, CloudWorkflowId, WorkerTarget,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
        InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest,
        InteractiveWorkerStatus,
    },
};
use std::{collections::BTreeMap, sync::Arc};

mod running;
pub use running::{AzureHostKeySource, AzureRunCommandHostKeys};

/// The worker image accepts the client key for `root` only.
pub const SSH_USERNAME: &str = "root";
/// Durable, cross-controller compare-and-set whose claim survives process exit. A
/// resource group name may return `true` at most once. The workflow store implements
/// it where the production client is assembled.
pub trait AzureCreationFence: Send + Sync {
    /// # Errors
    /// Returns [`AzureError::CreationFenceFailed`] when the durable claim cannot be read
    /// or recorded; the underlying cause stays with the adapter, never in this error.
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_group: &str,
    ) -> Result<bool, AzureError>;
}

impl<F> AzureCreationFence for F
where
    F: Fn(CloudWorkflowId, CloudJobId, &WorkerTarget, &str) -> Result<bool, AzureError> + Send + Sync,
{
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_group: &str,
    ) -> Result<bool, AzureError> {
        self(workflow_id, job_id, target, resource_group)
    }
}

/// The workflow store is the production fence: one durable claim per job, shared by
/// every controller that opens the same store.
impl AzureCreationFence for crate::cloud_run::CloudWorkflowStore {
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_group: &str,
    ) -> Result<bool, AzureError> {
        self.claim_worker_creation(workflow_id, job_id, target, resource_group)
            .map_err(|_| AzureError::CreationFenceFailed)
    }
}

/// Provider for persistent Azure CPU workers.
pub struct AzureClient {
    profile: AzureProfile,
    transport: Box<dyn AzureManagementTransport>,
    fence: Box<dyn AzureCreationFence>,
    host_keys: Box<dyn AzureHostKeySource>,
}

/// Whether an observation must also match the current profile's VM size and location.
/// Request-driven paths enforce it; persisted-handle paths stay policy-independent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Placement {
    Enforce,
    Ignore,
}

/// How `ensure` came by the worker's group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Origin {
    /// Found before the fence: this controller's earlier work, repairable.
    Existing,
    /// Created by this call, repairable.
    Created,
    /// Appeared during the claim: another controller's work, observed only.
    Adopted,
}

/// One worker's exact handle plus what the control plane currently shows.
#[derive(Debug)]
struct Observation {
    worker: AzureWorker,
    lifecycle: AzureLifecycle,
    /// Neither a deployment nor a VM exists yet: the only state repair may act on.
    bare: bool,
    host: Option<String>,
    vm: Option<AzureVmView>,
}

impl AzureClient {
    /// Production client: HTTPS-only requests to Azure Resource Manager under the
    /// profile's subscription, one authenticated transport shared by the management
    /// calls and the run-command host-key source, and the given durable fence.
    /// # Errors
    /// Rejects an invalid profile.
    pub fn new(
        profile: AzureProfile,
        credential: impl AzureCredentialSource + 'static,
        fence: impl AzureCreationFence + 'static,
    ) -> Result<Self, AzureError> {
        profile.validate()?;
        let transport = Arc::new(AzureArmHttp::new(profile.subscription_id.clone(), credential)?);
        let host_keys = AzureRunCommandHostKeys::new(transport.clone());
        Self::with_transport(profile, transport, fence, host_keys)
    }

    /// Client over any management transport and host-key source.
    /// # Errors
    /// Rejects an invalid profile.
    pub fn with_transport(
        profile: AzureProfile,
        transport: impl AzureManagementTransport + 'static,
        fence: impl AzureCreationFence + 'static,
        host_keys: impl AzureHostKeySource + 'static,
    ) -> Result<Self, AzureError> {
        profile.validate()?;
        // A transport for another subscription would create in the wrong place and then
        // reject every handle it produced.
        if transport.subscription_id() != profile.subscription_id {
            return Err(AzureError::InvalidProfile);
        }
        Ok(Self {
            profile,
            transport: Box::new(transport),
            fence: Box::new(fence),
            host_keys: Box::new(host_keys),
        })
    }

    /// The persisted handle, shape-checked before any I/O. Cost and placement policy are
    /// not re-applied, so a handle stays inspectable and deletable after profile changes,
    /// with one deliberate exception: a handle is bound to the profile's subscription,
    /// and a profile pointed at another subscription cannot address it.
    fn handle(&self, worker: &InteractiveWorker) -> Result<AzureWorker, AzureError> {
        if !worker.is_valid_for(CloudProvider::Azure) {
            return Err(AzureError::InvalidPersistedWorker);
        }
        let handle = AzureWorker {
            workflow_id: worker.identity.workflow_id,
            job_id: worker.identity.job_id,
            subscription_id: self.profile.subscription_id.clone(),
            resource_group: resource_group_name(worker.identity.workflow_id, worker.identity.job_id),
            group_id: worker.identity.resource_id.clone(),
            image: worker.target.image.clone(),
            lifetime: worker.lifetime.clone(),
        };
        handle.validate().map(|()| handle)
    }

    /// Every identity tag must match on the group itself.
    fn owned_group(
        &self,
        group: &str,
        expected: &BTreeMap<String, String>,
    ) -> Result<Option<AzureGroupInfo>, AzureError> {
        let Some(info) = self.transport.get_resource_group(group)? else {
            return Ok(None);
        };
        if expected.iter().any(|(key, value)| info.tags.get(key) != Some(value)) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        Ok(Some(info))
    }

    fn worker_for(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        group: &AzureGroupInfo,
        image: &str,
    ) -> AzureWorker {
        AzureWorker {
            workflow_id,
            job_id,
            subscription_id: self.profile.subscription_id.clone(),
            resource_group: group.name.clone(),
            group_id: group.id.clone(),
            image: image.to_string(),
            lifetime: InteractiveWorkerLifetime::Persistent,
        }
    }

    /// What the control plane shows for an owned group: the VM when it exists, else the
    /// deployment that should produce it. `resubmit` lets `ensure` repair a group whose
    /// deployment was never recorded; recovery never resubmits; a deleting group is left alone.
    fn observe(
        &self,
        worker: AzureWorker,
        group: &AzureGroupInfo,
        expected: &BTreeMap<String, String>,
        resubmit: Option<&AzureDeploymentPlan>,
        placement: Placement,
    ) -> Result<Option<Observation>, AzureError> {
        if group.provisioning_state == "Deleting" {
            // Re-prove before reporting: the snapshot may predate a retag or removal.
            return Ok(self
                .owned_group(&worker.resource_group, expected)?
                .map(|_| Observation {
                    worker,
                    lifecycle: AzureLifecycle::Deleting,
                    bare: false,
                    host: None,
                    vm: None,
                }));
        }
        // The group's region is immutable; a profile moved to another region under the
        // same name must never repair into, or serve, a group in the old one.
        if placement == Placement::Enforce && group.location != self.profile.location {
            return Err(AzureError::PlacementMismatch);
        }
        let deployment = self.transport.get_deployment(&worker.resource_group, DEPLOYMENT_NAME)?;
        let deployment_state = deployment.as_ref().map(|state| state.provisioning_state.as_str());
        let host = deployment
            .as_ref()
            .filter(|state| state.provisioning_state == "Succeeded")
            .and_then(|state| state.outputs.get("publicIp").cloned())
            .filter(|host| host.parse::<std::net::IpAddr>().is_ok_and(routable));
        let vm = self.transport.get_vm(&worker.resource_group, WORKER_VM_NAME)?;
        let lifecycle = match &vm {
            Some(vm) => {
                if expected.iter().any(|(key, value)| vm.tags.get(key) != Some(value)) {
                    return Err(AzureError::ResourceIdentityMismatch);
                }
                // A terminal deployment failure wins over everything the leftover VM shows.
                if matches!(deployment_state, Some("Failed" | "Canceled")) {
                    AzureLifecycle::Failed
                } else if placement == Placement::Enforce
                    && (vm.vm_size != self.profile.vm_size || vm.location != self.profile.location)
                {
                    // A profile edited under the same name, or a VM resized out of band,
                    // must not be served as the requested target on request-driven paths.
                    return Err(AzureError::PlacementMismatch);
                } else {
                    match vm_lifecycle(vm) {
                        // Running but unreachable by design: neither ready nor provisioning.
                        AzureLifecycle::Running if host.is_none() && deployment_state == Some("Succeeded") => {
                            AzureLifecycle::Unknown
                        }
                        lifecycle => lifecycle,
                    }
                }
            }
            None => match (deployment_state, resubmit) {
                (Some("Failed" | "Canceled"), _) => AzureLifecycle::Failed,
                // A finished deployment without its VM, or a group with no deployment
                // that recovery may not repair: ambiguous, never treated as retained.
                (Some("Succeeded"), _) | (None, None) => AzureLifecycle::Unknown,
                (Some(_), _) => AzureLifecycle::Transitioning,
                // Repair only after a complete fresh observation right before the PUT: the
                // group must still be owned and not deleting, and neither a deployment nor
                // a VM may have appeared; anything that did is observed through the same
                // identity, placement and lifecycle checks instead of being written over.
                (None, Some(plan)) => match self.owned_group(&plan.resource_group, expected)? {
                    Some(current) => match self.observe(worker.clone(), &current, expected, None, placement)? {
                        Some(fresh) if fresh.bare && fresh.lifecycle != AzureLifecycle::Deleting => {
                            self.submit(plan)?
                        }
                        fresh => return Ok(fresh),
                    },
                    None => return Ok(None),
                },
            },
        };
        // The reads above took time; a deletion or retag that began meanwhile must not be
        // reported as a live state, and a group that vanished is absent, not a status.
        let lifecycle = match self.owned_group(&worker.resource_group, expected)? {
            Some(current) if current.provisioning_state == "Deleting" => AzureLifecycle::Deleting,
            Some(current) if placement == Placement::Enforce && current.location != self.profile.location => {
                return Err(AzureError::PlacementMismatch);
            }
            Some(_) => lifecycle,
            None => return Ok(None),
        };
        let bare = vm.is_none() && deployment.is_none() && resubmit.is_none();
        Ok(Some(Observation {
            worker,
            lifecycle,
            bare,
            host,
            vm,
        }))
    }

    /// Submit the deployment; ARM may complete the PUT synchronously, even as failed. A
    /// synchronous success means the VM now exists but was not yet observed, so the
    /// worker is provisioning until the next observation, never `Unknown`.
    fn submit(&self, plan: &AzureDeploymentPlan) -> Result<AzureLifecycle, AzureError> {
        let state = self
            .transport
            .put_deployment(&plan.resource_group, DEPLOYMENT_NAME, &plan.template, &plan.parameters)
            .map_err(|cause| AzureError::CreationIncomplete { cause: Box::new(cause) })?;
        Ok(match state.provisioning_state.as_str() {
            "Failed" | "Canceled" => AzureLifecycle::Failed,
            _ => AzureLifecycle::Transitioning,
        })
    }

    fn status(
        &self,
        observation: Observation,
        target: &WorkerTarget,
        ssh_public_key: &str,
        placement: Placement,
    ) -> Result<InteractiveWorkerStatus, AzureError> {
        let worker = AzureWorker {
            image: target.image.clone(),
            ..observation.worker
        };
        let ssh = match (observation.lifecycle, observation.host.as_deref()) {
            (AzureLifecycle::Running, Some(host)) => self.attested_endpoint(
                &worker,
                observation.vm.as_ref(),
                host,
                target,
                ssh_public_key,
                placement,
            )?,
            _ => None,
        };
        let lifecycle = match observation.lifecycle {
            AzureLifecycle::Running if ssh.is_some() => InteractiveWorkerLifecycle::Ready,
            AzureLifecycle::Running | AzureLifecycle::Transitioning => InteractiveWorkerLifecycle::Provisioning,
            AzureLifecycle::Deallocated => InteractiveWorkerLifecycle::Stopped,
            AzureLifecycle::Failed => InteractiveWorkerLifecycle::Failed,
            AzureLifecycle::Deleting => InteractiveWorkerLifecycle::Deleting,
            // Guest halted but compute still billed: not a retained stop, not ready.
            AzureLifecycle::StoppedAllocated | AzureLifecycle::Unknown => InteractiveWorkerLifecycle::Unknown,
        };
        Ok(InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::Azure,
                    workflow_id: worker.workflow_id,
                    job_id: worker.job_id,
                    resource_id: worker.group_id,
                },
                target: target.clone(),
                ssh_public_key: ssh_public_key.to_string(),
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle,
            ssh,
        })
    }

    /// Create the group behind the durable fence. ARM is consulted again right before
    /// the PUT: a group left by an earlier attempt or a concurrent controller is adopted
    /// through the tag proof (or refused), never overwritten.
    fn create(
        &self,
        plan: &AzureDeploymentPlan,
        request: &InteractiveWorkerRequest,
    ) -> Result<(AzureGroupInfo, Origin), AzureError> {
        let claimed = self.fence.claim_once(
            request.workflow_id,
            request.job_id,
            &request.target,
            &plan.resource_group,
        )?;
        // The group PUT is an upsert, so look once more immediately before it: anything
        // that appeared since the first lookup must pass the same ownership proof and is
        // then observed rather than overwritten.
        match (claimed, self.owned_group(&plan.resource_group, &plan.tags)?) {
            (_, Some(group)) => Ok((group, Origin::Adopted)),
            (false, None) => Err(AzureError::CreationUnresolved),
            (true, None) => {
                let group = self
                    .transport
                    .create_resource_group(&plan.resource_group, &plan.location, &plan.tags)?;
                Ok((group, Origin::Created))
            }
        }
    }
}

/// A public address the worker can actually be reached on: no unspecified, loopback,
/// multicast, link-local, private or unique-local values.
fn routable(address: std::net::IpAddr) -> bool {
    // An IPv4-mapped IPv6 value is judged by the IPv4 rules it represents.
    let address = match address {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, std::net::IpAddr::V4),
        v4 @ std::net::IpAddr::V4(_) => v4,
    };
    match address {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_multicast()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_private())
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unicast_link_local()
                || v6.is_unique_local())
        }
    }
}

fn vm_lifecycle(vm: &AzureVmView) -> AzureLifecycle {
    AzureLifecycle::from_states(Some(&vm.provisioning_state), vm.power_state.as_deref())
}

impl InteractiveWorkerProvider for AzureClient {
    type Error = AzureError;

    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }

    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        let plan = AzureDeploymentPlan::new(&self.profile, request)?;
        let expected = worker_tags(request);
        let (group, origin) = match self.owned_group(&plan.resource_group, &expected)? {
            Some(group) => (group, Origin::Existing),
            None => self.create(&plan, request)?,
        };
        let worker = self.worker_for(request.workflow_id, request.job_id, &group, &request.target.image);
        // A group that raced in during the claim is only observed; repair is reserved
        // for groups this controller found before the fence or created itself.
        let repair = (origin != Origin::Adopted).then_some(&plan);
        // Creation and repair share one path: the deployment PUT happens only after the
        // group is re-proven owned and not deleting, and never over a deployment that
        // appeared in the meantime.
        // Vanishing during creation's own observation is a lost worker, not an absence.
        let observation = self
            .observe(worker, &group, &expected, repair, Placement::Enforce)?
            .ok_or(AzureError::CreationUnresolved)?;
        let status = self.status(
            observation,
            &request.target,
            &request.ssh_public_key,
            Placement::Enforce,
        )?;
        Ok(if origin == Origin::Created {
            InteractiveWorkerEnsure::Created(status)
        } else {
            InteractiveWorkerEnsure::Reused(status)
        })
    }

    fn reconcile_worker(
        &self,
        request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        let plan = AzureDeploymentPlan::new(&self.profile, request)?;
        let expected = worker_tags(request);
        let Some(group) = self.owned_group(&plan.resource_group, &expected)? else {
            return Ok(None);
        };
        let worker = self.worker_for(request.workflow_id, request.job_id, &group, &request.target.image);
        let observation = self.observe(worker, &group, &expected, None, Placement::Enforce)?;
        observation
            .map(|observation| {
                self.status(
                    observation,
                    &request.target,
                    &request.ssh_public_key,
                    Placement::Enforce,
                )
            })
            .transpose()
    }

    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        let Some((handle, group, expected)) = self.owned_handle(worker)? else {
            return Ok(None);
        };
        let observation = self.observe(handle, &group, &expected, None, Placement::Ignore)?;
        observation
            .map(|observation| self.status(observation, &worker.target, &worker.ssh_public_key, Placement::Ignore))
            .transpose()
    }

    /// `Deleted` means ARM accepted (202) or completed the deletion of exactly the owned
    /// group; the group may keep answering `Deleting` for a while afterwards.
    /// Resource groups carry no entity tag, so the ownership proof cannot be made atomic with
    /// the DELETE; it is repeated immediately before the call, and the name itself is
    /// derived from this worker's own identifiers, so a foreign replacement inside the
    /// remaining window would need the same two UUIDs.
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        let Some((handle, _, expected)) = self.owned_handle(worker)? else {
            return Ok(InteractiveWorkerCleanup::AlreadyAbsent);
        };
        match self.owned_group(&handle.resource_group, &expected)? {
            // An earlier accepted deletion is still in flight: nothing more to request.
            Some(current) if current.id.eq_ignore_ascii_case(&handle.group_id) => {
                if current.provisioning_state == "Deleting" {
                    return Ok(InteractiveWorkerCleanup::Deleted);
                }
            }
            Some(_) => return Err(AzureError::ResourceIdentityMismatch),
            None => return Ok(InteractiveWorkerCleanup::AlreadyAbsent),
        }
        match self.transport.delete_resource_group(&handle.resource_group) {
            Ok(_accepted_or_completed) => Ok(InteractiveWorkerCleanup::Deleted),
            // Removed between the ownership check and this call: a retry stays idempotent.
            Err(AzureError::UnexpectedStatus { status: 404, .. }) => Ok(InteractiveWorkerCleanup::AlreadyAbsent),
            Err(error) => Err(error),
        }
    }
}

/// A persisted handle proven against the live group.
type OwnedHandle = (AzureWorker, AzureGroupInfo, BTreeMap<String, String>);

impl AzureClient {
    /// Shape, every identity tag, and the group ID recorded at creation (ARM compares
    /// IDs case-insensitively) must all agree before any mutation.
    fn owned_handle(&self, worker: &InteractiveWorker) -> Result<Option<OwnedHandle>, AzureError> {
        let handle = self.handle(worker)?;
        let expected = identity_tags(
            handle.workflow_id,
            handle.job_id,
            &worker.target,
            &worker.ssh_public_key,
        );
        let Some(group) = self.owned_group(&handle.resource_group, &expected)? else {
            return Ok(None);
        };
        if !group.id.eq_ignore_ascii_case(&handle.group_id) {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        Ok(Some((handle, group, expected)))
    }
}
