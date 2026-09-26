//! Ordered, single-type v2 requests; only a definite refusal permits a fallback.
use super::{RunPod, json, volumes::Volume};
use crate::{Cancellation, CloudError, CreateState, Progress, WorkerSpec};
use serde_json::Value;

impl RunPod {
    pub(super) fn request_worker(
        &self,
        spec: &WorkerSpec,
        volume: Option<&Volume>,
        state: &mut CreateState,
        cancel: &Cancellation,
        persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
        progress: &mut impl FnMut(Progress),
    ) -> Result<Value, CloudError> {
        let candidates = if spec.profile.gpu {
            &spec.gpu_types
        } else {
            &spec.cpu_flavors
        };
        let mut last = CloudError::Invalid("No configured compute candidates");
        for candidate in candidates {
            cancel.check()?;
            let mut body = create_body(spec);
            body[if spec.profile.gpu { "gpu" } else { "cpu" }]["id"] = json!(candidate);
            if let Some(volume) = volume {
                body["mounts"] = json!({"network":[{"volumeId":volume.id,"path":"/workspace"}]});
                body["dataCenterIds"] = json!([volume.data_center_id]);
            }
            persist(&CreateState::Requested)?;
            *state = CreateState::Requested;
            progress(Progress::Requesting);
            match self.request("POST", "/pods", Some(body), cancel) {
                Ok(value) => return Ok(value),
                Err(error @ (CloudError::Rejected(_) | CloudError::Http(403, _))) => {
                    persist(&CreateState::Prepared)?;
                    *state = CreateState::Prepared;
                    last = error;
                }
                Err(error @ (CloudError::Unauthorized | CloudError::Cancelled | CloudError::Http(422, _))) => {
                    persist(&CreateState::Prepared)?;
                    *state = CreateState::Prepared;
                    return Err(error);
                }
                // A lost or malformed success, rate limit, or server error never
                // authorizes the next candidate: the first request may exist.
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }
}

pub(super) fn create_body(spec: &WorkerSpec) -> Value {
    let profile = &spec.profile;
    let mut body = json!({
        "name":spec.name(), "image":spec.image_digest, "cloud":"SECURE",
        "disk":profile.storage.container_gb, "ports":["22/tcp"],
        "env":{"PUBLIC_KEY":spec.public_key,"HORIZON_CLOUD_OPERATION":spec.operation_id,
               "HORIZON_WORKER_CAPABILITIES":json!(profile.capabilities).to_string()},
    });
    if let Some(metadata) = &spec.startup_metadata {
        body["env"][crate::startup::ENVIRONMENT_KEY] = json!(metadata.as_str());
    }
    if let Some(minutes) = spec.idle_stop_environment() {
        body["env"][crate::IDLE_STOP_ENVIRONMENT_KEY] = json!(minutes);
    }
    if profile.gpu {
        body["gpu"] = json!({"id":spec.gpu_types.first(), "count":1,
                            "minVcpuCountPerGpu":profile.cpu, "minRamPerGpu":profile.memory_gb});
        body["mounts"] = json!({"persistent":{"size":profile.storage.volume_gb,"path":"/workspace"}});
    } else {
        body["cpu"] = json!({"id":spec.cpu_flavors.first(),"vcpuCount":profile.cpu});
    }
    if !spec.data_centers.is_empty() {
        body["dataCenterIds"] = json!(spec.data_centers);
    }
    if let Some(id) = &spec.registry_auth_id {
        body["registry"] = json!(id);
    }
    body
}
