use super::*;
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
    request.settings.registries = None;
    state.operation = CreateState::Requested;
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
