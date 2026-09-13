pub(super) use super::super::{
    AzureClient, AzureDataDisk, AzureDeploymentState, AzureError, AzureGroupInfo, AzureLongRunningState,
    AzureManagementTransport, AzureRunCommand, AzureVmView, AzureWorker, SSH_USERNAME,
    deployment::{DEPLOYMENT_NAME, SSH_PORT, TAG_CLIENT_KEY_DIGEST, TAG_IMAGE_REF_DIGEST, TAG_JOB, worker_tags},
    resource_group_name,
};
pub(super) use super::{OTHER_SUB, SUB, ed25519_key, profile, target};
pub(super) use crate::cloud_run::{
    CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, WorkerTarget,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
        InteractiveWorkerLifecycle as Lifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider,
        InteractiveWorkerRequest, InteractiveWorkerStatus,
    },
};
pub(super) use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Simulated time: sleeps advance it instantly and are recorded, and the plane can
/// charge every bounded request against it, so the waits' deadline arithmetic runs
/// exactly as in production without a real second passing.
#[derive(Clone)]
pub(super) struct FakeClock(Arc<Mutex<(Instant, Vec<Duration>)>>);

impl Default for FakeClock {
    fn default() -> Self {
        Self(Arc::new(Mutex::new((Instant::now(), Vec::new()))))
    }
}

impl FakeClock {
    pub(super) fn advance(&self, by: Duration) {
        self.0.lock().expect("clock").0 += by;
    }

    pub(super) fn sleeps(&self) -> Vec<Duration> {
        self.0.lock().expect("clock").1.clone()
    }

    pub(super) fn instant(&self) -> Instant {
        self.0.lock().expect("clock").0
    }
}

impl super::super::provider::AzureClock for FakeClock {
    fn now(&self) -> Instant {
        self.0.lock().expect("clock").0
    }

    fn sleep(&self, duration: Duration) {
        let mut clock = self.0.lock().expect("clock");
        clock.0 += duration;
        clock.1.push(duration);
    }
}

/// Every management call the provider makes, in order, with its arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Call {
    GetGroup(String),
    CreateGroup(String, BTreeMap<String, String>),
    DeleteGroup(String),
    PutDeployment(String, String, serde_json::Value),
    GetDeployment(String),
    GetVm(String),
    Deallocate(String),
    Start(String),
    Run(String, AzureRunCommand),
}

/// What a concurrent actor did to the group while the provider was waiting.
pub(super) enum GroupChange {
    Replaced(AzureGroupInfo),
    Deleted,
}

#[derive(Default)]
pub(super) struct Plane {
    pub(super) group: Option<AzureGroupInfo>,
    pub(super) deployment: Option<AzureDeploymentState>,
    pub(super) vm_states: Vec<Option<AzureVmView>>,
    pub(super) fail_deployment: bool,
    pub(super) submitted_state: Option<&'static str>,
    pub(super) appear_on_second_lookup: Option<AzureGroupInfo>,
    /// Replaces the group after the first lookup, as a concurrent retag or deletion would.
    pub(super) group_after_first_lookup: Option<AzureGroupInfo>,
    pub(super) group_after_create: Option<AzureGroupInfo>,
    pub(super) deployment_on_second_read: Option<AzureDeploymentState>,
    pub(super) vanish_after_first_lookup: bool,
    pub(super) delete_status: Option<u16>,
    pub(super) deallocate: Option<Result<Option<AzureLongRunningState>, AzureError>>,
    pub(super) start: Option<Result<Option<AzureLongRunningState>, AzureError>>,
    pub(super) run_output: Option<String>,
    /// Replaces the group once the deallocation was posted, as a concurrent retag or
    /// deletion during the wait would.
    pub(super) group_after_deallocate: Option<GroupChange>,
    /// Replaces the group once the start was posted, as a concurrent retag or deletion
    /// during the wait would.
    pub(super) group_after_start: Option<GroupChange>,
    /// How long every bounded lookup takes on the scenario's clock.
    pub(super) request_takes: Duration,
    pub(super) clock: Option<FakeClock>,
    /// Every budget a bounded lookup was handed, in order.
    pub(super) budgets: Vec<Duration>,
    pub(super) calls: Vec<Call>,
}

#[derive(Clone, Default)]
pub(super) struct Fake(pub(super) Arc<Mutex<Plane>>);

impl Fake {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Plane> {
        self.0.lock().expect("plane")
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    pub(super) fn mutations(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| !matches!(call, Call::GetGroup(_) | Call::GetDeployment(_) | Call::GetVm(_)))
            .collect()
    }

    /// A bounded request: it must carry what is left of the bound, and it costs the
    /// scenario's request time on the clock (never more than its own budget, as a
    /// bounded request would be cut off there).
    fn charge(&self, budget: Duration) {
        assert!(
            !budget.is_zero() && budget <= Duration::from_secs(300),
            "a poll carries what is left of the bound: {budget:?}"
        );
        let mut plane = self.lock();
        plane.budgets.push(budget);
        if let Some(clock) = &plane.clock {
            clock.advance(plane.request_takes.min(budget));
        }
    }

    pub(super) fn budgets(&self) -> Vec<Duration> {
        self.lock().budgets.clone()
    }

    pub(super) fn script(
        &self,
        group: Option<AzureGroupInfo>,
        deployment: Option<AzureDeploymentState>,
        vm_states: Vec<Option<AzureVmView>>,
    ) {
        let mut plane = self.lock();
        plane.group = group;
        plane.deployment = deployment;
        plane.vm_states = vm_states;
        plane.calls.clear();
        plane.budgets.clear();
    }
}

impl AzureManagementTransport for Fake {
    fn subscription_id(&self) -> &str {
        SUB
    }

    fn get_resource_group(&self, name: &str) -> Result<Option<AzureGroupInfo>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::GetGroup(name.into()));
        let found = plane.group.clone().filter(|group| group.name == name);
        if found.is_none() {
            plane.group = plane.appear_on_second_lookup.take();
        } else if let Some(next) = plane.group_after_first_lookup.take() {
            plane.group = Some(next);
        } else if std::mem::take(&mut plane.vanish_after_first_lookup) {
            plane.group = None;
        }
        Ok(found)
    }

    fn create_resource_group(
        &self,
        name: &str,
        location: &str,
        tags: &BTreeMap<String, String>,
    ) -> Result<AzureGroupInfo, AzureError> {
        let mut plane = self.lock();
        assert_eq!(location, "northeurope");
        plane.calls.push(Call::CreateGroup(name.into(), tags.clone()));
        let info = group_info(name, tags.clone(), "Succeeded");
        plane.group = Some(plane.group_after_create.take().unwrap_or_else(|| info.clone()));
        Ok(info)
    }

    fn delete_resource_group(&self, name: &str) -> Result<AzureLongRunningState, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::DeleteGroup(name.into()));
        if let Some(status) = plane.delete_status {
            return Err(AzureError::UnexpectedStatus {
                operation: "resource group deletion",
                status,
            });
        }
        plane.group = None;
        Ok(AzureLongRunningState::Accepted)
    }

    fn put_deployment(
        &self,
        group: &str,
        name: &str,
        _template: &serde_json::Value,
        parameters: &serde_json::Value,
    ) -> Result<AzureDeploymentState, AzureError> {
        let mut plane = self.lock();
        plane
            .calls
            .push(Call::PutDeployment(group.into(), name.into(), parameters.clone()));
        if plane.fail_deployment {
            return Err(AzureError::UnexpectedStatus {
                operation: "deployment submission",
                status: 429,
            });
        }
        let state = AzureDeploymentState {
            provisioning_state: plane.submitted_state.unwrap_or("Accepted").into(),
            outputs: BTreeMap::new(),
        };
        plane.deployment = Some(state.clone());
        Ok(state)
    }

    fn get_deployment(&self, group: &str, _name: &str) -> Result<Option<AzureDeploymentState>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::GetDeployment(group.into()));
        let current = plane.deployment.clone();
        if let Some(next) = plane.deployment_on_second_read.take() {
            plane.deployment = Some(next);
        }
        Ok(current)
    }

    fn get_vm(&self, group: &str, _name: &str) -> Result<Option<AzureVmView>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::GetVm(group.into()));
        Ok(if plane.vm_states.len() > 1 {
            plane.vm_states.remove(0)
        } else {
            plane.vm_states.first().cloned().flatten()
        })
    }

    fn get_resource_group_within(&self, name: &str, budget: Duration) -> Result<Option<AzureGroupInfo>, AzureError> {
        self.charge(budget);
        self.get_resource_group(name)
    }

    fn get_vm_within(&self, group: &str, name: &str, budget: Duration) -> Result<Option<AzureVmView>, AzureError> {
        self.charge(budget);
        self.get_vm(group, name)
    }

    fn start_vm(&self, group: &str, _name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::Start(group.into()));
        if let Some(change) = plane.group_after_start.take() {
            plane.group = match change {
                GroupChange::Replaced(group) => Some(group),
                GroupChange::Deleted => None,
            };
        }
        plane.start.clone().unwrap_or(Ok(Some(AzureLongRunningState::Accepted)))
    }

    fn deallocate_vm(&self, group: &str, _name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::Deallocate(group.into()));
        if let Some(change) = plane.group_after_deallocate.take() {
            plane.group = match change {
                GroupChange::Replaced(group) => Some(group),
                GroupChange::Deleted => None,
            };
        }
        plane
            .deallocate
            .clone()
            .unwrap_or(Ok(Some(AzureLongRunningState::Accepted)))
    }

    fn run_command(&self, group: &str, _name: &str, command: AzureRunCommand) -> Result<Option<String>, AzureError> {
        let mut plane = self.lock();
        plane.calls.push(Call::Run(group.into(), command));
        Ok(plane.run_output.clone())
    }
}

pub(super) fn group_info(name: &str, tags: BTreeMap<String, String>, state: &str) -> AzureGroupInfo {
    AzureGroupInfo {
        id: format!("/subscriptions/{SUB}/resourceGroups/{name}"),
        name: name.into(),
        location: "northeurope".into(),
        provisioning_state: state.into(),
        tags,
    }
}

/// The instance identity every scripted VM carries unless a test replaces the instance.
pub(super) const INSTANCE: &str = "3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f";

pub(super) fn vm(power: &str, tags: &BTreeMap<String, String>) -> AzureVmView {
    AzureVmView {
        id: format!("/subscriptions/{SUB}/resourceGroups/g/providers/Microsoft.Compute/virtualMachines/worker"),
        instance_id: Some(INSTANCE.into()),
        name: "worker".into(),
        location: "northeurope".into(),
        vm_size: "Standard_D4s_v3".into(),
        provisioning_state: "Succeeded".into(),
        power_state: Some(format!("PowerState/{power}")),
        tags: tags.clone(),
        data_disks: vec![retained_disk()],
    }
}

/// The retained data disk as the deployment attaches it.
pub(super) fn retained_disk() -> AzureDataDisk {
    AzureDataDisk {
        id: format!("/subscriptions/{SUB}/resourceGroups/g/providers/Microsoft.Compute/disks/worker-data"),
        lun: Some(0),
        delete_option: "Detach".into(),
    }
}

pub(super) fn deployment(state: &str, host: &str) -> AzureDeploymentState {
    AzureDeploymentState {
        provisioning_state: state.into(),
        outputs: [("publicIp".to_string(), host.to_string())].into(),
    }
}

pub(super) struct Scenario {
    pub(super) clock: FakeClock,
    pub(super) request: InteractiveWorkerRequest,
    pub(super) group: String,
    pub(super) group_id: String,
    pub(super) tags: BTreeMap<String, String>,
    pub(super) plane: Fake,
}

impl Scenario {
    pub(super) fn new() -> Self {
        let request = InteractiveWorkerRequest {
            workflow_id: CloudWorkflowId::new(),
            job_id: CloudJobId::new(),
            target: target(),
            ssh_public_key: ed25519_key(7, "client"),
        };
        let group = resource_group_name(request.workflow_id, request.job_id);
        let clock = FakeClock::default();
        let plane = Fake::default();
        plane.lock().clock = Some(clock.clone());
        Self {
            clock,
            group_id: format!("/subscriptions/{SUB}/resourceGroups/{group}"),
            group,
            tags: worker_tags(&request),
            plane,
            request,
        }
    }

    /// A client whose fence answers `claim` and whose host-key source answers `host_key`
    /// only when asked about this exact worker and client key.
    pub(super) fn client(&self, claim: bool, host_key: Option<String>) -> AzureClient {
        let (expected, group) = (self.persisted(), self.group.clone());
        let fence = move |w: CloudWorkflowId, j: CloudJobId, t: &WorkerTarget, g: &str| {
            assert_eq!(
                (w, j, t, g),
                (
                    expected.identity.workflow_id,
                    expected.identity.job_id,
                    &expected.target,
                    group.as_str()
                )
            );
            Ok(claim)
        };
        let (expected, group) = (self.persisted(), self.group.clone());
        let keys = move |worker: &AzureWorker, _: &str, key: &str| {
            assert_eq!(
                (worker.resource_group.as_str(), key),
                (group.as_str(), expected.ssh_public_key.as_str())
            );
            host_key.clone()
        };
        AzureClient::with_transport(profile(), self.plane.clone(), fence, keys)
            .expect("client")
            .on_clock(self.clock.clone())
    }

    pub(super) fn persisted(&self) -> InteractiveWorker {
        InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::Azure,
                workflow_id: self.request.workflow_id,
                job_id: self.request.job_id,
                resource_id: self.group_id.clone(),
            },
            target: self.request.target.clone(),
            ssh_public_key: self.request.ssh_public_key.clone(),
            lifetime: InteractiveWorkerLifetime::Persistent,
        }
    }

    pub(super) fn reconcile(&self, host_key: Option<String>) -> InteractiveWorkerStatus {
        self.client(false, host_key)
            .reconcile_worker(&self.request)
            .expect("reconcile")
            .expect("present")
    }
}

pub(super) fn host_key() -> String {
    ed25519_key(9, "")
}

pub(super) fn owned(s: &Scenario) -> AzureGroupInfo {
    group_info(&s.group, s.tags.clone(), "Succeeded")
}

pub(super) const MISMATCH: AzureError = AzureError::ResourceIdentityMismatch;

mod creation;
mod instance;
mod observation;
mod running;
mod start;
mod stop_observer;
mod wait;
