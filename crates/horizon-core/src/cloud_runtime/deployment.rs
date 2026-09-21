//! Deployment orchestration. Credentials, images and source are ready before allocation.
use super::{
    Error, Event, Result, Stage,
    command::Runner,
    image::Images,
    repository,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, Store},
};
use horizon_cloud::{Cancellation, CreateState, WorkerSpec, runpod::RunPod};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub struct Request {
    pub cloud_id: String,
    pub repository: PathBuf,
    /// Immutable commit ID resolved by the caller before preparation.
    pub revision: String,
    pub profile: horizon_cloud::Profile,
    pub state_root: PathBuf,
    pub settings: Settings,
}
/// # Errors
/// Saves a retryable record for an already resolved commit before persisting deployment intent.
/// Does not invoke Git; deployment validates the committed tree before allocation.
pub fn prepare(request: &Request) -> Result<()> {
    if !horizon_cloud::valid_id(&request.cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    let store = Store::lock(&request.state_root)?;
    initial_state(request, &store).map(|_| ())
}
/// # Errors
/// Records any uncertain allocation durably and never deletes workers on disconnect.
pub fn deploy(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    let started = Instant::now();
    emit(Event::stage(Stage::Validate));
    if !horizon_cloud::valid_id(&request.cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    let store = Store::lock(&request.state_root)?;
    let provider = RunPod::new(request.settings.credential()?);
    super::settings::validate_ssh_identity(&request.settings.ssh_identity_file)?;
    let runner = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let mut state = initial_state(request, &store)?;
    let started = (state.operation == CreateState::Prepared && state.worker.is_none()).then_some(started);
    let git_auth = super::git_auth::Prepared::for_repository(&request.settings.git_credentials, &state.repository)?;
    let browser_auth = super::browser_auth::Prepared::for_repository(
        &request.settings.browserstack_credentials,
        &state.repository,
        state.profile.capabilities.browserstack.as_ref(),
        &state.cloud_id,
    )?;
    if state.stop_requested {
        return Err(Error::Invalid(
            "Worker was explicitly stopped. Resume it before reconnecting; prior processes may be lost.",
        ));
    }
    validate_agent_auth(&request.settings, &state.profile.capabilities)?;
    let pack_root = tempfile::tempdir_in(store.root())?;
    let pack = pack_root.path().join("source.pack");
    let mut auxiliary = None;
    if !state.source_ready {
        repository::validate_tree(&state.repository, &state.revision, &runner)?;
        repository::pack(&state.repository, &state.revision, &pack, &runner)?;
        auxiliary = Some(repository::auxiliary(
            &state.repository,
            &state.revision,
            pack_root.path(),
            &runner,
        )?);
    }
    if state.spec.is_none() {
        prepare_image(request, &store, &runner, &mut state)?;
    }
    let spec = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Deployment has no worker specification"))?;
    // Opt-in may be added after a definite provisioning rejection saved a spec.
    // Validate that exact digest on every path that can still allocate a worker.
    validate_allocation_image(request, &state, &spec, git_auth.is_some(), &runner)?;
    state.stage = Stage::Provision;
    store.save(&state)?;
    emit(Event::stage(state.stage));
    emit(Event::Progress(super::progress::Progress::activity(
        "Requesting or reconciling worker capacity",
    )));
    let mut operation = state.operation.clone();
    let worker = provider.ensure(
        &spec,
        &mut operation,
        cancel,
        |next| {
            state.operation = next.clone();
            store.save(&state).map_err(|_| horizon_cloud::CloudError::Persistence)
        },
        |progress| emit(Event::Output(format!("{progress:?}"))),
    )?;
    state.worker = Some(worker);
    let connection = readiness(request, &provider, &store, &runner, &mut state, &spec)?;
    if !state.source_ready {
        state.stage = Stage::Worktrees;
        store.save(&state)?;
        emit(Event::stage(state.stage));
        connection.transfer(&pack, &state.revision, &runner)?;
        if let Some(auxiliary) = auxiliary {
            connection.transfer_material(&auxiliary, &runner)?;
        }
    }
    begin_sessions(&mut state, &store, emit)?;
    configure_agent_auth(&connection, &request.settings, &state.profile.capabilities, &runner)?;
    if let Some(git_auth) = git_auth {
        git_auth.install(&connection, &runner)?;
    } else {
        runner.run(
            "Git credential removal",
            &mut connection.command(
                "if command -v horizon-worker-git-auth >/dev/null 2>&1; then horizon-worker-git-auth clear; fi",
            ),
            Duration::from_secs(20),
        )?;
    }
    if let Some(browser_auth) = browser_auth {
        state.browserstack_targets.clone_from(browser_auth.targets());
        store.arm_browserstack(&mut state)?;
        browser_auth.install(&connection, &runner)?;
    }
    finish_ready(state, &store, started, emit)
}

fn begin_sessions(state: &mut Deployment, store: &Store, emit: &dyn Fn(Event)) -> Result<()> {
    state.source_ready = true;
    state.stage = Stage::Sessions;
    store.save(state)?;
    emit(Event::stage(state.stage));
    Ok(())
}

fn finish_ready(
    mut state: Deployment,
    store: &Store,
    started: Option<Instant>,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    state.stage = Stage::Ready;
    if let Some(started) = started {
        state
            .ready_after_seconds
            .get_or_insert_with(|| started.elapsed().as_secs());
    }
    store.save(&state)?;
    emit(Event::ready(Box::new(state.clone())));
    Ok(state)
}
fn validate_allocation_image(
    request: &Request,
    state: &Deployment,
    spec: &WorkerSpec,
    git_auth: bool,
    runner: &Runner<'_>,
) -> Result<()> {
    if state.operation == CreateState::Prepared {
        Images {
            docker_host: request.settings.docker_host.as_deref(),
            docker_config: &request.settings.docker_config,
            runner,
        }
        .validate_contract(
            &spec.image_digest,
            &state.cloud_id,
            &state.profile.capabilities,
            git_auth,
        )?;
    }
    Ok(())
}

fn validate_agent_auth(settings: &Settings, capabilities: &horizon_cloud::Capabilities) -> Result<()> {
    for path in [
        ("claude", &settings.anthropic_api_key_file),
        ("codex", &settings.openai_api_key_file),
    ]
    .into_iter()
    .filter(|(agent, _)| capabilities.permits_agent(agent))
    .filter_map(|(_, path)| path.as_ref())
    {
        super::settings::validate_private_key_file(path)?;
    }
    Ok(())
}
fn configure_agent_auth(
    connection: &Connection,
    settings: &Settings,
    capabilities: &horizon_cloud::Capabilities,
    runner: &Runner<'_>,
) -> Result<()> {
    let clear = format!(
        "python3 - {} {} {} <<'HORIZON_AUTH_CLEANUP'\n{}\nHORIZON_AUTH_CLEANUP",
        u8::from(!capabilities.permits_agent("claude") || settings.anthropic_api_key_file.is_none()),
        u8::from(!capabilities.permits_agent("codex") || settings.openai_api_key_file.is_none()),
        u8::from(!capabilities.permits_agent("claude") || settings.anthropic_workspace_id.is_none()),
        include_str!("deployment/clear_agent_auth.py"),
    );
    runner.run(
        "Removed agent credential bindings",
        &mut connection.command(&clear),
        Duration::from_secs(20),
    )?;
    if capabilities.permits_agent("claude")
        && let Some(path) = &settings.anthropic_api_key_file
    {
        runner.private_input(&mut connection.command(
            "umask 077; mkdir -p /workspace/credentials && cat > /workspace/credentials/anthropic-api-key.new && mv /workspace/credentials/anthropic-api-key.new /workspace/credentials/anthropic-api-key"
        ), path)?;
    }
    if capabilities.permits_agent("codex")
        && let Some(path) = &settings.openai_api_key_file
    {
        runner.private_input(
            &mut connection.command("umask 077; HOME=/workspace/home codex login --with-api-key"),
            path,
        )?;
    }
    if capabilities.permits_agent("claude")
        && let Some(workspace) = &settings.anthropic_workspace_id
    {
        runner.run("Agent workspace binding", &mut connection.command(&format!(
            "umask 077; mkdir -p /workspace/credentials && printf '%s' '{workspace}' > /workspace/credentials/anthropic-workspace"
        )), Duration::from_secs(20))?;
    }
    Ok(())
}
fn initial_state(request: &Request, store: &Store) -> Result<Deployment> {
    if !repository::is_commit_id(&request.revision) {
        return Err(Error::Invalid(
            "Resolve a committed revision before preparing deployment",
        ));
    }
    let state = if let Some(state) = store.load()? {
        if state.cloud_id != request.cloud_id
            || state.revision != request.revision
            || state.repository != request.repository
            || state.profile != request.profile
        {
            return Err(Error::Invalid(
                "Cloud is permanently bound to its repository, revision and profile",
            ));
        }
        state
    } else {
        Deployment {
            version: 1,
            cloud_id: request.cloud_id.clone(),
            repository: request.repository.clone(),
            revision: request.revision.clone(),
            profile: request.profile.clone(),
            stage: Stage::Validate,
            operation: CreateState::Prepared,
            spec: None,
            worker: None,
            sessions: Vec::new(),
            source_ready: false,
            ready_after_seconds: None,
            stop_requested: false,
            browserstack_released: true,
            browserstack_targets: std::collections::BTreeSet::new(),
        }
    };
    store.save(&state)?;
    Ok(state)
}
fn prepare_image(request: &Request, store: &Store, runner: &Runner<'_>, state: &mut Deployment) -> Result<()> {
    let build_root = tempfile::tempdir_in(store.root())?;
    let source = if state.profile.build.is_some() {
        repository::snapshot(&state.repository, &state.revision, build_root.path(), runner)?
    } else {
        state.repository.clone()
    };
    let images = Images {
        docker_host: request.settings.docker_host.as_deref(),
        docker_config: &request.settings.docker_config,
        runner,
    };
    let digest = images.prepare(&state.profile, &source, &state.cloud_id)?;
    let public_key = std::fs::read_to_string(PathBuf::from(format!(
        "{}.pub",
        request.settings.ssh_identity_file.display()
    )))?
    .trim()
    .to_owned();
    state.spec = Some(WorkerSpec {
        operation_id: state.cloud_id.clone(),
        image_digest: digest,
        profile: state.profile.clone(),
        public_key,
        registry_auth_id: request.settings.registry_pull_auth_id.clone(),
        gpu_types: request.settings.gpu_types.clone(),
        cpu_flavors: request.settings.cpu_flavors.clone(),
        data_centers: request.settings.data_centers.clone(),
    });
    store.save(state)
}
fn readiness(
    request: &Request,
    provider: &RunPod,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    spec: &WorkerSpec,
) -> Result<Connection> {
    state.stage = Stage::Readiness;
    store.save(state)?;
    (runner.emit)(Event::stage(state.stage));
    let started = Instant::now();
    loop {
        runner.cancel.check()?;
        let id = &state
            .worker
            .as_ref()
            .ok_or(Error::Invalid("Missing worker identity"))?
            .id;
        let worker = provider
            .inspect(id, runner.cancel)?
            .ok_or(horizon_cloud::CloudError::WorkerLost)?;
        worker.verify(spec)?;
        worker.verify_resources(spec)?;
        let connection = Connection::new(&worker, &request.settings, store.root());
        state.worker = Some(worker);
        store.save(state)?;
        if let Ok(connection) = connection {
            (runner.emit)(Event::Progress(super::progress::Progress::activity(
                "Waiting for SSH and worker services",
            )));
            if connection.ready(runner, &state.profile.capabilities).is_ok() {
                return Ok(connection);
            }
        } else {
            (runner.emit)(Event::Progress(super::progress::Progress::activity(
                "Waiting for the provider to publish an SSH endpoint",
            )));
        }
        if started.elapsed() > Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds)) {
            return Err(Error::Invalid(
                "Worker readiness timed out; worker remains allocated for inspection or explicit deletion",
            ));
        }
        for _ in 0..20 {
            runner.cancel.check()?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
/// # Errors
/// Terminates only the persisted, identity-checked worker; never called on UI drop.
pub fn terminate(root: &std::path::Path, settings: &Settings, cancel: &Cancellation) -> Result<()> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    let provider = RunPod::new(settings.credential()?);
    let mut operation = state.operation.clone();
    if operation == CreateState::Requested {
        provider.ensure(
            &spec,
            &mut operation,
            cancel,
            |next| {
                state.operation = next.clone();
                store.save(&state).map_err(|_| horizon_cloud::CloudError::Persistence)
            },
            |_| {},
        )?;
    }
    if state.requires_browserstack_release()
        && let CreateState::Bound { worker_id } = &operation
    {
        let worker = provider.inspect(worker_id, cancel)?.ok_or(Error::Invalid(
            "Worker is lost; remote-device release must be verified before cleanup can be confirmed",
        ))?;
        worker.verify(&spec)?;
        if worker.status() == horizon_cloud::WorkerStatus::Stopped {
            return Err(Error::Invalid(
                "Resume the worker to release its hosted devices before deletion",
            ));
        }
        let connection = Connection::new(&worker, settings, store.root())?;
        super::browser_auth::revoke(
            &connection,
            &Runner {
                cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            },
        )?;
        state.browserstack_released = true;
        store.save(&state)?;
    }
    provider.terminate(&spec, &mut operation, cancel, |next| {
        state.operation = next.clone();
        store.save(&state).map_err(|_| horizon_cloud::CloudError::Persistence)
    })?;
    state.stage = Stage::Deleted;
    state.worker = None;
    store.save(&state)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn selected_agent_credentials_must_be_nonempty_before_deployment() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused", "ssh_identity_file":"unused", "docker_config":"unused",
            "registry_pull_auth_id":null, "cpu_flavors":[], "gpu_types":[]
        }))
        .unwrap();
        for agent in [horizon_cloud::Agent::Codex, horizon_cloud::Agent::Claude] {
            settings.openai_api_key_file = (agent == horizon_cloud::Agent::Codex).then(|| file.path().into());
            settings.anthropic_api_key_file = (agent == horizon_cloud::Agent::Claude).then(|| file.path().into());
            let selected: horizon_cloud::Capabilities =
                serde_json::from_value(serde_json::json!({"agents":[agent]})).unwrap();
            let disabled: horizon_cloud::Capabilities = serde_json::from_str("{}").unwrap();
            for content in ["", " \n\t\r", "fixture-credential\n"] {
                std::fs::write(file.path(), content).unwrap();
                assert_eq!(
                    validate_agent_auth(&settings, &selected).is_ok(),
                    !content.trim().is_empty()
                );
                assert!(validate_agent_auth(&settings, &disabled).is_ok());
            }
        }
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
        let request = Request {
            cloud_id: "test".into(),
            repository: repo.workdir().unwrap().into(),
            revision: revision.to_string(),
            profile,
            state_root: root.path().join("cloud"),
            settings,
        };
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
        let mut request = Request {
            cloud_id: "pinned-fixture".into(),
            repository: root.path().join("repository-is-not-mounted"),
            revision: "a".repeat(40),
            profile: serde_json::from_value(
                serde_json::json!({"provider":"runpod","image":"registry.example.com/worker","cpu":4,"memory_gb":8}),
            )
            .unwrap(),
            state_root: root.path().join("state"),
            settings: serde_json::from_value(
                serde_json::json!({"runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]}),
            )
            .unwrap(),
        };
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
}
