//! Deployment orchestration. Credentials, images and source are ready before allocation.
mod admission;
pub(super) mod azure;
pub use admission::{Problem, problems as admission_problems};
mod agent_credentials;
mod compute;
pub(in crate::cloud_runtime) use compute::Compute;
pub(in crate::cloud_runtime) mod deletion;
mod git_credentials;
pub(super) mod hetzner;
mod image;
mod readiness;
mod reconnect;
mod redeploy;
pub mod replacement;
mod sizing;
mod source;
pub(super) mod storage;
#[cfg(all(test, unix))]
mod tests;

use agent_credentials::configure_agent_auth;
pub(in crate::cloud_runtime) use agent_credentials::validate_agent_auth;
pub use deletion::terminate;
#[cfg(feature = "tailnet-node-cleanup")]
pub use deletion::{Absence, absence};
use git_credentials::configure_git_auth;
use image::{prepare_image, validate_allocation_image};
pub use reconnect::reconnect;
use sizing::{assign_requested_size, refresh_allocation};
pub use storage::growth::grow_storage;
pub use storage::resize::resize_compute;
pub use storage::{ResizeTarget, pending_resize};

use super::{
    Error, Event, Result, Stage, WorkerContract,
    command::Runner,
    mutation::{self, State as Mutation},
    repository,
    settings::Settings,
    siblings,
    ssh::Connection,
    state::{Deployment, OperationId, ReadyHistory, Store},
};
use horizon_cloud::{Cancellation, CreateState, WorkerSpec, runpod::RunPod};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
const RELAUNCH: Duration = Duration::from_mins(1);
#[derive(Clone)]
pub struct Request {
    pub cloud_id: String,
    pub repository: PathBuf,
    /// Immutable commit ID resolved by the caller before preparation.
    pub revision: String,
    pub profile: horizon_cloud::Profile,
    pub state_root: PathBuf,
    pub settings: Settings,
    /// The person asked this deployment to sign in to GitHub again, for example to add a
    /// repository, although the worker holds access. Only this deployment signs in again.
    pub renew_github: bool,
    /// The image builds from the working copy, with the uncommitted files a person
    /// confirmed in New cloud. The first deployment records the confirmation, and a later
    /// one may only repeat it or name none until the image is built.
    pub working_copy: Option<repository::uncommitted::Confirmed>,
    pub profile_name: Option<String>,
    /// The destination shown before Start; a new build refuses changes to this selection.
    pub publishing_selection: Option<super::publishing::Selection>,
    /// The agent panel that asks for this deployment. It is recorded as the cloud's
    /// creator, and it is the only caller that may delete the cloud it created.
    pub created_by: Option<super::state::Creator>,
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
            renew_github: false,
            working_copy: None,
            profile_name: None,
            publishing_selection: None,
            created_by: None,
        }
    }

    /// Records the agent panel that asks for this deployment, so only that panel deletes
    /// the cloud it created. A person's card records no creator.
    #[must_use]
    pub fn with_creator(mut self, actor: &str, session: Option<String>) -> Self {
        self.created_by = Some(super::state::Creator {
            actor: actor.to_owned(),
            session,
        });
        self
    }
}
/// # Errors
/// Saves a retryable record for an already resolved commit before persisting deployment intent.
/// Does not invoke Git; deployment validates the committed tree before allocation.
pub fn prepare(request: &Request) -> Result<()> {
    prepare_record(request, false)
}

/// Records an image-only preparation without resizing a cloud that already has its image.
/// The existing-image check and any record changes hold the same operation lock.
/// # Errors
/// As [`prepare`], or when a prepared worker specification already exists.
pub fn prepare_image_record(request: &Request) -> Result<()> {
    prepare_record(request, true)
}

fn prepare_record(request: &Request, image_only: bool) -> Result<()> {
    if !horizon_cloud::valid_id(&request.cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    super::providers::preflight(&request.cloud_id, &request.profile, &request.settings)?;
    let store = Store::lock(&request.state_root)?;
    if image_only && store.load()?.is_some_and(|state| state.spec.is_some()) {
        return Err(Error::Invalid(
            "This cloud already has a prepared worker image; use its rebuild flow to prepare a replacement",
        ));
    }
    initial_state(request, &store).map(|_| ())
}
/// # Errors
/// Records any uncertain allocation durably and never deletes workers on disconnect.
pub fn deploy(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    deploy_with_siblings(request, &[], cancel, emit)
}
/// As [`deploy`], first pinning the machine-local same-worker sibling `bindings`, in layering order,
/// before the image is built. Without bindings a recorded set is kept, as on a reconnect.
/// # Errors
/// As [`deploy`], and any sibling refusal.
pub fn deploy_with_siblings(
    request: &Request,
    bindings: &[siblings::Binding],
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    run(request, bindings, |_, _| Ok(()), cancel, emit)
}

/// As [`deploy_with_siblings`], once `admit` accepts the record under the lock.
fn run(
    request: &Request,
    bindings: &[siblings::Binding],
    admit: fn(&Store, &Deployment) -> Result<()>,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    deploy_with(request, bindings, None, admit, cancel, emit, mutation::IGNORE)
}

/// As [`deploy_with_siblings`] under the held `store`, telling `observe` before each
/// provider mutation and once the provider's work is settled.
#[cfg_attr(
    not(all(test, unix)),
    allow(dead_code, reason = "the companion lifecycle service calls it under its own lock")
)]
pub(in crate::cloud_runtime) fn deploy_locked(
    request: &Request,
    bindings: &[siblings::Binding],
    store: &Store,
    admit: fn(&Store, &Deployment) -> Result<()>,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
    observe: mutation::Observer<'_>,
) -> Result<Deployment> {
    deploy_with(request, bindings, Some(store), admit, cancel, emit, observe)
}

/// Deploys under `held`, or under the deployment lock taken once the request passes
/// its checks, so a refused request never contends for the lock.
fn deploy_with(
    request: &Request,
    bindings: &[siblings::Binding],
    held: Option<&Store>,
    admit: fn(&Store, &Deployment) -> Result<()>,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
    observe: mutation::Observer<'_>,
) -> Result<Deployment> {
    let started = Instant::now();
    let timeline = super::timeline::Recorder::default();
    let recorded = |event: Event| {
        timeline.observe(&event);
        emit(event);
    };
    let emit: &dyn Fn(Event) = &recorded;
    emit(Event::stage(Stage::Validate));
    connection_preflight(
        &request.cloud_id,
        &request.revision,
        &request.profile,
        &request.settings,
    )?;
    let taken;
    let store = if let Some(store) = held {
        store
    } else {
        taken = Store::lock(&request.state_root)?;
        &taken
    };
    let provider = super::providers::compute(request)?;
    let mut state = initial_state(request, store)?;
    admit(store, &state)?;
    let reconnected = super::timeline::reconnects(&state);
    if state.stage == Stage::Deleted {
        let public_key = current_public_key(&request.settings.ssh_identity_file)?;
        redeploy::reopen(store, &mut state, &public_key)?;
    }
    if assign_requested_size(request, &mut state)? {
        store.save(&state)?;
    }
    provider.settle(store, &mut state, cancel, observe)?;
    let started = attempt_started(&state, started);
    let publishing_runner = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let mut preparation = image::Preparation::new(request, store, &mut state, &publishing_runner)?;
    let runner = Runner {
        cancel,
        emit,
        secrets: preparation.redactions(),
    };
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
    if siblings::bind(bindings, &mut state, &runner)? {
        store.save(&state)?;
    }
    // Only after binding: the first deploy of a cloud with siblings sends their grants too.
    let git_auth = siblings::git_grants(&request.settings.git_credentials, &state)?;
    validate_agent_auth(&request.settings, &state.profile.capabilities, false)?;
    refresh_allocation(request, store, &mut state)?;
    let pack_root = tempfile::tempdir_in(store.root())?;
    let mut packed = source::pack(&state, pack_root.path(), &runner)?;
    if state.spec.is_none() {
        prepare_image(request, store, &runner, &mut state, &preparation)?;
    }
    if let Some(registry) = preparation.registry.as_mut() {
        provider.verify_registry(store, &mut state, registry, cancel, observe)?;
    }
    let spec = state
        .spec
        .clone()
        .ok_or(Error::Invalid("Deployment has no worker specification"))?;
    // Opt-in may be added after a definite provisioning rejection saved a spec.
    // Validate that exact digest on every path that can still allocate a worker.
    if let Some(image) = validate_allocation_image(
        request,
        &state,
        &spec,
        git_auth.is_some(),
        &runner,
        preparation.registry.as_ref(),
    )? {
        packed.settle(&image, &runner)?;
    }
    state.stage = Stage::Provision;
    store.save(&state)?;
    emit(Event::stage(state.stage));
    emit(Event::Progress(super::progress::Progress::activity(
        "Requesting or reconciling worker capacity",
    )));
    let (connection, contract) = provider.provision_ready(request, store, &runner, &mut state, &spec, observe)?;
    super::tailnet::configure_and_record(&connection, store, &mut state, &contract, &runner)?;
    source::transfer(&connection, store, &mut state, packed, &contract, &runner, emit)?;
    begin_sessions(&mut state, store, emit)?;
    configure_agent_auth(&connection, &request.settings, &state.profile.capabilities, &runner)?;
    configure_git_auth(git_auth, request, &state, &connection, &runner)?;
    install_browser_auth(browser_auth, store, &mut state, &connection, &runner)?;
    // A reconnect brings back every recorded session. A running process is left
    // alone; a lost one is started again in its existing worktree.
    resume_sessions(store, &mut state, &contract, &connection, &runner)?;
    state.timeline = Some(timeline.complete(&state, reconnected, contract.container_started));
    finish_ready(state, store, started, emit)
}

fn install_browser_auth(
    auth: Option<super::browser_auth::Prepared>,
    store: &Store,
    state: &mut Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    if let Some(auth) = auth {
        state.browserstack_targets.clone_from(auth.targets());
        store.arm_browserstack(state)?;
        auth.install(connection, runner)?;
    }
    Ok(())
}

fn resume_sessions(
    store: &Store,
    state: &mut Deployment,
    contract: &WorkerContract,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    if arm_recorded_session_resume(state) {
        store.save(state)?;
    }
    let relaunch = |command: &str| runner.run("Session relaunch", &mut connection.command(command), RELAUNCH);
    replacement::relaunch_sessions(store, state, contract, runner.emit, relaunch)
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
    super::registry::Prepared::for_image_for_provider(
        &request.settings,
        &state.profile.image,
        Some(&state.repository),
        state.spec.is_none()
            && state.profile.build.is_some()
            && state.publication.as_ref().is_none_or(|record| record.account.is_none()),
        horizon_cloud::provider::Description::of(&state.profile).kind,
    )
}

pub(in crate::cloud_runtime) fn connection_preflight(
    cloud_id: &str,
    revision: &str,
    profile: &horizon_cloud::Profile,
    settings: &Settings,
) -> Result<()> {
    if !horizon_cloud::valid_id(cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    if !repository::is_commit_id(revision) {
        return Err(Error::Invalid(
            "Resolve a committed revision before preparing deployment",
        ));
    }
    super::providers::preflight(cloud_id, profile, settings)?;
    super::settings::validate_ssh_identity(&settings.ssh_identity_file)
}

fn bind_registry(store: &Store, state: &mut Deployment, ensure: impl FnOnce() -> Result<String>) -> Result<()> {
    let spec = state
        .spec
        .as_mut()
        .ok_or(Error::Invalid("Deployment has no worker specification"))?;
    spec.registry_auth_id = Some(ensure()?);
    // Credentials, storage and worker creation are one composite operation. A
    // registry retry cannot settle an earlier uncertain volume allocation.
    store.save(state)
}

fn provision(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
    observe: mutation::Observer<'_>,
) -> Result<()> {
    if state.operation == CreateState::Requested {
        observe(Mutation::Pending)?;
    }
    storage::observe_pending(store, spec, observe)?;
    spec.validate()?;
    if state.operation != CreateState::Prepared {
        // Existing workers cannot allocate replacement storage while reconnecting.
        storage::expected(store, spec)?;
    }
    let mut operation = state.operation.clone();
    let volume = storage::prepare(provider, store, state, spec, cancel, observe)?;
    let mut failure = None;
    let worker = provider.ensure_with_volume(
        spec,
        &mut operation,
        volume.as_ref(),
        cancel,
        |next| {
            state.operation = next.clone();
            store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)?;
            if *next == CreateState::Requested {
                observe(Mutation::Pending).map_err(|error| {
                    failure = Some(error);
                    horizon_cloud::CloudError::Persistence
                })?;
            }
            Ok(())
        },
        |progress| emit(Event::Output(format!("{progress:?}"))),
    );
    if let Some(error) = failure {
        return Err(error);
    }
    state.worker = Some(worker?);
    store.save(state)?;
    observe(Mutation::Settled)?;
    Ok(())
}

/// Journals one relaunch for a worker that was ready before and still has sessions.
fn arm_recorded_session_resume(state: &mut Deployment) -> bool {
    if !super::timeline::reconnects(state) || state.session_restart.is_some() || state.sessions.is_empty() {
        return false;
    }
    state.session_restart = Some(OperationId::generate());
    true
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
    super::tailnet::validate_pending(store.root())?;
    if !repository::is_commit_id(&request.revision) {
        return Err(Error::Invalid(
            "Resolve a committed revision before preparing deployment",
        ));
    }
    let state = if let Some(mut state) = store.load()? {
        // CPU and memory may change until a worker is requested; the image does not depend on them.
        let mut resized = state.profile.clone();
        let requested = super::publishing::requested_profile(&state, &request.profile);
        resized.cpu = request.profile.cpu;
        resized.memory_gb = request.profile.memory_gb;
        if state.cloud_id != request.cloud_id
            || state.publication.as_ref().is_some_and(|record|
                request.profile_name.as_ref().is_some_and(|name| name != &record.profile))
            || state.revision != request.revision
            || state.repository != request.repository
            || resized != requested
            || (state.profile != resized && !state.accepts_next_size())
            // Until the first image is built; a rebuild records its own files later.
            || (state.spec.is_none()
                && request
                    .working_copy
                    .as_ref()
                    .is_some_and(|confirmed| state.working_copy.as_ref() != Some(confirmed)))
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
        let mut created = Deployment {
            version: 1,
            service_admission: None,
            exposure: None,
            cloud_id: request.cloud_id.clone(),
            repository: request.repository.clone(),
            revision: request.revision.clone(),
            profile: request.profile.clone(),
            stage: Stage::Validate,
            operation: CreateState::Prepared,
            spec: None,
            registry_generation: None,
            publication: None,
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
            tailnet_device: None,
            tailnet_enrollment: super::tailnet::receipt::EnrollmentLedger::default(),
            siblings: None,
            uncommitted_image: None,
            working_copy: request.working_copy.clone(),
            created_by: request.created_by.clone(),
        };
        super::publishing::bind_initial(request, &mut created)?;
        if created.publication.is_some() {
            store.save(&created)?;
        }
        created
    };
    if state.publication.is_none() && state.profile.automatic_image() && !request.settings.portable_image_publishing {
        return Err(Error::Invalid(super::publishing::STAGED));
    }
    if !state.profile.automatic_image() {
        super::providers::admit(
            &request.settings,
            &state.profile,
            &state.profile.image,
            &state.operation,
        )?;
    }
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
