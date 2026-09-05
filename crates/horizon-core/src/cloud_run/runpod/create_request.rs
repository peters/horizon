use super::super::CLOUD_RUN_PROTOCOL_VERSION;
use super::{
    CloudJobId, CloudWorkflowId, JOB_ENV, PROTOCOL_ENV, RunPodError, RunPodProfile, SSH_PUBLIC_KEY_ENV, TERMINATE_ENV,
    WORKFLOW_ENV, WorkerTarget, termination_deadline,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreatePodRequest {
    pub(super) allowed_cuda_versions: Vec<String>,
    pub(super) cloud_type: &'static str,
    pub(super) container_disk_in_gb: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) container_registry_auth_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) data_center_id: Option<String>,
    pub(super) env: Vec<CreatePodEnv>,
    pub(super) gpu_count: u16,
    #[serde(rename = "gpuTypeIdList")]
    pub(super) gpu_type_ids: Vec<String>,
    pub(super) image_name: String,
    #[serde(rename = "minDisk", skip_serializing_if = "Option::is_none")]
    pub(super) min_disk_bandwidth_mbps: Option<u32>,
    #[serde(rename = "minDownload", skip_serializing_if = "Option::is_none")]
    pub(super) min_download_mbps: Option<u32>,
    #[serde(rename = "minUpload", skip_serializing_if = "Option::is_none")]
    pub(super) min_upload_mbps: Option<u32>,
    pub(super) name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) ports: String,
    pub(super) start_ssh: bool,
    pub(super) support_public_ip: bool,
    pub(super) terminate_after: String,
    pub(super) volume_in_gb: u32,
    pub(super) volume_mount_path: &'static str,
}
#[derive(Clone, Debug, Serialize)]
pub(super) struct CreatePodEnv {
    pub(super) key: String,
    pub(super) value: String,
}
impl CreatePodRequest {
    pub(super) fn new(
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
        name: String,
        ssh_public_key: Option<&str>,
    ) -> Result<Self, RunPodError> {
        let seconds = target.lifetime.time_limit_seconds().ok_or(RunPodError::InvalidTarget)?;
        let terminate_after = termination_deadline(seconds)?;
        let mut env: Vec<_> = [
            (WORKFLOW_ENV, workflow_id.to_string()),
            (JOB_ENV, job_id.to_string()),
            (PROTOCOL_ENV, CLOUD_RUN_PROTOCOL_VERSION.to_string()),
            (TERMINATE_ENV, terminate_after.clone()),
        ]
        .into_iter()
        .map(|(key, value)| CreatePodEnv {
            key: key.to_string(),
            value,
        })
        .collect();
        if let Some(ssh_public_key) = ssh_public_key {
            env.push(CreatePodEnv {
                key: SSH_PUBLIC_KEY_ENV.to_string(),
                value: ssh_public_key.to_string(),
            });
        }
        Ok(Self {
            allowed_cuda_versions: profile.allowed_cuda_versions.clone(),
            cloud_type: "SECURE",
            container_disk_in_gb: target.disk_gib,
            container_registry_auth_id: profile.container_registry_auth_id.clone(),
            data_center_id: profile.data_center_id.clone(),
            env,
            gpu_count: profile.gpu_count,
            gpu_type_ids: profile.gpu_type_ids.clone(),
            image_name: target.image.clone(),
            min_disk_bandwidth_mbps: profile.min_disk_bandwidth_mbps,
            min_download_mbps: profile.min_download_mbps,
            min_upload_mbps: profile.min_upload_mbps,
            name,
            ports: profile.ports.join(","),
            start_ssh: true,
            support_public_ip: true,
            terminate_after,
            volume_in_gb: profile.volume_gib,
            volume_mount_path: "/workspace",
        })
    }
}
