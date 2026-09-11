pub(super) use super::super::{
    AzureClient, AzureDeploymentState, AzureError, AzureGroupInfo, AzureLongRunningState, AzureManagementTransport,
    AzureVmView,
    deployment::{DEPLOYMENT_NAME, TAG_CLIENT_KEY_DIGEST, TAG_JOB, worker_tags},
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
};

/// Every management call the provider makes, in order, with its arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Call {
    GetGroup(String),
    CreateGroup(String, BTreeMap<String, String>),
    DeleteGroup(String),
    PutDeployment(String, String, serde_json::Value),
    GetDeployment(String),
    GetVm(String),
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

pub(super) fn vm(power: &str, tags: &BTreeMap<String, String>) -> AzureVmView {
    AzureVmView {
        id: format!("/subscriptions/{SUB}/resourceGroups/g/providers/Microsoft.Compute/virtualMachines/worker"),
        name: "worker".into(),
        location: "northeurope".into(),
        vm_size: "Standard_D4s_v3".into(),
        provisioning_state: "Succeeded".into(),
        power_state: Some(format!("PowerState/{power}")),
        tags: tags.clone(),
    }
}

pub(super) fn deployment(state: &str, host: &str) -> AzureDeploymentState {
    AzureDeploymentState {
        provisioning_state: state.into(),
        outputs: [("publicIp".to_string(), host.to_string())].into(),
    }
}

pub(super) struct Scenario {
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
        Self {
            group_id: format!("/subscriptions/{SUB}/resourceGroups/{group}"),
            group,
            tags: worker_tags(&request),
            plane: Fake::default(),
            request,
        }
    }

    /// A client whose fence answers `claim` for exactly this request.
    pub(super) fn client(&self, claim: bool) -> AzureClient {
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
        AzureClient::with_transport(profile(), self.plane.clone(), fence).expect("client")
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

    pub(super) fn reconcile(&self) -> InteractiveWorkerStatus {
        self.client(false)
            .reconcile_worker(&self.request)
            .expect("reconcile")
            .expect("present")
    }
}

pub(super) fn owned(s: &Scenario) -> AzureGroupInfo {
    group_info(&s.group, s.tags.clone(), "Succeeded")
}

pub(super) const MISMATCH: AzureError = AzureError::ResourceIdentityMismatch;

mod creation;
mod observation;
