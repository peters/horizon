//! Reconnecting a recorded cloud as the cloud panel's Reconnect does, for callers
//! that hold only its state root.
use super::{
    Deployment, Error, Event, Request, Result, Settings, Stage, Store,
    hetzner::{Journal, JournalFile as _},
};
use horizon_cloud::{Cancellation, CreateState, provider::Kind};
use std::path::Path;

/// Reconnects the cloud recorded under `state_root` with its recorded repository,
/// revision, profile and same-worker siblings, waits for readiness and relaunches its
/// recorded sessions, as the cloud panel's Reconnect does.
/// # Errors
/// Refuses, before any provider request, a cloud [`admit`] refuses; otherwise as
/// [`super::deploy_with_siblings`].
pub fn reconnect(
    state_root: &Path,
    settings: Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    let store = Store::lock(state_root)?;
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    admit(&store, &state)?;
    drop(store);
    let request = Request::new(
        state.cloud_id,
        state.repository,
        state.revision,
        state.profile,
        state_root.into(),
        settings,
    );
    // Checked again under the deployment's own lock, in case the record changed.
    super::run(&request, &[], admit, cancel, emit)
}

/// A reconnect never reopens a deleted cloud, never starts a stopped one and never
/// creates a cloud's first worker.
fn admit(store: &Store, state: &Deployment) -> Result<()> {
    if state.stage == Stage::Deleted {
        return Err(Error::Invalid(
            "The cloud was deleted; deploy it again to create a new worker",
        ));
    }
    if state.stop_requested {
        return Err(Error::Invalid("The worker is stopped; resume it before reconnecting"));
    }
    let bound = matches!(state.operation, CreateState::Bound { .. });
    match horizon_cloud::provider::Description::of(&state.profile).kind {
        Kind::RunPod if bound => Ok(()),
        Kind::Hetzner => hetzner_admits(store, bound),
        Kind::RunPod => Err(NO_WORKER),
    }
}

const NO_WORKER: Error = Error::Invalid("The cloud has no worker to reconnect to; deploy it first");

/// A Hetzner stop deletes the server and Resume clears its fence, so the reconnect
/// after it creates the new server. It may do so only on a workspace volume a server
/// has held, which is what distinguishes it from a first deployment.
fn hetzner_admits(store: &Store, bound: bool) -> Result<()> {
    let journal = Journal::load(store.root())?;
    if journal.deleting {
        return Err(Error::Invalid(
            "The cloud is being deleted; finish deleting it before reconnecting",
        ));
    }
    if bound || (matches!(journal.volume, CreateState::Bound { .. }) && !journal.unused) {
        Ok(())
    } else {
        Err(NO_WORKER)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::cloud_runtime::{
        siblings::{Set, Sibling},
        state::{OperationId, ReplacementImage},
    };

    const HELD: &str = r#"{"volume":{"state":"bound","worker_id":"9"}}"#;

    fn record(provider: &str, stage: &str, operation: &serde_json::Value) -> Deployment {
        let profile = serde_json::json!({"provider":provider,"image":"registry.example/worker","cpu":4,"memory_gb":8});
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"reconnect","repository":"/synthetic/not-mounted","revision":"a".repeat(40),
            "profile":profile,"stage":stage,"operation":operation,
            "spec":{
                "operation_id":"reconnect","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
                "profile":profile,"public_key":"unused","registry_auth_id":null,"gpu_types":[],
                "cpu_flavors":["cpu3c"],"data_centers":[]
            },
            "worker":null,"sessions":[],"ready_history":"Observed"
        }))
        .unwrap()
    }

    fn bound() -> serde_json::Value {
        serde_json::json!({"state":"bound","worker_id":"worker1"})
    }

    /// Settings whose credential files exist and are private when `private`, so an
    /// attempt reaches the checks under the deployment lock.
    fn settings(root: &Path, private: bool) -> Settings {
        if private {
            for file in ["compute", "identity", "hetzner-token"] {
                let path = root.join(file);
                std::fs::write(&path, b"synthetic").unwrap();
                std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
            }
        }
        serde_json::from_value(serde_json::json!({
            "runpod_key_file":root.join("compute"),"ssh_identity_file":root.join("identity"),
            "docker_config":root.join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
            "hetzner":{"token_file":root.join("hetzner-token"),"server_types":["cx33"],"locations":["hel1"]}
        }))
        .unwrap()
    }

    fn save(root: &Path, state: &Deployment, journal: Option<&str>) -> Vec<u8> {
        Store::lock(root).unwrap().save(state).unwrap();
        if let Some(journal) = journal {
            std::fs::write(root.join("hetzner.json"), journal).unwrap();
        }
        std::fs::read(root.join("deployment.json")).unwrap()
    }

    #[test]
    fn refused_reconnects_leave_the_record_untouched_before_any_provider_request() {
        let temp = tempfile::tempdir().unwrap();
        let prepared = serde_json::json!({"state":"prepared"});
        let requested = serde_json::json!({"state":"requested"});
        let mut stopped = record("runpod", "Stopped", &bound());
        stopped.stop_requested = true;
        let mut deleted_hetzner = record("hetzner", "Deleted", &prepared);
        deleted_hetzner.operation = CreateState::Terminated { worker_id: "42".into() };
        let first_hetzner = record("hetzner", "Readiness", &prepared);
        let cases = [
            (record("runpod", "Deleted", &prepared), None, "deleted"),
            (deleted_hetzner, Some(HELD), "deleted"),
            (stopped, None, "resume it"),
            (record("runpod", "Provision", &prepared), None, "deploy it first"),
            (record("runpod", "Provision", &requested), None, "deploy it first"),
            // A first deployment: no volume yet, or one no server has held.
            (first_hetzner.clone(), None, "deploy it first"),
            (
                first_hetzner,
                Some(r#"{"volume":{"state":"bound","worker_id":"9"},"unused":true}"#),
                "deploy it first",
            ),
            (
                record("hetzner", "Ready", &bound()),
                Some(r#"{"volume":{"state":"bound","worker_id":"9"},"deleting":true}"#),
                "being deleted",
            ),
        ];
        for (index, (state, journal, refusal)) in cases.into_iter().enumerate() {
            let root = temp.path().join(index.to_string());
            let saved = save(&root, &state, journal);
            // Unreadable credentials: a reconnect past the guard would fail differently.
            let error = reconnect(&root, settings(temp.path(), false), &Cancellation::default(), &|_| {})
                .unwrap_err()
                .to_string();
            assert!(error.contains(refusal), "{index}: {error}");
            assert_eq!(std::fs::read(root.join("deployment.json")).unwrap(), saved, "{index}");
        }
        let error = reconnect(
            &temp.path().join("none"),
            settings(temp.path(), false),
            &Cancellation::default(),
            &|_| {},
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "No cloud deployment");
    }

    #[test]
    fn a_resumed_hetzner_cloud_is_admitted_on_a_volume_a_server_held() {
        let temp = tempfile::tempdir().unwrap();
        for operation in [
            serde_json::json!({"state":"prepared"}),
            serde_json::json!({"state":"requested"}),
        ] {
            // Stopped and resumed before it was ever ready: the volume held a server all the same.
            let mut state = record("hetzner", "Readiness", &operation);
            state.ready_history = crate::cloud_runtime::state::ReadyHistory::Unobserved;
            save(temp.path(), &state, Some(HELD));
            assert!(admit(&Store::lock(temp.path()).unwrap(), &state).is_ok());
        }
        let state = record("runpod", "Readiness", &serde_json::json!({"state":"prepared"}));
        assert!(
            admit(&Store::lock(temp.path()).unwrap(), &state).is_err(),
            "a RunPod resume keeps its worker"
        );
    }

    #[test]
    fn a_record_deleted_after_the_first_check_is_refused_again_under_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let state = record("runpod", "Deleted", &serde_json::json!({"state":"prepared"}));
        let saved = save(&root, &state, None);
        let request = Request::new(
            state.cloud_id.clone(),
            state.repository.clone(),
            state.revision.clone(),
            state.profile.clone(),
            root.clone(),
            settings(temp.path(), true),
        );
        let error = super::super::run(&request, &[], admit, &Cancellation::default(), &|_| {})
            .unwrap_err()
            .to_string();
        assert!(error.contains("deleted"), "{error}");
        assert_eq!(
            std::fs::read(root.join("deployment.json")).unwrap(),
            saved,
            "not reopened"
        );
    }

    #[test]
    fn pending_image_replacements_and_resumed_hetzner_clouds_pass_both_checks() {
        let temp = tempfile::tempdir().unwrap();
        let settings = settings(temp.path(), true);
        let mut replacing = record("runpod", "Ready", &bound());
        replacing
            .begin_replacement(OperationId::generate(), "c".repeat(40))
            .unwrap();
        replacing
            .replacement_built(ReplacementImage {
                digest: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
                registry_auth_id: None,
                registry_generation: None,
            })
            .unwrap();
        replacing.request_replacement().unwrap();
        let mut switching = replacing.clone();
        switching.stage = Stage::Replace;
        let mut resumed = record("hetzner", "Readiness", &serde_json::json!({"state":"requested"}));
        resumed.repository = temp.path().to_path_buf();
        // Cancelled, so the first command or provider request past both checks stops
        // before it runs: a pending image switch is settled with the provider first,
        // and the resumed Hetzner cloud first packs its source.
        let cancel = Cancellation::default();
        cancel.cancel();
        for (index, (state, journal)) in [(replacing, None), (switching, None), (resumed, Some(HELD))]
            .into_iter()
            .enumerate()
        {
            let root = temp.path().join(index.to_string());
            save(&root, &state, journal);
            let error = reconnect(&root, settings.clone(), &cancel, &|_| {}).unwrap_err();
            assert!(
                matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)),
                "{index}: {error}"
            );
            let saved = Store::lock(&root).unwrap().load().unwrap().unwrap();
            assert_eq!(saved.image_replacement, state.image_replacement, "{index}");
            assert_eq!(saved.operation, state.operation, "{index}");
        }
    }

    #[test]
    fn a_reconnect_keeps_the_recorded_siblings_without_bindings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let mut state = record("runpod", "Readiness", &bound());
        state.version = crate::cloud_runtime::siblings::RECORD_VERSION;
        state.siblings = Some(Set {
            primary_directory: "app".into(),
            members: vec![Sibling {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                revision: "b".repeat(40),
                image_revision: None,
                local_repository: "/synthetic/native-lib".into(),
                profile: "gpu".into(),
            }],
        });
        save(&root, &state, None);
        // Past the guard, the attempt packs the recorded sibling from its recorded
        // checkout, which this fixture lacks, before any provider request.
        let error = reconnect(&root, settings(temp.path(), true), &Cancellation::default(), &|_| {})
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("Sibling `native` checkout"), "{error}");
        let saved = Store::lock(&root).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.siblings, state.siblings);
        assert_eq!(saved.operation, state.operation);
    }
}
