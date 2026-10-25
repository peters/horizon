use super::*;
mod publishing_fence;
#[test]
#[cfg(unix)]
fn private_registry_intent_survives_failed_preparation_and_missing_bindings() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"registry-retry","repository":root.path(),"revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/team/worker","cpu":4,"memory_gb":8},
        "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":root.path().join("compute"),"ssh_identity_file":root.path().join("identity"),
        "docker_config":root.path().join("docker"),"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":root.path().join("registry"),"bindings":[{
            "repository":"registry.example/team/worker","generation":"generation1","read_only_confirmed":true,
            "pull":{"username":"reader","secret_file":root.path().join("missing"),"expires_at":null},
            "publish":null
        }]}
    }))
    .unwrap();
    let mut request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        root.path().into(),
        settings,
    );
    assert!(state.registry_generation.is_none(), "legacy state remains readable");
    assert!(prepare_registry(&request, &store, &mut state).is_err());
    state = store.load().unwrap().unwrap();
    assert_eq!(state.registry_generation.as_deref(), Some("generation1"));
    let config = request.settings.registries.take().unwrap();
    for omitted in [
        None,
        Some(super::super::registry::Config {
            root: config.root.clone(),
            bindings: Vec::new(),
        }),
    ] {
        request.settings.registries = omitted;
        assert!(
            matches!(prepare_registry(&request, &store, &mut state), Err(Error::Invalid(message)) if message.contains("binding is missing"))
        );
        state.spec = Some(WorkerSpec {
            exact_placement: false,
            operation_id: state.cloud_id.clone(),
            image_digest: format!("{}@sha256:{}", state.profile.image, "a".repeat(64)),
            profile: state.profile.clone(),
            public_key: "fixture".into(),
            registry_auth_id: None,
            gpu_types: Vec::new(),
            cpu_flavors: Vec::new(),
            data_centers: Vec::new(),
            startup_metadata: None,
        });
        store.save(&state).unwrap();
        state = store.load().unwrap().unwrap();
        assert!(
            matches!(prepare_registry(&request, &store, &mut state), Err(Error::Invalid(message)) if message.contains("binding is missing"))
        );
    }
    request.settings.registries = Some(config);
    request.settings.registries.as_mut().unwrap().bindings[0].generation = "generation2".into();
    assert!(
        prepare_registry(&request, &store, &mut state).is_err(),
        "replacement must still load its credential"
    );
    assert_eq!(
        store.load().unwrap().unwrap().registry_generation.as_deref(),
        Some("generation2")
    );
    state.operation = CreateState::Requested;
    assert!(
        prepare_registry(&request, &store, &mut state).unwrap().is_none(),
        "an existing registry cannot settle a requested worker"
    );
    assert_requested_worker_stays_pending_without_storage(&store, &mut state);
    request.settings.registries = None;
    assert!(
        prepare_registry(&request, &store, &mut state).unwrap().is_none(),
        "reconciliation must not require a removed local grant"
    );
    state.operation = CreateState::Prepared;
    state.registry_generation = None;
    assert!(
        prepare_registry(&request, &store, &mut state).unwrap().is_none(),
        "legacy unbound images retain their behavior"
    );
}

fn assert_requested_worker_stays_pending_without_storage(store: &Store, state: &mut Deployment) {
    let mut spec = state.spec.clone().unwrap();
    spec.public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".into();
    spec.cpu_flavors = vec!["cpu3c".into()];
    spec.validate().unwrap();
    let provider = RunPod::new(horizon_cloud::Credential::new("synthetic".into()).unwrap());
    let seen = std::cell::RefCell::new(Vec::new());
    let observe = |phase| {
        let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
        seen.borrow_mut().push(phase);
        Ok(previous)
    };
    assert!(
        provision(
            &provider,
            store,
            state,
            &spec,
            &Cancellation::default(),
            &|_| {},
            &observe
        )
        .is_err()
    );
    assert_eq!(
        *seen.borrow(),
        [Mutation::Pending],
        "missing storage cannot clear worker uncertainty"
    );
    assert_registry_retry_keeps_storage_pending(store, state, spec);
}

fn assert_registry_retry_keeps_storage_pending(store: &Store, state: &Deployment, mut spec: WorkerSpec) {
    let mut state = state.clone();
    spec.registry_auth_id = Some("pull1".into());
    state.operation = CreateState::Prepared;
    state.spec = Some(spec.clone());
    store.save(&state).unwrap();
    std::fs::write(store.root().join("workspace-volume.json"), serde_json::to_vec(&serde_json::json!({
        "version":1,"worker":spec,"spec":{"operation_id":spec.operation_id,"size":spec.profile.storage.volume_gb,"data_center_id":"dc1"},
        "state":{"state":"requested"}
    })).unwrap()).unwrap();
    let seen = std::cell::RefCell::new(vec![Mutation::Pending]);
    let observe = |phase| {
        let previous = *seen.borrow().last().unwrap();
        seen.borrow_mut().push(phase);
        Ok(previous)
    };
    bind_registry(store, &mut state, || Ok("pull1".into())).unwrap();
    storage::observe_pending(store, &spec, &observe).unwrap();
    assert!(matches!(storage::expected(store, &spec), Err(Error::Invalid(_))));
    assert_eq!(
        *seen.borrow(),
        [Mutation::Pending, Mutation::Pending],
        "registry reconciliation must not settle the earlier volume allocation"
    );
}

#[test]
#[cfg(unix)]
fn completed_source_is_durable_before_session_configuration() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"source","repository":"/fixture","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Worktrees","operation":{"state":"requested"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    begin_sessions(&mut state, &store, &|event| {
        assert!(matches!(event, Event::Stage(Stage::Sessions, _)));
        let saved = store.load().unwrap().unwrap();
        assert!(saved.source_ready);
        assert_eq!(saved.stage, Stage::Sessions);
    })
    .unwrap();
    drop(store);
    let restored = Store::lock(root.path()).unwrap().load().unwrap().unwrap();
    assert!(restored.source_ready);
    assert_eq!(restored.stage, Stage::Sessions);
}

#[test]
#[cfg(unix)]
fn ready_timing_preserves_unknown_legacy_history_and_survives_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let legacy: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"timing","repository":"/fixture","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Ready","operation":{"state":"requested"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let unknown = finish_ready(legacy.clone(), &store, None, &|_| {}).unwrap();
    assert!(unknown.ready_after_seconds.is_none());
    let start = Instant::now().checked_sub(Duration::from_secs(420)).unwrap();
    let measured = finish_ready(legacy, &store, Some(start), &|_| {}).unwrap();
    assert_eq!(measured.ready_after_seconds, Some(420));
    let restored = store.load().unwrap().unwrap();
    let reconnected = finish_ready(restored, &store, None, &|_| {}).unwrap();
    assert_eq!(reconnected.ready_after_seconds, measured.ready_after_seconds);
}

#[test]
#[cfg(unix)]
fn failed_readiness_needs_no_ssh_cleanup_but_interrupted_credential_install_does() {
    let root = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(root.path().join("repo")).unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let author = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let revision = repo
        .commit(Some("HEAD"), &author, &author, "Create fixture", &tree, &[])
        .unwrap();
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":"unused", "ssh_identity_file":"unused", "docker_config":"unused",
        "registry_pull_auth_id":null, "cpu_flavors":[], "gpu_types":[]
    }))
    .unwrap();
    let profile = serde_json::from_value(serde_json::json!({
        "provider":"runpod", "image":"registry.example.com/worker:mobile", "cpu":4, "memory_gb":8, "gpu":false,
        "capabilities":{"browserstack":{"targets":["phone"]}}
    }))
    .unwrap();
    let request = Request::new(
        "test".into(),
        repo.workdir().unwrap().into(),
        revision.to_string(),
        profile,
        root.path().join("cloud"),
        settings,
    );
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = initial_state(&request, &store).unwrap();
    state.stage = Stage::Readiness;
    store.save(&state).unwrap();
    assert!(!store.load().unwrap().unwrap().requires_browserstack_release());
    store.arm_browserstack(&mut state).unwrap();
    // Simulate losing the SSH installer response: the persisted fence must survive.
    assert!(store.load().unwrap().unwrap().requires_browserstack_release());
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy.as_object_mut().unwrap().remove("browserstack_released");
    assert!(
        serde_json::from_value::<Deployment>(legacy)
            .unwrap()
            .requires_browserstack_release()
    );
}
#[test]
#[cfg(unix)]
fn preparation_preserves_pinned_revision_without_reading_a_repository() {
    let root = tempfile::tempdir().unwrap();
    let mut request = Request::new(
        "pinned-fixture".into(),
        root.path().join("repository-is-not-mounted"),
        "a".repeat(40),
        serde_json::from_value(
            serde_json::json!({"provider":"runpod","image":"registry.example.com/worker","cpu":4,"memory_gb":8}),
        )
        .unwrap(),
        root.path().join("state"),
        serde_json::from_value(
            serde_json::json!({"runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]}),
        )
        .unwrap(),
    );
    prepare(&request).unwrap();
    let saved = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
    assert_eq!(saved.revision, request.revision);
    assert_eq!(saved.operation, CreateState::Prepared);
    assert!(saved.worker.is_none());
    assert!(
        !saved.source_ready,
        "background preallocation tree validation is still required"
    );
    for revision in ["HEAD", "main", "abc123", "z".repeat(40).as_str()] {
        request.revision = revision.into();
        assert!(prepare(&request).is_err());
    }
}
#[test]
#[cfg(unix)]
fn an_azure_cloud_without_azure_settings_is_refused_before_a_deployment_record_is_written() {
    let root = tempfile::tempdir().unwrap();
    let request = Request::new(
        "azure-fixture".into(),
        root.path().join("repository-is-not-mounted"),
        "a".repeat(40),
        serde_json::from_value(
            serde_json::json!({"provider":"azure","image":"registry.example.com/worker","cpu":4,"memory_gb":8}),
        )
        .unwrap(),
        root.path().join("state"),
        serde_json::from_value(
            serde_json::json!({"runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]}),
        )
        .unwrap(),
    );
    let missing = "Add an azure section to the cloud settings before deploying an Azure cloud";
    for refused in [prepare(&request), prepare_image_record(&request)] {
        assert_eq!(refused.unwrap_err().to_string(), missing);
    }
    let deployed = deploy(&request, &Cancellation::default(), &|_| {}).map(drop);
    assert_eq!(deployed.unwrap_err().to_string(), missing);
    assert!(!request.state_root.join("deployment.json").exists());
}
#[test]
#[cfg(unix)]
fn retry_timing_preserves_prior_ready_history_even_after_failed_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let legacy: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"timing","repository":"/fixture","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Ready","operation":{"state":"bound","worker_id":"fixture"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    assert!(attempt_started(&legacy, Instant::now()).is_none());
    store.save(&legacy).unwrap();
    let mut reconnect = store.load().unwrap().unwrap();
    reconnect.stage = Stage::Readiness;
    store.save(&reconnect).unwrap();
    assert!(attempt_started(&store.load().unwrap().unwrap(), Instant::now()).is_none());
    let mut unfinished = legacy;
    for stage in [Stage::Validate, Stage::Readiness, Stage::Worktrees, Stage::Sessions] {
        unfinished.stage = stage;
        assert!(attempt_started(&unfinished, Instant::now()).is_some());
    }
    let started = Instant::now().checked_sub(Duration::from_secs(12)).unwrap();
    let timed = finish_ready(unfinished, &store, Some(started), &|_| {}).unwrap();
    assert_eq!(timed.ready_after_seconds, Some(12));
    assert_eq!(timed.ready_history, ReadyHistory::Observed);
    assert!(attempt_started(&timed, Instant::now()).is_none());
}
#[test]
#[cfg(unix)]
fn size_changes_apply_until_a_worker_is_requested() {
    let root = tempfile::tempdir().unwrap();
    let mut request = Request::new(
        "resize".into(),
        root.path().join("repository-is-not-mounted"),
        "a".repeat(40),
        serde_json::from_value(
            serde_json::json!({"provider":"runpod","image":"registry.example.com/worker","cpu":8,"memory_gb":32}),
        )
        .unwrap(),
        root.path().join("state"),
        serde_json::from_value(
            serde_json::json!({"runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[]}),
        )
        .unwrap(),
    );
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = initial_state(&request, &store).unwrap();
    // A definite provider rejection keeps the built image's spec with its old size.
    state.spec = Some(WorkerSpec {
        exact_placement: false,
        operation_id: "resize".into(),
        image_digest: format!("registry.example.com/worker@sha256:{}", "a".repeat(64)),
        profile: state.profile.clone(),
        public_key: String::new(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: vec!["cpu3c".into()],
        data_centers: Vec::new(),
        startup_metadata: None,
    });
    store.save(&state).unwrap();
    request.profile.cpu = 1;
    request.profile.memory_gb = 16;
    assert!(initial_state(&request, &store).is_err());
    let saved = store.load().unwrap().unwrap();
    assert_eq!((saved.profile.cpu, saved.profile.memory_gb), (8, 32));
    request.profile.cpu = 4;
    initial_state(&request, &store).unwrap();
    let saved = store.load().unwrap().unwrap();
    let spec = saved.spec.as_ref().unwrap();
    assert_eq!((saved.profile.cpu, saved.profile.memory_gb), (4, 16));
    assert_eq!(spec.profile, saved.profile);
    assert_eq!(spec.cpu_flavors, ["cpu3g"]);
    // Machine settings changed after a rejection apply to the next attempt.
    request.settings.cpu_flavors = vec!["cpu5g".into()];
    request.settings.gpu_types = vec!["fixture-gpu".into()];
    let mut state = initial_state(&request, &store).unwrap();
    refresh_allocation(&request, &store, &mut state).unwrap();
    let spec = store.load().unwrap().unwrap().spec.unwrap();
    assert_eq!(spec.cpu_flavors, ["cpu5g"]);
    assert_eq!(spec.gpu_types, ["fixture-gpu"]);
    request.profile.storage.volume_gb += 1;
    assert!(initial_state(&request, &store).is_err());
    request.profile.storage.volume_gb -= 1;
    // A requested worker fixes its size and saved spec.
    state.operation = CreateState::Requested;
    store.save(&state).unwrap();
    request.settings.cpu_flavors = vec!["cpu3g".into()];
    refresh_allocation(&request, &store, &mut state).unwrap();
    assert_eq!(store.load().unwrap().unwrap().spec.unwrap().cpu_flavors, ["cpu5g"]);
    request.profile.cpu = 8;
    assert!(initial_state(&request, &store).is_err());
    request.profile.cpu = 4;
    assert!(initial_state(&request, &store).is_ok());
}

#[test]
#[cfg(unix)]
fn reconnect_observes_an_image_update_that_may_be_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let mut key = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    std::io::Write::write_all(&mut key, b"synthetic-test-key").unwrap();
    let mut identity = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    std::io::Write::write_all(&mut identity, b"synthetic-identity").unwrap();
    let profile = serde_json::json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8});
    let request = Request::new(
        "reconnect".into(),
        // Unmounted, so an attempt that passes the guard stops before provider I/O.
        root.path().join("repository-is-not-mounted"),
        "a".repeat(40),
        serde_json::from_value(profile.clone()).unwrap(),
        root.path().join("state"),
        serde_json::from_value(serde_json::json!({
            "runpod_key_file":key.path(),"ssh_identity_file":identity.path(),
            "docker_config":root.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap(),
    );
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":request.cloud_id,"repository":request.repository,"revision":request.revision,
        "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "spec":{
            "operation_id":request.cloud_id,"image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":profile,"public_key":"unused","registry_auth_id":null,"gpu_types":[],
            "cpu_flavors":["cpu3c"],"data_centers":[]
        },
        "worker":null,"sessions":[]
    }))
    .unwrap();
    state
        .begin_replacement(OperationId::generate(), "c".repeat(40))
        .unwrap();
    state
        .replacement_built(super::super::state::ReplacementImage {
            digest: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
            registry_auth_id: None,
            registry_generation: None,
        })
        .unwrap();
    // Cancelled, so the provider observation stops before sending a request.
    let cancel = Cancellation::default();
    cancel.cancel();
    for requested in [false, true] {
        if requested {
            state.request_replacement().unwrap();
        }
        Store::lock(&request.state_root).unwrap().save(&state).unwrap();
        let error = deploy(&request, &cancel, &|_| {}).unwrap_err();
        // Only an update that may have been sent is settled through the provider.
        let observed = matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled));
        assert_eq!(observed, requested, "{error}");
        let saved = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.image_replacement, state.image_replacement);
        assert_eq!(saved.stage, state.stage);
    }
}

#[test]
fn a_reconnect_arms_resume_for_every_recorded_session() {
    let session = serde_json::json!({
        "panel_id":"panel","agent":"claude","tmux":"panel",
        "branch":"agent/panel","worktree":"/workspace/agents/panel"
    });
    let mut reconnecting: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"cloud","repository":"/tmp/repo","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker"},
        "spec":null,"worker":null,"sessions":[session],"ready_history":"Observed"
    }))
    .unwrap();
    assert!(arm_recorded_session_resume(&mut reconnecting));
    let operation = reconnecting.session_restart.expect("resume armed");
    assert!(
        !arm_recorded_session_resume(&mut reconnecting),
        "the same reconnect keeps one operation"
    );
    assert_eq!(reconnecting.session_restart, Some(operation));

    let mut first = reconnecting.clone();
    first.session_restart = None;
    first.ready_history = ReadyHistory::Unobserved;
    first.operation = CreateState::Prepared;
    assert!(!arm_recorded_session_resume(&mut first));

    let mut empty = reconnecting.clone();
    empty.session_restart = None;
    empty.sessions.clear();
    assert!(!arm_recorded_session_resume(&mut empty));
}

#[test]
fn local_preparation_and_unsent_replacements_preserve_mutation_evidence() {
    for failure in ["source", "auth", "prepared", "built"] {
        let root = tempfile::tempdir().unwrap();
        let key = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        std::fs::write(key.path(), "synthetic-key").unwrap();
        let store = Store::lock(&root.path().join("cloud")).unwrap();
        let mut state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"local-failure","repository":root.path(),"revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"capabilities":{"agents":["codex"]}},
            "stage":"Readiness","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"worker":null,"sessions":[]
        })).unwrap();
        if matches!(failure, "prepared" | "built") {
            state.stage = Stage::Ready;
            state.spec = Some(WorkerSpec {
                exact_placement: false,
                operation_id: state.cloud_id.clone(),
                image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
                profile: state.profile.clone(),
                public_key: "fixture".into(),
                registry_auth_id: None,
                gpu_types: vec![],
                cpu_flavors: vec!["cpu3c".into()],
                data_centers: vec![],
                startup_metadata: None,
            });
            state
                .begin_replacement(OperationId::generate(), "c".repeat(40))
                .unwrap();
            if failure == "built" {
                state
                    .replacement_built(crate::cloud_runtime::state::ReplacementImage {
                        digest: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
                        registry_auth_id: None,
                        registry_generation: None,
                    })
                    .unwrap();
            }
        }
        store.save(&state).unwrap();
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":key.path(),"ssh_identity_file":key.path(),"docker_config":"/missing",
            "cpu_flavors":[],"gpu_types":[],"openai_api_key_file":(failure == "auth").then(|| root.path().join("missing-auth"))
        }))
        .unwrap();
        let request = Request::new(
            state.cloud_id,
            state.repository,
            state.revision,
            state.profile,
            store.root().into(),
            settings,
        );
        let seen = std::cell::RefCell::new(Vec::new());
        let observe = |phase| {
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        let error = deploy_locked(
            &request,
            &[],
            &store,
            |_, _| Ok(()),
            &Cancellation::default(),
            &|_| {},
            &observe,
        )
        .unwrap_err();
        if failure == "auth" {
            assert!(matches!(error, Error::AgentCredential { agent: "codex", .. }));
        } else {
            assert!(matches!(error, Error::Command(_)));
        }
        assert!(
            seen.borrow().is_empty(),
            "preparation cannot clear prior evidence or invent a mutation"
        );
    }
}

/// A cloud of `profile` for `repository`, bound to synthetic machine files below `root`:
/// a `RunPod` key, an SSH identity and a saved pull login that quick start must not use.
fn quick_start_request(
    root: &std::path::Path,
    launch: &repository::launch::Prepared,
    profile: horizon_cloud::Profile,
) -> Request {
    use std::os::unix::fs::PermissionsExt;
    let private = |name: &str, contents: &str| {
        let path = root.join(name);
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    };
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": private("compute-key", "synthetic-compute-key"),
        "ssh_identity_file": private("identity", "synthetic-identity"),
        "docker_config": root.join("docker"), "registry_pull_auth_id": "saved-pull-login",
        "cpu_flavors": [], "gpu_types": []
    }))
    .unwrap();
    std::fs::write(
        root.join("identity.pub"),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF2kk2kaQHcd1MHINbQ4muiDkEONuV3co+7ug6QOawIB fixture\n",
    )
    .unwrap();
    Request::new(
        super::super::new_id(),
        launch.repository.clone(),
        launch.revision.clone(),
        profile,
        root.join(format!("cloud-{}", super::super::new_id())),
        settings,
    )
}

fn prepare_quick_start_fixture(root: &std::path::Path, cancel: &Cancellation) -> repository::launch::Prepared {
    use repository::launch::Configuration;
    let plain = root.join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("README"), "Hello World!\n").unwrap();
    for args in [
        &["init", "--quiet"][..],
        &["add", "README"],
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "First",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&plain)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let quiet = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    repository::launch::prepare_with_configuration(plain.to_str().unwrap(), "HEAD", Configuration::QuickStart, &quiet)
        .unwrap()
}

/// Quick start reaches the provider request with no `docker` program and no Docker daemon:
/// no digest lookup, no pull and no local contract check. The same image with another
/// selection of tools keeps the local check, so it stops here and says that Docker is
/// missing. The test cancels at Provision, before any provider request, so it allocates
/// nothing.
#[test]
fn quick_start_reaches_allocation_without_local_docker() {
    use repository::launch::quick_start;
    if !super::super::image::without_docker::child(
        "cloud_runtime::deployment::tests::quick_start_reaches_allocation_without_local_docker",
    ) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let launch = prepare_quick_start_fixture(root.path(), &cancel);
    let profile = launch.config.profiles[quick_start::PROFILE].clone();
    let request = quick_start_request(root.path(), &launch, profile.clone());
    let stages = std::cell::RefCell::new(Vec::new());
    let output = std::cell::RefCell::new(Vec::new());
    let emit = |event| match event {
        Event::Stage(stage, _) => {
            stages.borrow_mut().push(stage);
            if stage == Stage::Provision {
                cancel.cancel();
            }
        }
        Event::Output(line) => output.borrow_mut().push(line),
        _ => {}
    };
    let error = deploy(&request, &cancel, &emit).unwrap_err();
    assert!(
        matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)),
        "{error}"
    );
    assert_eq!(*stages.borrow(), [Stage::Validate, Stage::Provision]);
    assert!(
        output
            .borrow()
            .iter()
            .any(|line| line.contains("does not run in local Docker")),
        "{:?}",
        output.borrow()
    );
    let state = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
    let spec = state.spec.unwrap();
    assert_eq!(spec.image_digest, quick_start::IMAGE);
    assert_eq!(spec.registry_auth_id, None);
    assert!(
        !request.state_root.join("workspace-volume.json").exists(),
        "nothing reached the provider"
    );

    let mut other = profile;
    other.capabilities.desktop = false;
    let request = quick_start_request(root.path(), &launch, other);
    let error = deploy(&request, &Cancellation::default(), &|_| {})
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("Docker is not installed on this computer"), "{error}");
    assert!(
        Store::lock(&request.state_root)
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .spec
            .is_none()
    );
}

#[test]
fn quick_start_accepts_tagged_tailnet_without_local_docker() {
    use repository::launch::quick_start;
    if !super::super::image::without_docker::child(
        "cloud_runtime::deployment::tests::quick_start_accepts_tagged_tailnet_without_local_docker",
    ) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let launch = prepare_quick_start_fixture(root.path(), &Cancellation::default());
    let profile = launch.config.profiles[quick_start::PROFILE].clone();
    let request = quick_start_request(root.path(), &launch, profile);
    std::fs::create_dir(&request.state_root).unwrap();
    std::fs::write(request.state_root.join("tailnet.json"), br#"{"tailnet":"work"}"#).unwrap();
    let stages = std::cell::RefCell::new(Vec::new());
    let cancel = Cancellation::default();
    let error = deploy(&request, &cancel, &|event| {
        if let Event::Stage(stage, _) = event {
            stages.borrow_mut().push(stage);
            if stage == Stage::Provision {
                cancel.cancel();
            }
        }
    })
    .unwrap_err();
    assert!(
        matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)),
        "{error}"
    );
    assert_eq!(*stages.borrow(), [Stage::Validate, Stage::Provision]);
    let state = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
    assert!(state.worker.is_none());
    assert_eq!(state.spec.unwrap().image_digest, quick_start::IMAGE);
    assert_eq!(
        state.operation,
        CreateState::Prepared,
        "no provider allocation was requested"
    );
    assert!(!request.state_root.join("workspace-volume.json").exists());
}

#[test]
#[cfg(unix)]
fn a_cloud_records_the_agent_that_created_it_and_earlier_records_still_load() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
        "provider":"runpod","cpu":2,"memory_gb":4,
        "build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap();
    let request = |creator: Option<(&str, Option<String>)>| {
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":root.path().join("key"),"ssh_identity_file":root.path().join("ssh"),
            "docker_config":root.path().join("docker"),"registry_pull_auth_id":null,
            "cpu_flavors":[],"gpu_types":[],"portable_image_publishing":true,
            "registries":{"root":root.path().join("registry"),"bindings":[{
                "repository":"registry.example/team/worker","generation":"grant-one",
                "read_only_confirmed":true,
                "pull":{"username":"reader","secret_file":root.path().join("pull"),"expires_at":null},
                "publish":{"username":"builder","secret_file":root.path().join("push"),"expires_at":null}
            }]}
        }))
        .unwrap();
        settings.registries.as_mut().unwrap().bindings[0].repository = profile.image.clone();
        let made = Request::new(
            "created-by".into(),
            repository.clone(),
            "a".repeat(40),
            profile.clone(),
            root.path().join("cloud"),
            settings,
        );
        match creator {
            Some((actor, session)) => made.with_creator(actor, session),
            None => made,
        }
    };
    // The agent that asked is recorded on the cloud, with the session that named it.
    let agent = request(Some(("horizon:agent", Some("session-1".into()))));
    prepare(&agent).unwrap();
    let store = Store::lock(&agent.state_root).unwrap();
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        saved.created_by,
        Some(crate::cloud_runtime::state::Creator {
            actor: "horizon:agent".into(),
            session: Some("session-1".into()),
        })
    );
    assert_eq!(saved.version, 1, "a creator changes no record version");
    let bytes = std::fs::read(agent.state_root.join("deployment.json")).unwrap();
    assert!(
        std::str::from_utf8(&bytes).unwrap().contains("\"created_by\""),
        "the saved record carries the creator"
    );
    drop(store);
    // A cloud a person created omits it, so an earlier record keeps its exact encoding.
    std::fs::remove_dir_all(&agent.state_root).unwrap();
    let person = request(None);
    prepare(&person).unwrap();
    let store = Store::lock(&person.state_root).unwrap();
    assert_eq!(store.load().unwrap().unwrap().created_by, None);
    let bytes = std::fs::read(person.state_root.join("deployment.json")).unwrap();
    assert!(
        !std::str::from_utf8(&bytes).unwrap().contains("created_by"),
        "a cloud without a creator keeps the encoding an earlier build wrote"
    );
}

#[test]
#[cfg(all(unix, feature = "tailnet-node-cleanup"))]
fn a_creator_survives_the_legacy_split_of_its_cloud() {
    use crate::cloud_runtime::allocation::{AllocationId, ControllerId, ProjectId, ProjectIdentity, legacy::Records};
    let record = serde_json::json!({
        "version":1,"cloud_id":"split-cloud","repository":"/synthetic/app","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker-1"},"spec":null,"worker":null,
        "sessions":[],
        "created_by":{"actor":"horizon:agent","session":"session-1"}
    });
    let identity = ProjectIdentity::new(
        ProjectId::generate(),
        "session-1".into(),
        "workspace-1".into(),
        "split-cloud".into(),
    )
    .unwrap();
    let pair = Records::from_legacy(
        &serde_json::to_vec(&record).unwrap(),
        identity,
        AllocationId::generate(),
        ControllerId::generate(),
    )
    .unwrap();
    // A reconcile, resume or rebuild reads the split record back as a deployment, which
    // keeps the lease: without it the cloud would belong to no agent after the split.
    assert_eq!(
        pair.deployment().created_by,
        Some(crate::cloud_runtime::state::Creator {
            actor: "horizon:agent".into(),
            session: Some("session-1".into()),
        })
    );
    let mut without = record.clone();
    without.as_object_mut().unwrap().remove("created_by");
    let identity = ProjectIdentity::new(
        ProjectId::generate(),
        "session-1".into(),
        "workspace-1".into(),
        "split-cloud".into(),
    )
    .unwrap();
    let pair = Records::from_legacy(
        &serde_json::to_vec(&without).unwrap(),
        identity,
        AllocationId::generate(),
        ControllerId::generate(),
    )
    .unwrap();
    assert_eq!(
        pair.deployment().created_by,
        None,
        "a person's cloud still splits without a creator"
    );
}

#[test]
#[cfg(all(unix, feature = "tailnet-node-cleanup"))]
fn absence_reports_only_what_the_record_and_its_provider_journal_confirm() {
    let root = tempfile::tempdir().unwrap();
    let state_root = root.path().join("absence-cloud");
    std::fs::create_dir_all(&state_root).unwrap();
    let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
        "provider":"runpod","image":"example/worker","cpu":2,"memory_gb":4
    }))
    .unwrap();
    let record = |stage: &str, operation: serde_json::Value| -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"absence-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":stage,"operation":operation,
            "spec":{"operation_id":"absence-cloud","image_digest":format!("example/worker@sha256:{}", "a".repeat(64)),
                "profile":profile,"public_key":"unused-fixture-key","registry_auth_id":null,
                "gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]},
            "worker":null,"sessions":[]
        }))
        .unwrap()
    };
    let terminated = || serde_json::json!({"state":"terminated","worker_id":"worker-1"});
    let save = |state: &Deployment| {
        let store = Store::lock(&state_root).unwrap();
        store.save(state).unwrap();
    };
    let journal = state_root.join("workspace-volume.json");
    // The worker's record and its provider journal both confirm that nothing remains.
    save(&record("Deleted", terminated()));
    assert_eq!(absence(&state_root).unwrap(), Absence::Proven);
    // A live workspace volume is a provider resource that still exists.
    let worker = record("Deleted", terminated()).spec.unwrap();
    let size = u32::from(worker.profile.storage.volume_gb);
    let data_center = "EU-TEST-1";
    let bound = serde_json::json!({
        "version":1,
        "worker":worker,
        "spec":{"operation_id":"absence-cloud","size":size,"data_center_id":data_center,"tier":"STANDARD"},
        "state":{"state":"bound","volume":{
            "id":"volume1","name":format!("horizon-volume-{}", worker.operation_id),
            "size":size,"dataCenterId":data_center
        }}
    });
    std::fs::write(&journal, serde_json::to_vec(&bound).unwrap()).unwrap();
    assert_eq!(absence(&state_root).unwrap(), Absence::MayRemain);
    // A record that never reached the deleted step proves no absence, also with no journal.
    std::fs::remove_file(&journal).unwrap();
    save(&record(
        "Ready",
        serde_json::json!({"state":"bound","worker_id":"worker-1"}),
    ));
    assert_eq!(absence(&state_root).unwrap(), Absence::Unconfirmed);
}
