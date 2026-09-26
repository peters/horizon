//! Deployment orchestration. Credentials, images and source are ready before allocation.
mod agent_credentials;
mod deletion;
mod git_credentials;
mod image;
mod readiness;
mod redeploy;
pub mod replacement;
mod sizing;
mod source;
pub(super) mod storage;
#[cfg(all(test, unix))]
mod tests;

use agent_credentials::{configure_agent_auth, validate_agent_auth};
pub use deletion::terminate;
use git_credentials::configure_git_auth;
use image::{prepare_image, validate_allocation_image};
use sizing::{assign_requested_size, refresh_allocation};

use super::{
    Error, Event, Result, Stage, WorkerContract,
    command::Runner,
    repository,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, OperationId, ReadyHistory, Store},
};
use horizon_cloud::{Cancellation, CreateState, WorkerSpec, runpod::RunPod};
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

impl Request {
    #[must_use]
    pub fn new(
        cloud_id: String,
        repository: PathBuf,
        revision: String,
        profile: horizon_cloud::Profile,
        state_root: PathBuf,
        settings: Settings,
    ) -> Self {
        Self {
            cloud_id,
            repository,
            revision,
            profile,
            state_root,
            settings,
        }
    }
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
    let reconnected = super::timeline::reconnects(&state);
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
    let packed = source::pack(&state, pack_root.path(), &runner)?;
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
    source::transfer(&connection, &store, &mut state, packed, &runner, emit)?;
    begin_sessions(&mut state, &store, emit)?;
    configure_agent_auth(&connection, &request.settings, &state.profile.capabilities, &runner)?;
    configure_git_auth(git_auth, &connection, &runner)?;
    if let Some(browser_auth) = browser_auth {
        state.browserstack_targets.clone_from(browser_auth.targets());
        store.arm_browserstack(&mut state)?;
        browser_auth.install(&connection, &runner)?;
    }
    let relaunch = |command: &str| runner.run("Session relaunch", &mut connection.command(command), RELAUNCH);
    replacement::relaunch_sessions(&store, &mut state, &contract, emit, relaunch)?;
    state.timeline = Some(timeline.complete(&state, reconnected, contract.container_started));
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
            last_self_stop: None,
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
