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
    pub revision: String,
    pub profile: horizon_cloud::Profile,
    pub state_root: PathBuf,
    pub settings: Settings,
}
/// # Errors
/// Saves an initial retryable record before the UI persists deployment intent.
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
    emit(Event::stage(Stage::Validate));
    if !horizon_cloud::valid_id(&request.cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    let store = Store::lock(&request.state_root)?;
    let provider = RunPod::new(request.settings.credential()?);
    let runner = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let mut state = initial_state(request, &store)?;
    let git_auth = super::git_auth::Prepared::for_repository(&request.settings.git_credentials, &state.repository)?;
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
    if state.operation == CreateState::Prepared {
        Images {
            docker_host: request.settings.docker_host.as_deref(),
            docker_config: &request.settings.docker_config,
            runner: &runner,
        }
        .validate_contract(
            &spec.image_digest,
            &state.cloud_id,
            &state.profile.capabilities,
            git_auth.is_some(),
        )?;
    }
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
        state.source_ready = true;
    }
    emit(Event::stage(Stage::Sessions));
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
    state.stage = Stage::Ready;
    store.save(&state)?;
    emit(Event::ready(Box::new(state.clone())));
    Ok(state)
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
            revision: repository::resolve(&request.repository, &request.revision)?,
            profile: request.profile.clone(),
            stage: Stage::Validate,
            operation: CreateState::Prepared,
            spec: None,
            worker: None,
            sessions: Vec::new(),
            source_ready: false,
            stop_requested: false,
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
    provider.terminate(&spec, &mut operation, cancel, |next| {
        state.operation = next.clone();
        store.save(&state).map_err(|_| horizon_cloud::CloudError::Persistence)
    })?;
    state.stage = Stage::Deleted;
    state.worker = None;
    store.save(&state)
}
