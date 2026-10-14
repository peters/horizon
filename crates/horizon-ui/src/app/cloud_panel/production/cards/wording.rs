//! What stopping and deleting a cloud costs, said as its provider does it: `RunPod`
//! keeps the stopped worker, while Hetzner deletes the server and keeps the
//! workspace volume and the cloud's SSH key, which costs nothing (see
//! `provider::StoppedCost`).
use super::super::Runtime;
use horizon_core::cloud_runtime::provider::{self, StoppedCost};

/// How the cloud's provider stops it; a cloud without a known provider is a `RunPod` one.
fn stopped(runtime: &Runtime) -> StoppedCost {
    runtime
        .state
        .as_ref()
        .and_then(|state| provider::by_id(&state.profile.provider))
        .map_or(StoppedCost::WorkerKept, |provider| provider.stopped)
}

/// Whether the cloud runs on a saved Docker host, where nothing is billed.
pub(in crate::app::cloud_panel::production) fn on_docker_host(runtime: &Runtime) -> bool {
    runtime
        .state
        .as_ref()
        .is_some_and(|state| state.profile.provider == provider::DOCKER_HOST.id)
}

pub(super) fn stop_confirmation(runtime: &Runtime) -> &'static str {
    if on_docker_host(runtime) {
        return "Stop this worker? Running processes will end. Its container and volume stay on the Docker host.";
    }
    match stopped(runtime) {
        StoppedCost::WorkerKept => "Stop this worker? Running processes will end. Storage remains billable.",
        StoppedCost::ServerDeleted => {
            "Stop this cloud? Running processes will end and its server is deleted. The workspace volume is kept and stays billable."
        }
    }
}

pub(super) fn stopped_note(runtime: &Runtime) -> &'static str {
    if on_docker_host(runtime) {
        return "Stopped. The container and its volume stay on the Docker host; previous processes may be lost.";
    }
    match stopped(runtime) {
        StoppedCost::WorkerKept => "Stopped. Storage can remain billable; previous processes may be lost.",
        StoppedCost::ServerDeleted => {
            "Stopped. The server was deleted; the workspace volume is kept and stays billable. Resume creates a new server on the same volume."
        }
    }
}

pub(in crate::app::cloud_panel::production) fn delete_confirmation(runtime: &Runtime) -> &'static str {
    if on_docker_host(runtime) {
        return "Delete this worker's container and workspace volume on the Docker host? Running sessions and files in the volume cannot be recovered.";
    }
    match stopped(runtime) {
        StoppedCost::WorkerKept => {
            "Delete this worker and its managed workspace storage? Running sessions and files in that storage cannot be recovered. Any separately attached network volumes retain their files and credentials and remain billable until deleted."
        }
        StoppedCost::ServerDeleted => {
            "Delete this cloud's server, workspace volume and SSH key? Running sessions and files on the volume cannot be recovered."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{delete_confirmation, stop_confirmation, stopped_note};
    use crate::app::cloud_panel::production::Runtime;

    fn on(provider: &str) -> Runtime {
        let state = serde_json::from_value(serde_json::json!({
            "version": 1, "cloud_id": "cloud-1", "repository": "/fixture", "revision": "a".repeat(40),
            "profile": {"provider": provider, "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
                "storage": {"container_gb": 20, "volume_gb": 50}},
            "stage": "Stopped", "operation": {"state": "prepared"}, "worker": null, "sessions": []
        }))
        .unwrap();
        Runtime {
            state: Some(state),
            ..Runtime::default()
        }
    }

    #[test]
    fn a_hetzner_cloud_says_its_server_is_deleted_and_its_volume_kept() {
        let hetzner = on("hetzner");
        for text in [stop_confirmation(&hetzner), stopped_note(&hetzner)] {
            assert!(
                text.contains("server is deleted") || text.contains("server was deleted"),
                "{text}"
            );
            assert!(text.contains("workspace volume is kept and stays billable"), "{text}");
        }
        assert!(!delete_confirmation(&hetzner).contains("network volumes"));
        let runpod = on("runpod");
        assert!(stop_confirmation(&runpod).contains("Storage remains billable"));
        assert!(stopped_note(&runpod).contains("Storage can remain billable"));
        assert!(delete_confirmation(&runpod).contains("network volumes"));
        // A card without a record yet reads as RunPod, as before.
        assert_eq!(stop_confirmation(&Runtime::default()), stop_confirmation(&runpod));
        // Nothing on a saved Docker host is billed.
        let docker = on("docker-host");
        for text in [
            stop_confirmation(&docker),
            stopped_note(&docker),
            delete_confirmation(&docker),
        ] {
            assert!(text.contains("Docker host") && !text.contains("billable"), "{text}");
        }
        assert_eq!(
            super::super::cost::spend(&docker, std::time::SystemTime::now()).line,
            "No rent"
        );
    }
}
