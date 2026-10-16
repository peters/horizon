//! Explicit replacement after confirmed deletion. Unresolved workers stay fenced.
use super::{Deployment, Error, Result, Stage, Store, storage};
use crate::cloud_runtime::state::ReadyHistory;
use horizon_cloud::CreateState;

/// Reopens a confirmed-deleted cloud for a new allocation.
/// The deleted storage journal is removed before the deployment fence is saved,
/// so a crash retries this step instead of provisioning against a deleted volume.
pub(super) fn reopen(store: &Store, state: &mut Deployment, public_key: &str) -> Result<()> {
    if state.stage != Stage::Deleted {
        return Err(Error::Invalid("Only a deleted cloud can be redeployed"));
    }
    #[cfg(feature = "tailnet-node-cleanup")]
    super::super::tailnet::cleanup::require_complete(store, state)?;
    if state.worker.is_some()
        || state.requires_browserstack_release()
        || !matches!(state.operation, CreateState::Prepared | CreateState::Terminated { .. })
    {
        return Err(Error::Invalid("Finish worker cleanup before redeploying this cloud"));
    }
    if state.spec.is_some() && !horizon_cloud::valid_public_key(public_key) {
        return Err(Error::Invalid(
            "Replacement worker requires the current Ed25519 public key",
        ));
    }
    if state
        .spec
        .as_ref()
        .is_some_and(|spec| spec.operation_id != state.cloud_id)
    {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    match &state.spec {
        Some(spec) => storage::release_deleted_journal(store, spec)?,
        None if store.root().join("workspace-volume.json").try_exists()?
            || store.root().join("workspace-volume.required").try_exists()? =>
        {
            return Err(Error::Invalid(
                "Managed workspace storage is not confirmed deleted; finish cleanup before redeploying",
            ));
        }
        None => {}
    }
    if let Some(spec) = &mut state.spec {
        public_key.clone_into(&mut spec.public_key);
    }
    state.stage = Stage::Validate;
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.source_ready = false;
    state.ready_after_seconds = None;
    state.ready_history = ReadyHistory::Unobserved;
    state.stop_requested = false;
    state.browserstack_released = true;
    state.browserstack_targets.clear();
    // A new worker starts without the previous worker's stop reason.
    state.last_self_stop = None;
    store.save(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    mod cleanup;
    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    mod docker_host;
    #[cfg(unix)]
    use crate::cloud_runtime::{deployment::Request, settings::Settings};
    #[cfg(unix)]
    const PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF2kk2kaQHcd1MHINbQ4muiDkEONuV3co+7ug6QOawIB fixture";

    fn deleted(operation: &serde_json::Value) -> Deployment {
        let operation = operation.clone();
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"deleted-cloud","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Deleted","operation":operation,
            "spec":{
                "operation_id":"deleted-cloud",
                "image_digest":format!("registry.example/worker@sha256:{}", "ab".repeat(32)),
                "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
                "public_key":"fixture","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]
            },
            "registry_generation":"generation1","worker":null,
            "sessions":[{"panel_id":"panel1","agent":"shell","tmux":"panel1","branch":"agent/panel1","worktree":"/workspace/agents/panel1"}],
            "source_ready":true,"ready_after_seconds":12,"ready_history":"Observed","stop_requested":true,
            "browserstack_released":true,"browserstack_targets":["phone"]
        }))
        .unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn enrollment_ledger_survives_failed_save_confirmed_deletion_and_redeploy() {
        use crate::cloud_runtime::tailnet::receipt::tests::populated;
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.spec = None;
        state.tailnet_enrollment = populated();
        state.stage = Stage::Provision;
        store.save(&state).unwrap();
        let before = state.tailnet_enrollment.clone();
        // A failed later stage cannot discard an already durable captured ID.
        std::fs::rename(root.path().join("deployment.json"), root.path().join("saved.json")).unwrap();
        std::fs::create_dir(root.path().join("deployment.json")).unwrap();
        assert!(store.save(&state).is_err());
        std::fs::remove_dir(root.path().join("deployment.json")).unwrap();
        std::fs::rename(root.path().join("saved.json"), root.path().join("deployment.json")).unwrap();
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
        super::super::deletion::finish_deletion(state, &store).unwrap();
        let mut deleted = store.load().unwrap().unwrap();
        assert_eq!(deleted.stage, Stage::Deleted);
        assert!(deleted.worker.is_none());
        assert_eq!(deleted.tailnet_enrollment, before);
        #[cfg(not(feature = "tailnet-node-cleanup"))]
        {
            reopen(&store, &mut deleted, "unused").unwrap();
            assert_eq!(deleted.stage, Stage::Validate);
        }
        #[cfg(feature = "tailnet-node-cleanup")]
        {
            assert!(reopen(&store, &mut deleted, "unused").is_err());
            let receipt = deleted.tailnet_enrollment.receipts()[0].clone();
            deleted.tailnet_enrollment.confirm_removed(&receipt).unwrap();
            store.save(&deleted).unwrap();
            let confirmed = deleted.tailnet_enrollment.clone();
            assert!(reopen(&store, &mut deleted, "unused").is_err());
            assert_eq!(deleted.stage, Stage::Deleted);
            assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, confirmed);
            assert_eq!(deleted.tailnet_enrollment.attempts(), before.attempts());
        }
        #[cfg(not(feature = "tailnet-node-cleanup"))]
        assert_eq!(store.load().unwrap().unwrap().tailnet_enrollment, before);
    }

    #[cfg(unix)]
    fn write_matching_journal(root: &std::path::Path, spec: &horizon_cloud::WorkerSpec, state: &str) {
        let body = serde_json::json!({
            "version": 1,
            "worker": spec,
            "spec": {
                "operation_id": spec.operation_id,
                "size": spec.profile.storage.volume_gb,
                "data_center_id": "EU-TEST-1"
            },
            "state": {"state": state}
        });
        std::fs::write(root.join("workspace-volume.required"), b"").unwrap();
        std::fs::write(root.join("workspace-volume.json"), serde_json::to_vec(&body).unwrap()).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn confirmed_deletion_reopens_without_dropping_sessions_or_the_image() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "deleted");
        reopen(&store, &mut state, PUBLIC_KEY).unwrap();
        assert_eq!(state.stage, Stage::Validate);
        assert_eq!(state.operation, CreateState::Prepared);
        assert!(state.worker.is_none());
        assert!(!state.source_ready);
        assert!(state.ready_after_seconds.is_none());
        assert_eq!(state.ready_history, ReadyHistory::Unobserved);
        assert!(!state.stop_requested);
        assert!(state.browserstack_released);
        assert!(state.browserstack_targets.is_empty());
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.registry_generation.as_deref(), Some("generation1"));
        let spec = state.spec.as_ref().unwrap();
        assert_eq!(spec.operation_id, "deleted-cloud");
        assert_eq!(spec.public_key, PUBLIC_KEY);
        assert!(spec.image_digest.ends_with(&"ab".repeat(32)));
        assert!(state.resizable());
        assert!(!root.path().join("workspace-volume.json").exists());
        assert!(!root.path().join("workspace-volume.required").exists());
        let restored = store.load().unwrap().unwrap();
        assert_eq!(restored.stage, Stage::Validate);
        assert_eq!(restored.sessions[0].panel_id, "panel1");
    }

    #[test]
    #[cfg(unix)]
    fn unfinished_cleanup_keeps_the_deleted_fence_and_journal() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "deleted");
        assert!(
            reopen(&store, &mut state, "not-a-key")
                .unwrap_err()
                .to_string()
                .contains("Ed25519")
        );
        assert!(root.path().join("workspace-volume.json").exists());
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "prepared");
        let error = reopen(&store, &mut state, PUBLIC_KEY).unwrap_err();
        assert!(error.to_string().contains("not confirmed deleted"), "{error}");
        assert_eq!(state.stage, Stage::Deleted);
        assert!(root.path().join("workspace-volume.json").exists());
        assert!(root.path().join("workspace-volume.required").exists());
        state.operation = CreateState::Requested;
        write_matching_journal(root.path(), state.spec.as_ref().unwrap(), "deleted");
        assert!(
            reopen(&store, &mut state, PUBLIC_KEY)
                .unwrap_err()
                .to_string()
                .contains("Finish worker cleanup")
        );
        assert!(root.path().join("workspace-volume.json").exists());
        state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.browserstack_released = false;
        state.profile.capabilities.browserstack = Some(horizon_cloud::BrowserStack {
            provider: horizon_cloud::BrowserStack::default_provider(),
            targets: ["phone".into()].into(),
            local_ports: std::collections::BTreeSet::new(),
        });
        assert!(
            reopen(&store, &mut state, PUBLIC_KEY)
                .unwrap_err()
                .to_string()
                .contains("Finish worker cleanup")
        );
        state = deleted(&serde_json::json!({"state":"prepared"}));
        state.spec.as_mut().unwrap().operation_id = "other".into();
        assert!(
            reopen(&store, &mut state, PUBLIC_KEY)
                .unwrap_err()
                .to_string()
                .contains("identities differ")
        );
        std::fs::remove_file(root.path().join("workspace-volume.json")).unwrap();
        state = deleted(&serde_json::json!({"state":"prepared"}));
        assert!(
            reopen(&store, &mut state, PUBLIC_KEY)
                .unwrap_err()
                .to_string()
                .contains("journal is missing")
        );
    }

    #[test]
    #[cfg(unix)]
    fn deploy_reopens_a_deleted_cloud_before_source_export_can_fail() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cloud = root.path().join("cloud");
        let repository = root.path().join("missing-repo");
        let revision = "a".repeat(40);
        let profile = serde_json::json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false});
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.repository.clone_from(&repository);
        state.revision.clone_from(&revision);
        state.registry_generation = None;
        state.profile = serde_json::from_value(profile.clone()).unwrap();
        state.spec.as_mut().unwrap().profile = state.profile.clone();
        {
            let store = Store::lock(&cloud).unwrap();
            store.save(&state).unwrap();
            write_matching_journal(&cloud, state.spec.as_ref().unwrap(), "deleted");
        }
        let key = root.path().join("runpod");
        let ssh = root.path().join("ssh");
        for path in [&key, &ssh] {
            std::fs::write(path, b"synthetic-key\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::write(root.path().join("ssh.pub"), format!("{PUBLIC_KEY}\n")).unwrap();
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file": key, "ssh_identity_file": ssh, "docker_config": root.path().join("docker"),
            "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
        }))
        .unwrap();
        let request = Request::new(
            state.cloud_id.clone(),
            repository,
            revision,
            serde_json::from_value(profile).unwrap(),
            cloud.clone(),
            settings,
        );
        assert!(super::super::deploy(&request, &horizon_cloud::Cancellation::default(), &|_| {}).is_err());
        let saved = Store::lock(&cloud).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.stage, Stage::Validate);
        assert_eq!(saved.operation, CreateState::Prepared);
        assert!(!saved.source_ready);
        assert!(!saved.stop_requested);
        assert_eq!(saved.sessions[0].panel_id, "panel1");
        assert_eq!(saved.spec.as_ref().unwrap().public_key, PUBLIC_KEY);
        assert!(!cloud.join("workspace-volume.json").exists());
    }

    #[test]
    #[cfg(unix)]
    fn missing_credentials_do_not_reopen_a_deleted_cloud() {
        let root = tempfile::tempdir().unwrap();
        let cloud = root.path().join("cloud");
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.repository = root.path().join("missing-repo");
        state.revision = "b".repeat(40);
        {
            let store = Store::lock(&cloud).unwrap();
            store.save(&state).unwrap();
            write_matching_journal(&cloud, state.spec.as_ref().unwrap(), "deleted");
        }
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file": root.path().join("missing-key"),
            "ssh_identity_file": root.path().join("missing-ssh"),
            "docker_config": root.path().join("docker"),
            "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
        }))
        .unwrap();
        let request = Request::new(
            state.cloud_id.clone(),
            state.repository.clone(),
            state.revision.clone(),
            state.profile.clone(),
            cloud.clone(),
            settings,
        );
        assert!(super::super::deploy(&request, &horizon_cloud::Cancellation::default(), &|_| {}).is_err());
        let saved = Store::lock(&cloud).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.stage, Stage::Deleted);
        assert!(matches!(saved.operation, CreateState::Terminated { .. }));
        assert!(cloud.join("workspace-volume.json").exists());
    }

    #[test]
    #[cfg(unix)]
    fn preparing_a_deleted_cloud_keeps_the_recorded_worker_identity() {
        let root = tempfile::tempdir().unwrap();
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        state.revision = "c".repeat(40);
        state.repository = root.path().join("repo");
        state.profile.gpu = false;
        state.spec.as_mut().unwrap().profile.gpu = false;
        let original_cpu = state.spec.as_ref().unwrap().profile.cpu;
        let journal = serde_json::json!({
            "version": 1,
            "worker": state.spec,
            "spec": {
                "operation_id": "deleted-cloud",
                "size": state.profile.storage.volume_gb,
                "data_center_id": "EU-TEST-1"
            },
            "state": {"state": "deleted"}
        });
        {
            let store = Store::lock(root.path()).unwrap();
            store.save(&state).unwrap();
            std::fs::write(root.path().join("workspace-volume.required"), b"").unwrap();
            std::fs::write(
                root.path().join("workspace-volume.json"),
                serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
        }
        let mut profile = state.profile.clone();
        profile.cpu = 8;
        profile.memory_gb = 16;
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused",
            "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        super::super::prepare(&Request::new(
            state.cloud_id.clone(),
            state.repository.clone(),
            state.revision,
            profile,
            root.path().into(),
            settings,
        ))
        .unwrap();
        let store = Store::lock(root.path()).unwrap();
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.spec.as_ref().unwrap().profile.cpu, original_cpu);
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &saved).unwrap());
    }

    #[test]
    fn replacement_size_is_available_only_after_confirmed_deletion() {
        let mut state = deleted(&serde_json::json!({"state":"terminated","worker_id":"worker1"}));
        assert!(!state.resizable());
        assert!(state.accepts_next_size());
        state.stage = Stage::Provision;
        assert!(!state.accepts_next_size());
        state.stage = Stage::Deleted;
        state.operation = CreateState::Prepared;
        assert!(!state.resizable());
        assert!(state.accepts_next_size());
        state.operation = CreateState::Bound {
            worker_id: "worker1".into(),
        };
        assert!(!state.resizable());
        assert!(!state.accepts_next_size());
    }
}
