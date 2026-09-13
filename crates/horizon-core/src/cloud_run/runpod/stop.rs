//! Exact-Pod Stop with verified volume retention, separate from termination.
//! Provider metadata proves only point-in-time retained/inactive state, not a backup,
//! successful task exit, preserved process memory, or qualified filesystem durability.

use super::{
    ApiPod, InteractiveWorkerLifetime, RunPodClient, RunPodError, RunPodLifecycle, RunPodProfile, RunPodWorker,
    status_from_resource, valid_ssh_public_key,
};
use crate::cloud_run::interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopObservation};
use serde::Deserialize;
use serde_json::Value;

/// Optional metadata is ignored by existing lifecycle reads; Stop requires positive proof.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct StopMetadata {
    mounts: Value,
    pub(super) actions: Value,
    pub(super) locked: Value,
    pub(super) cloud: Value,
    pub(super) cluster: Value,
    #[serde(default, deserialize_with = "present_runtime")]
    pub(super) runtime: Option<Value>,
    #[serde(rename = "dataCenterId")]
    data_center_id: Value,
}

fn present_runtime<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl StopMetadata {
    pub(super) fn mounts(&self) -> &Value {
        &self.mounts
    }

    pub(super) fn is_secure_data_center(&self, expected: &str) -> bool {
        self.cloud == "SECURE" && self.data_center_id.as_str() == Some(expected)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mounts {
    persistent: PersistentMount,
}

#[derive(Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PersistentMount {
    size: u32,
    path: String,
}

struct RetainedState {
    mount: RetainedMount,
    stopped: bool,
}

#[derive(Eq, PartialEq)]
pub(super) enum RetainedMount {
    Ordinary(PersistentMount),
    SelectedNetwork,
}

impl RunPodClient {
    /// One exact mutation, then a fresh observation even if its response was lost.
    /// No implicit polling/retry, host-key access, restart or compensating deletion.
    pub(super) fn stop_interactive_worker(
        &self,
        worker: &RunPodWorker,
        ssh_public_key: &str,
        profile: &RunPodProfile,
    ) -> Result<InteractiveWorkerStop, RunPodError> {
        self.validate_stop_request(worker, ssh_public_key, profile)?;
        let Some(before) = self.transport.get(&worker.pod_id)? else {
            return Ok(InteractiveWorkerStop::AlreadyAbsent);
        };
        let before = self.retained_state(&before, worker, ssh_public_key, profile)?;
        if before.stopped {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        let stopping = self.transport.stop(&worker.pod_id);
        let after = self
            .transport
            .get(&worker.pod_id)?
            .ok_or(RunPodError::StopResourceLost)?;
        let after = self.retained_state(&after, worker, ssh_public_key, profile)?;
        if before.mount != after.mount {
            return Err(RunPodError::StopRetentionUnverified);
        }
        if after.stopped {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        stopping?;
        Err(RunPodError::StopVerificationFailed)
    }

    pub(super) fn observe_interactive_worker_stop(
        &self,
        worker: &RunPodWorker,
        ssh_public_key: &str,
        profile: &RunPodProfile,
    ) -> Result<InteractiveWorkerStopObservation, RunPodError> {
        self.validate_stop_request(worker, ssh_public_key, profile)?;
        let Some(pod) = self.transport.get(&worker.pod_id)? else {
            return Ok(InteractiveWorkerStopObservation::Absent);
        };
        let retained = self.retained_state(&pod, worker, ssh_public_key, profile)?;
        Ok(if retained.stopped {
            InteractiveWorkerStopObservation::RetainedStopped
        } else {
            InteractiveWorkerStopObservation::Pending
        })
    }

    fn validate_stop_request(
        &self,
        worker: &RunPodWorker,
        ssh_public_key: &str,
        profile: &RunPodProfile,
    ) -> Result<(), RunPodError> {
        worker.validate()?;
        if worker.lifetime != InteractiveWorkerLifetime::Persistent {
            return Err(RunPodError::StopUnsupportedLifetime);
        }
        if !valid_ssh_public_key(ssh_public_key) {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        if self.network_binding.is_none() && profile.volume_gib == 0 {
            return Err(RunPodError::StopRetentionUnverified);
        }
        Ok(())
    }

    fn retained_state(
        &self,
        pod: &ApiPod,
        worker: &RunPodWorker,
        ssh_public_key: &str,
        profile: &RunPodProfile,
    ) -> Result<RetainedState, RunPodError> {
        let status = status_from_resource(pod, worker, Some(ssh_public_key))?;
        let metadata = &pod.stop;
        let mount = self.retained_mount(pod, profile)?;
        let stopped = match status.lifecycle {
            RunPodLifecycle::Exited if metadata.runtime.as_ref().is_none_or(Value::is_null) => true,
            RunPodLifecycle::Provisioning | RunPodLifecycle::Running
                if metadata.locked == false
                    && metadata
                        .actions
                        .as_array()
                        .is_some_and(|actions| actions.iter().any(|value| value == "stop")) =>
            {
                false
            }
            _ => return Err(RunPodError::StopStateUnverified),
        };
        Ok(RetainedState { mount, stopped })
    }

    pub(super) fn retained_mount(&self, pod: &ApiPod, profile: &RunPodProfile) -> Result<RetainedMount, RunPodError> {
        let metadata = &pod.stop;
        if metadata.cloud != "SECURE" || !metadata.cluster.is_null() {
            return Err(RunPodError::StopRetentionUnverified);
        }
        Ok(if self.network_binding.is_some() {
            self.verify_selected_volume()?;
            self.verify_attachment(pod)?;
            RetainedMount::SelectedNetwork
        } else {
            let mounts: Mounts =
                serde_json::from_value(metadata.mounts.clone()).map_err(|_| RunPodError::StopRetentionUnverified)?;
            if mounts.persistent.path != "/workspace" || mounts.persistent.size < profile.volume_gib {
                return Err(RunPodError::StopRetentionUnverified);
            }
            RetainedMount::Ordinary(mounts.persistent)
        })
    }
}
