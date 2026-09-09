//! Exact-Pod Stop with ordinary volume retention, separate from termination.
//! Provider metadata proves only point-in-time retained/inactive state, not a backup,
//! successful task exit, preserved process memory, or qualified filesystem durability.

use super::{
    ApiPod, InteractiveWorkerLifetime, RunPodClient, RunPodError, RunPodLifecycle, RunPodProfile, RunPodWorker,
    status_from_resource, valid_ssh_public_key,
};
use crate::cloud_run::interactive_worker_stop::InteractiveWorkerStop;
use serde::Deserialize;
use serde_json::Value;

/// Optional metadata is ignored by existing lifecycle reads; Stop requires positive proof.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct StopMetadata {
    mounts: Value,
    actions: Value,
    locked: Value,
    cloud: Value,
    cluster: Value,
    runtime: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mounts {
    persistent: PersistentMount,
}

#[derive(Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistentMount {
    size: u32,
    path: String,
}

struct RetainedState {
    mount: PersistentMount,
    stopped: bool,
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
        worker.validate()?;
        if worker.lifetime != InteractiveWorkerLifetime::Persistent {
            return Err(RunPodError::StopUnsupportedLifetime);
        }
        if !valid_ssh_public_key(ssh_public_key) {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        if profile.volume_gib == 0 {
            return Err(RunPodError::StopRetentionUnverified);
        }
        let Some(before) = self.transport.get(&worker.pod_id)? else {
            return Ok(InteractiveWorkerStop::AlreadyAbsent);
        };
        let before = retained_state(&before, worker, ssh_public_key, profile)?;
        if before.stopped {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        let stopping = self.transport.stop(&worker.pod_id);
        let after = self
            .transport
            .get(&worker.pod_id)?
            .ok_or(RunPodError::StopResourceLost)?;
        let after = retained_state(&after, worker, ssh_public_key, profile)?;
        if before.mount != after.mount {
            return Err(RunPodError::StopRetentionUnverified);
        }
        if after.stopped {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        stopping?;
        Err(RunPodError::StopVerificationFailed)
    }
}

fn retained_state(
    pod: &ApiPod,
    worker: &RunPodWorker,
    ssh_public_key: &str,
    profile: &RunPodProfile,
) -> Result<RetainedState, RunPodError> {
    let status = status_from_resource(pod, worker, Some(ssh_public_key))?;
    let metadata = &pod.stop;
    let mounts: Mounts =
        serde_json::from_value(metadata.mounts.clone()).map_err(|_| RunPodError::StopRetentionUnverified)?;
    if metadata.cloud != "SECURE"
        || !metadata.cluster.is_null()
        || mounts.persistent.path != "/workspace"
        || mounts.persistent.size < profile.volume_gib
    {
        return Err(RunPodError::StopRetentionUnverified);
    }
    let stopped = match status.lifecycle {
        RunPodLifecycle::Exited if metadata.runtime.is_null() => true,
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
    Ok(RetainedState {
        mount: mounts.persistent,
        stopped,
    })
}
