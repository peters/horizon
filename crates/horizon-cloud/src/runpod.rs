//! `RunPod` REST adapter. Never repeats a create request after an uncertain response.
use crate::{Cancellation, CloudError, CreateState, Credential, Progress, Worker, WorkerSpec, valid_id};
use serde_json::{Value, json};
use std::time::Duration;

pub mod flavors;
pub mod recovery;
pub mod volumes;

#[cfg(test)]
mod tests;

pub struct RunPod {
    agent: ureq::Agent,
    credential: Credential,
    endpoint: String,
    catalog_endpoint: String,
    api_endpoint: String,
}
impl RunPod {
    #[must_use]
    pub fn new(credential: Credential) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            credential,
            endpoint: "https://rest.runpod.io/v1".into(),
            catalog_endpoint: "https://api.runpod.io/v2/catalog".into(),
            api_endpoint: "https://api.runpod.io/v2".into(),
        }
    }

    /// The caller must hold its operation lock throughout this call. `persist`
    /// must durably commit each transition before returning. A Requested operation
    /// can only reconcile; an empty list never permits a second POST.
    /// # Errors
    /// Reports cancellation, ambiguous creation, missing workers and identity conflicts.
    pub fn ensure(
        &self,
        spec: &WorkerSpec,
        state: &mut CreateState,
        cancel: &Cancellation,
        persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        progress: impl FnMut(Progress),
    ) -> Result<Worker, CloudError> {
        self.ensure_with_volume(spec, state, None, cancel, persist, progress)
    }

    /// As `ensure`, with an explicitly owned and verified workspace volume.
    /// # Errors
    /// Refuses incompatible storage and retains uncertain worker creation fences.
    pub fn ensure_with_volume(
        &self,
        spec: &WorkerSpec,
        state: &mut CreateState,
        volume: Option<&volumes::Volume>,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
    ) -> Result<Worker, CloudError> {
        spec.validate()?;
        if let Some(volume) = volume {
            volume.verify_worker_spec(spec)?;
        }
        cancel.check()?;
        progress(Progress::Reconciling);
        if *state == CreateState::Requested {
            let recovered = self.reconcile(spec, state, None, cancel, &mut persist)?;
            return match recovered.outcome {
                recovery::Outcome::Found { worker_id } => {
                    progress(Progress::WorkerFound(worker_id));
                    recovered.worker.ok_or(CloudError::InvalidResponse)
                }
                recovery::Outcome::Conflicting { .. } => Err(CloudError::DuplicateWorkers),
                recovery::Outcome::Inactive { .. } => Err(CloudError::Invalid(
                    "Existing worker is not running; check provider before reconnecting",
                )),
                recovery::Outcome::Missing { .. } | recovery::Outcome::Terminated { .. } => Err(CloudError::WorkerLost),
                recovery::Outcome::Prepared | recovery::Outcome::Unresolved => Err(CloudError::CreationUnresolved),
            };
        }
        match state {
            CreateState::Bound { worker_id } => {
                let worker = self.inspect(worker_id, cancel)?.ok_or(CloudError::WorkerLost)?;
                worker.verify(spec)?;
                if worker.desired_status != "RUNNING" {
                    return Err(CloudError::Invalid(
                        "Existing worker is not running; check provider before reconnecting",
                    ));
                }
                return Ok(worker);
            }
            CreateState::Terminated { .. } => return Err(CloudError::WorkerLost),
            CreateState::Prepared | CreateState::Requested => {}
        }
        spec.validate_request()?;
        let workers: Vec<Worker> = self
            .list(cancel)?
            .into_iter()
            .filter(|w| w.name == spec.name())
            .collect();
        if workers.len() > 1 {
            return Err(CloudError::DuplicateWorkers);
        }
        if let Some(worker) = workers.into_iter().next() {
            worker.verify(spec)?;
            bind(state, &worker, &mut persist)?;
            progress(Progress::WorkerFound(worker.id.clone()));
            return Ok(worker);
        }
        cancel.check()?;
        persist(&CreateState::Requested)?;
        *state = CreateState::Requested;
        progress(Progress::Requesting);
        let mut body = create_body(spec);
        if let Some(volume) = volume {
            body["networkVolumeId"] = json!(volume.id);
            body["volumeInGb"] = json!(0);
            body["dataCenterIds"] = json!([volume.data_center_id]);
            body["dataCenterPriority"] = json!("custom");
        }
        let value = match self.request("POST", "/pods", Some(body), cancel) {
            Ok(value) => value,
            Err(error @ (CloudError::Unauthorized | CloudError::Rejected | CloudError::Cancelled)) => {
                persist(&CreateState::Prepared)?;
                *state = CreateState::Prepared;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let worker: Worker = serde_json::from_value(value).map_err(|_| CloudError::CreationUnresolved)?;
        worker.verify(spec)?;
        bind(state, &worker, &mut persist)?;
        progress(Progress::WorkerFound(worker.id.clone()));
        Ok(worker)
    }
    /// # Errors
    /// Returns transport, authentication or response errors without response-body contents.
    pub fn list(&self, cancel: &Cancellation) -> Result<Vec<Worker>, CloudError> {
        serde_json::from_value(self.request(
            "GET",
            "/pods?includeNetworkVolume=true&includeWorkers=true",
            None,
            cancel,
        )?)
        .map_err(|_| CloudError::InvalidResponse)
    }
    /// # Errors
    /// Rejects invalid IDs and provider failures. HTTP 404 is a missing worker.
    pub fn inspect(&self, id: &str, cancel: &Cancellation) -> Result<Option<Worker>, CloudError> {
        self.inspect_bounded(id, cancel, None)
    }
    /// # Errors
    /// As `inspect`, with a caller budget covering response headers and body.
    pub fn inspect_with_timeout(
        &self,
        id: &str,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<Option<Worker>, CloudError> {
        cancel.check()?;
        if timeout.is_zero() {
            return Err(CloudError::Transport);
        }
        self.inspect_bounded(id, cancel, Some(timeout.min(Duration::from_secs(30))))
    }
    fn inspect_bounded(
        &self,
        id: &str,
        cancel: &Cancellation,
        timeout: Option<Duration>,
    ) -> Result<Option<Worker>, CloudError> {
        if !valid_id(id) {
            return Err(CloudError::Invalid("Invalid worker ID"));
        }
        match self.request_with_timeout(
            "GET",
            &format!("/pods/{id}?includeNetworkVolume=true"),
            None,
            cancel,
            timeout,
        ) {
            Err(CloudError::Http(404)) => Ok(None),
            result => {
                let worker: Worker = serde_json::from_value(result?).map_err(|_| CloudError::InvalidResponse)?;
                if worker.id != id {
                    return Err(CloudError::IdentityMismatch);
                }
                Ok(Some(worker))
            }
        }
    }
    /// Explicitly terminates only the worker recorded for this operation. A lost
    /// DELETE response is reconciled by another inspect; creation stays fenced.
    /// # Errors
    /// Refuses unbound or mismatching workers and reports unsuccessful deletion.
    pub fn terminate(
        &self,
        spec: &WorkerSpec,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        let id = match state {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            _ => return Err(CloudError::CreationUnresolved),
        };
        if let Some(worker) = self.inspect(&id, cancel)? {
            worker.verify(spec)?;
            match self.request("DELETE", &format!("/pods/{id}"), None, cancel) {
                Ok(_) | Err(CloudError::Http(404)) => {}
                Err(e) => return Err(e),
            }
            if self.inspect(&id, cancel)?.is_some() {
                return Err(CloudError::Invalid("Termination pending; reconcile again"));
            }
        }
        let next = CreateState::Terminated { worker_id: id };
        persist(&next)?;
        *state = next;
        Ok(())
    }
    /// # Errors
    /// Stops an explicitly selected bound worker; storage may remain billable.
    pub fn stop(&self, spec: &WorkerSpec, id: &str, cancel: &Cancellation) -> Result<(), CloudError> {
        self.inspect(id, cancel)?.ok_or(CloudError::WorkerLost)?.verify(spec)?;
        self.request("POST", &format!("/pods/{id}/stop"), Some(json!({})), cancel)?;
        Ok(())
    }
    /// # Errors
    /// Resumes only the same identity-checked worker. This does not restore processes.
    pub fn start(&self, spec: &WorkerSpec, id: &str, cancel: &Cancellation) -> Result<(), CloudError> {
        self.inspect(id, cancel)?.ok_or(CloudError::WorkerLost)?.verify(spec)?;
        self.request("POST", &format!("/pods/{id}/start"), Some(json!({})), cancel)?;
        Ok(())
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        cancel: &Cancellation,
    ) -> Result<Value, CloudError> {
        self.request_with_timeout(method, path, body, cancel, None)
    }
    fn request_with_timeout(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        cancel: &Cancellation,
        timeout: Option<Duration>,
    ) -> Result<Value, CloudError> {
        cancel.check()?;
        let url = format!("{}{path}", self.endpoint);
        self.request_url(method, &url, body, cancel, timeout)
    }
    fn request_url(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
        cancel: &Cancellation,
        timeout: Option<Duration>,
    ) -> Result<Value, CloudError> {
        cancel.check()?;
        let auth = zeroize::Zeroizing::new(format!("Bearer {}", self.credential.value()));
        let response = match method {
            "POST" => self
                .agent
                .post(url)
                .header("Authorization", auth.as_str())
                .send_json(body.unwrap_or(Value::Null)),
            "DELETE" => self.agent.delete(url).header("Authorization", auth.as_str()).call(),
            _ => {
                let request = self.agent.get(url).header("Authorization", auth.as_str());
                if let Some(timeout) = timeout {
                    request.config().timeout_global(Some(timeout)).build().call()
                } else {
                    request.call()
                }
            }
        };
        let mut response = response.map_err(|_| CloudError::Transport)?;
        let status = response.status().as_u16();
        match status {
            204 => return Ok(Value::Null),
            200..=299 => {}
            401 | 403 => return Err(CloudError::Unauthorized),
            400 | 422 => return Err(CloudError::Rejected),
            _ => return Err(CloudError::Http(status)),
        }
        if method == "DELETE" || url.ends_with("/stop") || url.ends_with("/start") {
            return Ok(Value::Null);
        }
        response
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_json()
            .map_err(|_| CloudError::InvalidResponse)
    }
}
fn bind(
    state: &mut CreateState,
    worker: &Worker,
    persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    let next = CreateState::Bound {
        worker_id: worker.id.clone(),
    };
    persist(&next)?;
    *state = next;
    Ok(())
}
fn create_body(spec: &WorkerSpec) -> Value {
    let profile = &spec.profile;
    let mut body = json!({
        "name":spec.name(),"imageName":spec.image_digest,"cloudType":"SECURE",
        "computeType":if profile.gpu {"GPU"} else {"CPU"},
        "containerDiskInGb":profile.storage.container_gb,"volumeInGb":profile.storage.volume_gb,
        "volumeMountPath":"/workspace","ports":["22/tcp"],"supportPublicIp":true,
        "env":{"PUBLIC_KEY":spec.public_key,"HORIZON_CLOUD_OPERATION":spec.operation_id,"HORIZON_WORKER_CAPABILITIES":json!(profile.capabilities).to_string()},
        "interruptible":false,
    });
    if profile.gpu {
        body["gpuCount"] = json!(1);
        body["gpuTypeIds"] = json!(spec.gpu_types);
        body["gpuTypePriority"] = json!("custom");
        body["minVCPUPerGPU"] = json!(profile.cpu);
        body["minRAMPerGPU"] = json!(profile.memory_gb);
    } else {
        body["vcpuCount"] = json!(profile.cpu);
        body["cpuFlavorIds"] = json!(spec.cpu_flavors);
        body["cpuFlavorPriority"] = json!("custom");
    }
    if !spec.data_centers.is_empty() {
        body["dataCenterIds"] = json!(spec.data_centers);
        body["dataCenterPriority"] = json!("custom");
    }
    if let Some(id) = &spec.registry_auth_id {
        body["containerRegistryAuthId"] = json!(id);
    }
    body
}
