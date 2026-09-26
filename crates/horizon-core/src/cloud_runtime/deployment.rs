//! Deployment orchestration. Credentials, images and source are ready before allocation.
mod readiness;
mod redeploy;
pub mod replacement;
pub(super) mod storage;

use super::{
    Error, Event, Result, Stage, WorkerContract,
    command::Runner,
    image::Images,
    repository,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, OperationId, ReadyHistory, Store},
};
use horizon_cloud::{Cancellation, CreateState, ImageSide, Worker, WorkerSpec, runpod::RunPod};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
const RELAUNCH: Duration = Duration::from_mins(1);
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
    let timeline = super::timeline::Recorder::default();
    let recorded = |event: Event| {
        timeline.observe(&event);
        emit(event);
    };
    let emit: &dyn Fn(Event) = &recorded;
    emit(Event::stage(Stage::Validate));
    if !horizon_cloud::valid_id(&request.cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    let store = Store::lock(&request.state_root)?;
    let provider = RunPod::new(request.settings.credential()?);
    super::settings::validate_ssh_identity(&request.settings.ssh_identity_file)?;
    let mut state = initial_state(request, &store)?;
    let reconnected = matches!(state.operation, CreateState::Bound { .. });
    if state.stage == Stage::Deleted {
        let public_key = current_public_key(&request.settings.ssh_identity_file)?;
        redeploy::reopen(&store, &mut state, &public_key)?;
    }
    if assign_requested_size(request, &mut state)? {
        store.save(&state)?;
    }
    replacement::settle(&provider, &store, &mut state, cancel)?;
    let started = attempt_started(&state, started);
    let mut registry = prepare_registry(request, &store, &mut state)?;
    let runner = Runner {
        cancel,
        emit,
        secrets: registry
            .as_ref()
            .map_or_else(Vec::new, super::registry::Prepared::redactions),
    };
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
    refresh_allocation(request, &store, &mut state)?;
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
        prepare_image(request, &store, &runner, &mut state, registry.as_ref())?;
    }
    verify_registry(&provider, &store, &mut state, registry.as_mut(), cancel)?;
    let spec = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Deployment has no worker specification"))?;
    // Opt-in may be added after a definite provisioning rejection saved a spec.
    // Validate that exact digest on every path that can still allocate a worker.
    validate_allocation_image(request, &state, &spec, git_auth.is_some(), &runner, registry.as_ref())?;
    state.stage = Stage::Provision;
    store.save(&state)?;
    emit(Event::stage(state.stage));
    emit(Event::Progress(super::progress::Progress::activity(
        "Requesting or reconciling worker capacity",
    )));
    provision(&provider, &store, &mut state, &spec, cancel, emit)?;
    let (connection, contract) = readiness::wait(request, &provider, &store, &runner, &mut state, &spec)?;
    let container_started = contract.container_started;
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
    let relaunch = |command: &str| runner.run("Session relaunch", &mut connection.command(command), RELAUNCH);
    replacement::relaunch_sessions(&store, &mut state, contract, emit, relaunch)?;
    state.timeline = Some(timeline.finish(reconnected, container_started, std::time::SystemTime::now()));
    finish_ready(state, &store, started, emit)
}

fn prepare_registry(
    request: &Request,
    store: &Store,
    state: &mut Deployment,
) -> Result<Option<super::registry::Prepared>> {
    if !state.resizable() {
        return Ok(None);
    }
    let binding = request
        .settings
        .registries
        .as_ref()
        .map(|config| config.select(&state.profile.image))
        .transpose()?
        .flatten();
    if state.registry_generation.is_some() && binding.is_none() {
        return Err(Error::Invalid(
            "Private image registry binding is missing; restore or rotate it before retrying",
        ));
    }
    if let Some(binding) = binding {
        // Persist private intent before credential loading or image preparation can fail.
        state.registry_generation = Some(binding.generation.clone());
        store.save(state)?;
    }
    super::registry::Prepared::for_image(
        &request.settings,
        &state.profile.image,
        Some(&state.repository),
        state.spec.is_none() && state.profile.build.is_some(),
    )
}

fn verify_registry(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    registry: Option<&mut super::registry::Prepared>,
    cancel: &Cancellation,
) -> Result<()> {
    if let Some(registry) = registry {
        let spec = state
            .spec
            .as_mut()
            .ok_or(Error::Invalid("Deployment has no image to validate"))?;
        registry.verify_image(&spec.image_digest, cancel)?;
        spec.registry_auth_id = Some(registry.ensure_provider(provider, cancel)?);
        store.save(state)?;
    }
    Ok(())
}

fn provision(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let mut operation = state.operation.clone();
    let volume = storage::prepare(provider, store, state, spec, cancel)?;
    let worker = provider.ensure_with_volume(
        spec,
        &mut operation,
        volume.as_ref(),
        cancel,
        |next| {
            state.operation = next.clone();
            store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)
        },
        |progress| emit(Event::Output(format!("{progress:?}"))),
    )?;
    state.worker = Some(worker);
    Ok(())
}

fn begin_sessions(state: &mut Deployment, store: &Store, emit: &dyn Fn(Event)) -> Result<()> {
    state.source_ready = true;
    state.stage = Stage::Sessions;
    store.save(state)?;
    emit(Event::stage(state.stage));
    Ok(())
}

fn attempt_started(state: &Deployment, now: Instant) -> Option<Instant> {
    (state.ready_after_seconds.is_none()
        && state.ready_history == ReadyHistory::Unobserved
        && state.stage != Stage::Ready)
        .then_some(now)
}

fn finish_ready(
    mut state: Deployment,
    store: &Store,
    started: Option<Instant>,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    state.stage = Stage::Ready;
    state.ready_history = ReadyHistory::Observed;
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
    registry: Option<&super::registry::Prepared>,
) -> Result<()> {
    if state.operation == CreateState::Prepared {
        Images {
            docker_host: request.settings.docker_host.as_deref(),
            isolated_registry: registry.is_some(),
            docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(false)
            }),
            runner,
        }
        .validate_contract(&spec.image_digest, &state.cloud_id, &state.profile, git_auth)?;
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
    let state = if let Some(mut state) = store.load()? {
        // CPU and memory may change until a worker is requested; the image does not depend on them.
        let mut resized = state.profile.clone();
        resized.cpu = request.profile.cpu;
        resized.memory_gb = request.profile.memory_gb;
        if state.cloud_id != request.cloud_id
            || state.revision != request.revision
            || state.repository != request.repository
            || resized != request.profile
            || (state.profile != resized && !state.accepts_next_size())
        {
            return Err(Error::Invalid(
                "Cloud is permanently bound to its repository, revision and profile",
            ));
        }
        // A deleted cloud keeps the recorded spec until redeploy releases its journal.
        if state.resizable() {
            assign_requested_size(request, &mut state)?;
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
            registry_generation: None,
            worker: None,
            sessions: Vec::new(),
            source_ready: false,
            ready_after_seconds: None,
            ready_history: ReadyHistory::Unobserved,
            stop_requested: false,
            browserstack_released: true,
            browserstack_targets: std::collections::BTreeSet::new(),
            image_replacement: None,
            session_restart: None,
            timeline: None,
        }
    };
    store.save(&state)?;
    Ok(state)
}

fn current_public_key(identity: &std::path::Path) -> Result<String> {
    let path = PathBuf::from(format!("{}.pub", identity.display()));
    let key = std::fs::read_to_string(&path)?.trim().to_owned();
    if !horizon_cloud::valid_public_key(&key) {
        return Err(Error::Invalid(
            "Replacement worker requires the current Ed25519 public key",
        ));
    }
    Ok(key)
}

/// Applies CPU and memory once the deployment fence allows it.
fn assign_requested_size(request: &Request, state: &mut Deployment) -> Result<bool> {
    let mut resized = state.profile.clone();
    resized.cpu = request.profile.cpu;
    resized.memory_gb = request.profile.memory_gb;
    if resized != request.profile || (state.profile != resized && !state.resizable()) {
        return Err(Error::Invalid(
            "Cloud is permanently bound to its repository, revision and profile",
        ));
    }
    if state.profile == resized {
        return Ok(false);
    }
    let cpu_flavors = cpu_flavors(&resized, &request.settings)?;
    if let Some(spec) = &mut state.spec {
        spec.profile.clone_from(&resized);
        spec.cpu_flavors = cpu_flavors;
    }
    state.profile = resized;
    Ok(true)
}

fn prepare_image(
    request: &Request,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    registry: Option<&super::registry::Prepared>,
) -> Result<()> {
    let build_root = tempfile::tempdir_in(store.root())?;
    let source = if state.profile.build.is_some() {
        repository::snapshot(&state.repository, &state.revision, build_root.path(), runner)?
    } else {
        state.repository.clone()
    };
    let images = Images {
        docker_host: request.settings.docker_host.as_deref(),
        isolated_registry: registry.is_some(),
        docker_config: registry.map_or(request.settings.docker_config.as_path(), |registry| {
            registry.docker_config(state.profile.build.is_some())
        }),
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
        cpu_flavors: cpu_flavors(&state.profile, &request.settings)?,
        data_centers: request.settings.data_centers.clone(),
        startup_metadata: None,
    });
    store.save(state)
}
/// Size and machine settings apply until a worker is requested. Flavors are
/// chosen before the image build so an unavailable size fails in seconds.
fn refresh_allocation(request: &Request, store: &Store, state: &mut Deployment) -> Result<()> {
    if !state.resizable() {
        return Ok(());
    }
    let cpu_flavors = cpu_flavors(&state.profile, &request.settings)?;
    if let Some(spec) = &mut state.spec {
        spec.profile.clone_from(&state.profile);
        spec.cpu_flavors = cpu_flavors;
        spec.gpu_types.clone_from(&request.settings.gpu_types);
        spec.data_centers.clone_from(&request.settings.data_centers);
        store.save(state)?;
    }
    Ok(())
}
fn cpu_flavors(profile: &horizon_cloud::Profile, settings: &Settings) -> Result<Vec<String>> {
    if profile.gpu {
        return Ok(settings.cpu_flavors.clone());
    }
    Ok(horizon_cloud::runpod::flavors::for_profile(
        profile,
        &settings.cpu_flavors,
    )?)
}
/// # Errors
/// Terminates only the persisted, identity-checked worker; never called on UI drop.
/// Each step is emitted as a deletion stage; the saved record moves straight to `Deleted`.
pub fn terminate(
    root: &std::path::Path,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    let replacement = state.replacement_worker()?;
    let provider = RunPod::new(settings.credential()?);
    if state.operation != CreateState::Prepared {
        let runner = Runner {
            cancel,
            emit,
            secrets: Vec::new(),
        };
        terminate_worker(&provider, &store, &mut state, (spec, replacement), settings, &runner)?;
    }
    deletion_step(
        emit,
        Stage::DeleteStorage,
        "Deleting managed workspace storage and confirming its removal",
    );
    storage::terminate(&provider, &store, &state, &committed(), emit)?;
    finish_deletion(state, &store)
}

/// `specs` holds the recorded worker and, while a replacement is journaled, the
/// worker on its replacement image.
fn terminate_worker(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    specs: (WorkerSpec, Option<WorkerSpec>),
    settings: &Settings,
    runner: &Runner<'_>,
) -> Result<()> {
    let cancel = runner.cancel;
    let release = state.requires_browserstack_release();
    let first = if release {
        Stage::ReleaseDevices
    } else {
        Stage::DeleteWorker
    };
    let (spec, replacement) = specs;
    let mut operation = state.operation.clone();
    if operation == CreateState::Requested {
        deletion_step(runner.emit, first, "Reconciling the requested worker");
        provider.ensure(
            &spec,
            &mut operation,
            cancel,
            |next| {
                state.operation = next.clone();
                store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)
            },
            |_| {},
        )?;
    }
    let spec = match (replacement, &operation) {
        (Some(next), CreateState::Bound { worker_id } | CreateState::Terminated { worker_id }) => {
            deletion_step(runner.emit, first, "Identifying which image the worker runs");
            reported_spec(provider.inspect(worker_id, cancel)?.as_ref(), spec, next)?
        }
        _ => spec,
    };
    if release && let CreateState::Bound { worker_id } = &operation {
        deletion_step(runner.emit, Stage::ReleaseDevices, "Confirming worker identity");
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
        super::browser_auth::revoke(&connection, runner)?;
        state.browserstack_released = true;
        store.save(state)?;
    }
    let committed = commit_deletion(cancel)?;
    deletion_step(
        runner.emit,
        Stage::DeleteWorker,
        "Deleting the worker and confirming its removal",
    );
    provider.terminate_with_progress(
        &spec,
        &mut operation,
        &committed,
        |next| {
            state.operation = next.clone();
            store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)
        },
        request_detail(runner.emit),
    )?;
    Ok(())
}

/// The token for delete requests. A sent delete cannot be recalled, and a cancelled
/// confirmation would report an accepted delete as failed, so cancellation ends once
/// deletion starts; the provider timeout still bounds every request.
fn committed() -> Cancellation {
    Cancellation::default()
}

/// Ends the cancellable part of a worker deletion. A cancellation requested while
/// an earlier step was shown stops here, before the irreversible delete request.
fn commit_deletion(cancel: &Cancellation) -> Result<Cancellation> {
    cancel.check()?;
    Ok(committed())
}

fn deletion_step(emit: &dyn Fn(Event), stage: Stage, detail: &str) {
    emit(Event::stage(stage));
    emit(Event::Progress(super::progress::Progress::activity(detail)));
}

/// Replaces the current step's detail with each provider request before it is sent.
fn request_detail(emit: &dyn Fn(Event)) -> impl FnMut(horizon_cloud::Progress) {
    move |request| {
        emit(Event::Progress(super::progress::Progress::activity(
            request.to_string(),
        )));
    }
}

/// Commits a requested image replacement once the provider reports the new image on
/// every API. The storage journal is rebound first, then one deployment write switches
/// the worker specification; after a crash in between, the replacement is still
/// requested and committing again completes it. No provider I/O.
/// # Errors
/// Refuses unless the replacement was requested, and reports persistence failures.
pub fn commit_replacement(store: &Store, state: &mut Deployment) -> Result<OperationId> {
    commit_with(store, state, || Ok(()))
}

/// `rebound` runs between the storage and deployment writes.
fn commit_with(store: &Store, state: &mut Deployment, rebound: impl FnOnce() -> Result<()>) -> Result<OperationId> {
    let previous = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let mut committed = state.clone();
    let operation = committed.commit_replacement()?;
    let next = committed
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    storage::rebind(store, &previous, next)?;
    rebound()?;
    store.save(&committed)?;
    *state = committed;
    Ok(operation)
}

/// While a journaled replacement has a built image, the worker may run either image of
/// the pair. Delete it as the one it reports; the provider verifies that again strictly.
fn reported_spec(worker: Option<&Worker>, current: WorkerSpec, next: WorkerSpec) -> Result<WorkerSpec> {
    let side = worker.map(|worker| worker.verify_either(&current, &next)).transpose()?;
    Ok(if side == Some(ImageSide::Next) { next } else { current })
}

fn finish_deletion(mut state: Deployment, store: &Store) -> Result<()> {
    drop_replacement(store, &mut state)?;
    state.stage = Stage::Deleted;
    state.worker = None;
    store.save(&state)
}

/// A deleted worker has no image to replace and no sessions to relaunch. An interrupted
/// commit may have rebound the storage journal to the replacement's worker, so it is
/// bound back to the recorded one, which later storage checks compare against.
pub(in crate::cloud_runtime) fn drop_replacement(store: &Store, state: &mut Deployment) -> Result<()> {
    if let (Some(next), Some(current)) = (state.replacement_worker()?, state.spec.as_ref()) {
        storage::rebind(store, &next, current)?;
    }
    state.image_replacement = None;
    state.session_restart = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_deletion_commits_only_before_its_caller_cancels() {
        let cancel = Cancellation::default();
        let committed = commit_deletion(&cancel).unwrap();
        cancel.cancel();
        assert!(!committed.is_cancelled(), "a sent delete ignores later cancellation");
        assert!(matches!(
            commit_deletion(&cancel),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
    }
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
        let mut request = Request {
            cloud_id: state.cloud_id.clone(),
            repository: state.repository.clone(),
            revision: state.revision.clone(),
            profile: state.profile.clone(),
            state_root: root.path().into(),
            settings,
        };
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
        let mut request = Request {
            cloud_id: "resize".into(),
            repository: root.path().join("repository-is-not-mounted"),
            revision: "a".repeat(40),
            profile: serde_json::from_value(
                serde_json::json!({"provider":"runpod","image":"registry.example.com/worker","cpu":8,"memory_gb":32}),
            )
            .unwrap(),
            state_root: root.path().join("state"),
            settings: serde_json::from_value(
                serde_json::json!({"runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused","registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[]}),
            )
            .unwrap(),
        };
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
    fn deletion_identifies_a_replacing_worker_by_either_image() {
        let current: WorkerSpec = serde_json::from_value(serde_json::json!({
            "operation_id":"replacing","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "public_key":"unused","registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        }))
        .unwrap();
        let mut next = current.clone();
        next.image_digest = format!("registry.example/worker@sha256:{}", "b".repeat(64));
        let worker = |image: &str, name: &str| -> Worker {
            serde_json::from_value(serde_json::json!({
                "id":"worker1","name":name,"imageName":image,"desiredStatus":"RUNNING",
                "env":{"HORIZON_CLOUD_OPERATION":"replacing"}
            }))
            .unwrap()
        };
        let name = current.name();
        for (reported, expected) in [
            (Some(worker(&current.image_digest, &name)), &current),
            (Some(worker(&next.image_digest, &name)), &next),
            (None, &current),
        ] {
            assert_eq!(
                reported_spec(reported.as_ref(), current.clone(), next.clone()).unwrap(),
                *expected
            );
        }
        let third = format!("registry.example/worker@sha256:{}", "c".repeat(64));
        for reported in [worker(&third, &name), worker(&next.image_digest, "horizon-cloud-other")] {
            assert!(reported_spec(Some(&reported), current.clone(), next.clone()).is_err());
        }
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
        let request = Request {
            cloud_id: "reconnect".into(),
            // Unmounted, so an attempt that passes the guard stops before provider I/O.
            repository: root.path().join("repository-is-not-mounted"),
            revision: "a".repeat(40),
            profile: serde_json::from_value(profile.clone()).unwrap(),
            state_root: root.path().join("state"),
            settings: serde_json::from_value(serde_json::json!({
                "runpod_key_file":key.path(),"ssh_identity_file":identity.path(),
                "docker_config":root.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
            }))
            .unwrap(),
        };
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
    fn gpu_profiles_keep_configured_cpu_flavors() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused", "ssh_identity_file":"unused", "docker_config":"unused",
            "registry_pull_auth_id":null, "cpu_flavors":["cpu3c"], "gpu_types":["fixture-gpu"]
        }))
        .unwrap();
        let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
            "provider":"runpod","image":"registry.example.com/worker","cpu":3,"memory_gb":100,"gpu":true
        }))
        .unwrap();
        assert_eq!(cpu_flavors(&profile, &settings).unwrap(), ["cpu3c"]);
    }
    #[test]
    fn each_provider_deletion_request_becomes_the_current_detail() {
        use horizon_cloud::Progress;
        let details = std::cell::RefCell::new(Vec::new());
        let emit = |event: Event| {
            if let Event::Progress(progress) = event {
                details.borrow_mut().push(progress.detail);
            }
        };
        let mut forward = request_detail(&emit);
        for request in [
            Progress::ConfirmingWorker,
            Progress::Terminating,
            Progress::ConfirmingTermination,
            Progress::ConfirmingVolume,
            Progress::CheckingAttachments,
            Progress::InspectingMounts { worker: 2, workers: 3 },
            Progress::DeletingVolume,
            Progress::ConfirmingVolumeDeletion,
        ] {
            forward(request);
        }
        assert_eq!(
            details.take(),
            [
                "Confirming worker identity",
                "Requesting worker deletion",
                "Confirming worker removal",
                "Confirming workspace storage identity",
                "Checking workspace storage attachments",
                "Checking workspace storage attachments · worker 2 of 3",
                "Requesting workspace storage deletion",
                "Confirming workspace storage removal",
            ]
        );
    }
}
