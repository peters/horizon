//! Rebuilds a dedicated cloud's image from its latest committed recipe and switches the
//! bound worker to it. Each provider mutation is journaled first (`ImageReplacement`), and
//! the steps outside the record sit behind `Steps` so every persistence boundary is
//! tested offline. Only dedicated clouds have a `deployment.json` record; a migrated
//! allocation does not load here.
mod live;
#[cfg(test)]
mod tests;

use super::{
    super::{
        WorkerContract,
        progress::Progress,
        state::{Deployment, ImageReplacement, OperationId, ReplacementImage, ReplacementPhase, Session},
    },
    Error, Event, Request, Result, Stage, Store,
};
use horizon_cloud::{
    Cancellation, CloudConfig, CloudError, CreateState, Profile, WorkerSpec,
    runpod::{
        RunPod,
        replacement::{Observed, may_have_applied},
    },
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const NOTHING_PENDING: &str = "No image replacement is pending";
const NOT_READY: &str = "Only a ready, running cloud without a pending replacement can rebuild its image";
const NO_RECIPE: &str = "This cloud's profile has no build section, so there is no recipe to rebuild its image from";
const NO_CONFIG: &str =
    "The latest commit has no readable .horizon/cloud.yml; commit the cloud configuration to rebuild";
const NO_PROFILE: &str = "The latest commit's .horizon/cloud.yml no longer defines this cloud's profile";
const UNSETTLED: &str =
    "The provider has not reported the switched image yet; the replacement stays pending. Continue or cancel it.";
const RELAUNCH_STATUS: &str = "horizon-relaunch-status=";
const ADDS_CREDENTIAL: &str = "The rebuilt image needs a registry credential that this worker was created without, and a switch that adds one cannot be undone. Create a new cloud to use it.";

/// Durable writes of a replacement, where tests inject crashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    Begun,
    Built,
    Requested,
    Patched,
    Observed,
    Rebound,
    Reverted,
}

/// The repository's committed HEAD and its `.horizon/cloud.yml`, if readable.
#[derive(Clone)]
struct Head {
    revision: String,
    config: Option<CloudConfig>,
}

/// Provider calls of a replacement.
trait Provider {
    /// Sends the pod update that switches `worker_id` from `from`'s image to `to`'s.
    fn replace(&self, worker_id: &str, from: &WorkerSpec, to: &WorkerSpec) -> Result<()>;
    /// Which image of the journaled pair every provider API reports, read before
    /// `deadline` when one is given.
    fn observe(&self, state: &Deployment, deadline: Option<Instant>) -> Result<Observed>;
    /// Waits before observing again; `false` once `deadline` has passed.
    fn pause(&self, deadline: Instant) -> Result<bool>;
    /// Runs after each durable write.
    fn checkpoint(&self, _boundary: Boundary) -> Result<()> {
        Ok(())
    }
}

/// The rest of a rebuild outside its record.
trait Steps: Provider {
    fn head(&self, repository: &Path) -> Result<Head>;
    /// Builds `revision`'s recipe under `tag`, validates the worker contract, pushes the
    /// image and verifies that the worker's pull binding can read it.
    fn build(&self, state: &Deployment, revision: &str, tag: &str) -> Result<ReplacementImage>;
    /// Verifies again that a built image and its pull binding are unchanged.
    fn verify(&self, state: &Deployment, image: &ReplacementImage) -> Result<()>;
    /// Releases hosted devices before the container reset ends the worker's copies.
    fn release_devices(&self, state: &Deployment) -> Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Driven {
    Committed,
    Unchanged,
}

/// Rebuilds the image from the repository's latest committed `.horizon` recipe with the
/// newest agent CLIs, switches the bound worker to it and relaunches its sessions. The
/// worker ID and `/workspace` are kept; the container is reset.
/// # Errors
/// Refuses unless the cloud is ready with nothing pending and the committed profile named
/// `profile_name` equals the bound one. After a failure the journal stays, so the
/// replacement can be continued or cancelled.
pub fn rebuild(
    request: &Request,
    profile_name: &str,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    emit(Event::stage(Stage::Validate));
    let (store, mut state) = open(request)?;
    ready(&state)?;
    let steps = live::Live::new(request, &store, &state, true, cancel, emit)?;
    begin(&steps, &store, &mut state, profile_name, emit)?;
    let driven = drive(&steps, &store, &mut state, emit)?;
    drop(steps);
    drop(store);
    finish(request, driven, state, cancel, emit)
}

/// Resumes a pending replacement from its next step: a prepared one builds, a built one
/// verifies its image again, and a requested one is sent again only while the provider
/// still reports the previous image.
/// # Errors
/// Refuses without a pending replacement; keeps the journal after a failure.
pub fn continue_replacement(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    emit(Event::stage(Stage::Validate));
    let (store, mut state) = open(request)?;
    let journal = state
        .image_replacement
        .as_ref()
        .ok_or(Error::Invalid(NOTHING_PENDING))?;
    let building = journal.phase == ReplacementPhase::Prepared {};
    let steps = live::Live::new(request, &store, &state, building, cancel, emit)?;
    let driven = drive(&steps, &store, &mut state, emit)?;
    drop(steps);
    drop(store);
    finish(request, driven, state, cancel, emit)
}

/// Cancels a pending replacement. An unsent one is dropped without provider I/O;
/// otherwise the worker is switched back to its previous image and reconnected.
/// # Errors
/// Keeps the journal until the provider reports the previous image.
pub fn cancel_replacement(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    let (store, mut state) = open(request)?;
    if !state
        .image_replacement
        .as_ref()
        .is_some_and(ImageReplacement::requested)
    {
        discard(&store, &mut state, emit)?;
        if !devices_released(&state) {
            return Ok(state);
        }
        // The switch released hosted devices; reconnecting arms them again.
        drop(store);
        return tail(request, cancel, emit);
    }
    let provider = RunPod::new(request.settings.credential()?);
    revert(&live::Pod::new(&provider, &store, cancel), &store, &mut state, emit)?;
    drop(store);
    tail(request, cancel, emit)
}

/// Settles a replacement whose provider update may have been sent, reading the provider
/// only: a worker on the new image commits it; anything else keeps the journal.
/// # Errors
/// Refuses while the worker reports the previous or an unsettled image.
pub(in crate::cloud_runtime) fn settle(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    cancel: &Cancellation,
) -> Result<()> {
    settle_with(&live::Pod::new(provider, store, cancel), store, state)
}

fn settle_with(steps: &impl Provider, store: &Store, state: &mut Deployment) -> Result<()> {
    if matches!(state.operation, CreateState::Bound { .. })
        && state
            .image_replacement
            .as_ref()
            .is_some_and(ImageReplacement::requested)
        && steps.observe(state, None)? == Observed::Next
    {
        return commit(steps, store, state);
    }
    state.refuse_unsettled_replacement()
}

/// After a replacement reset the worker's container, relaunches each recorded session's
/// process in its existing worktree and clears the request. A relaunch is idempotent per
/// replacement, so a reconnect retries safely. Images without the relaunch contract
/// report their sessions lost, as Stop and Resume do.
/// # Errors
/// Keeps the request when a relaunch cannot be confirmed.
pub(super) fn relaunch_sessions(
    store: &Store,
    state: &mut Deployment,
    contract: WorkerContract,
    emit: &dyn Fn(Event),
    mut run: impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    let Some(operation) = state.session_restart else {
        return Ok(());
    };
    if contract.session_restart && !state.sessions.is_empty() {
        emit(activity("Relaunching sessions in their existing worktrees"));
    }
    for session in &state.sessions {
        let relaunched = contract.session_restart
            && match relaunch_status(&run(&relaunch_command(operation, session, &state.revision)?)?) {
                Some(0 | 5) => true,
                Some(3 | 4) => false,
                _ => return Err(Error::Invalid("A session could not be relaunched; reconnect to retry")),
            };
        if !relaunched {
            emit(Event::Output(format!(
                "Session {} ({}) lost its process in the container reset and was not relaunched",
                session.panel_id, session.agent
            )));
        }
    }
    state.session_restart = None;
    store.save(state)
}

fn relaunch_command(operation: OperationId, session: &Session, revision: &str) -> Result<String> {
    if !horizon_cloud::valid_id(&session.panel_id)
        || !matches!(session.agent.as_str(), "codex" | "claude" | "grok" | "shell")
        || !super::repository::is_commit_id(revision)
    {
        return Err(Error::Invalid("Invalid remote session identity"));
    }
    Ok(format!(
        "horizon-worker-session --relaunch {operation} {} {} {revision}; printf '\\n{RELAUNCH_STATUS}%s\\n' \"$?\"",
        session.panel_id, session.agent
    ))
}

fn relaunch_status(output: &str) -> Option<i32> {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(RELAUNCH_STATUS))
        .and_then(|status| status.parse().ok())
}

fn open(request: &Request) -> Result<(Store, Deployment)> {
    let store = Store::lock(&request.state_root)?;
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    if state.cloud_id != request.cloud_id
        || state.repository != request.repository
        || state.revision != request.revision
        || state.profile != request.profile
    {
        return Err(Error::Invalid(
            "Cloud is permanently bound to its repository, revision and profile",
        ));
    }
    Ok((store, state))
}

fn ready(state: &Deployment) -> Result<()> {
    state.refuse_pending_replacement()?;
    if state.profile.build.is_none() {
        return Err(Error::Invalid(NO_RECIPE));
    }
    if !matches!(state.operation, CreateState::Bound { .. })
        || state.stop_requested
        || state.stage != Stage::Ready
        || state.session_restart.is_some()
    {
        return Err(Error::Invalid(NOT_READY));
    }
    Ok(())
}

fn begin(
    steps: &impl Steps,
    store: &Store,
    state: &mut Deployment,
    profile_name: &str,
    emit: &dyn Fn(Event),
) -> Result<()> {
    emit(activity("Reading the latest committed recipe"));
    let head = steps.head(&state.repository)?;
    unchanged_profile(&state.profile, head.config.as_ref(), profile_name)?;
    let operation = OperationId::generate();
    let tag = format!("horizon-{}-{}", state.cloud_id, uuid::Uuid::from(operation).simple());
    if !super::super::image::valid_tag(&tag) {
        return Err(Error::Invalid("This cloud's identity is too long for an image tag"));
    }
    state.begin_replacement(operation, head.revision, tag)?;
    store.save(state)?;
    steps.checkpoint(Boundary::Begun)
}

/// A running worker's size, capabilities and image repository are fixed, so the
/// committed profile must equal the bound one exactly.
fn unchanged_profile(bound: &Profile, config: Option<&CloudConfig>, name: &str) -> Result<()> {
    let mut head = config
        .ok_or(Error::Invalid(NO_CONFIG))?
        .profiles
        .get(name)
        .ok_or(Error::Invalid(NO_PROFILE))?
        .clone();
    // A CPU cloud's vCPU and memory are chosen when it is created; the committed ones are defaults.
    if !bound.gpu && !head.gpu {
        (head.cpu, head.memory_gb) = (bound.cpu, bound.memory_gb);
    }
    if head == *bound {
        return Ok(());
    }
    Err(Error::Invalid(
        if (head.cpu, head.memory_gb, head.gpu, &head.storage)
            != (bound.cpu, bound.memory_gb, bound.gpu, &bound.storage)
        {
            "The latest commit changes this cloud's size (CPU, memory, GPU or storage), and a running worker cannot be resized. Create a new cloud to use it."
        } else if head.capabilities != bound.capabilities {
            "The latest commit changes this cloud's capabilities, which are fixed when its worker is created. Create a new cloud to use them."
        } else if head.image != bound.image {
            "The latest commit changes this cloud's image repository. Create a new cloud to use it."
        } else if head.build != bound.build {
            "The latest commit changes this cloud's build section. Create a new cloud to use it."
        } else {
            "The latest commit changes this cloud's provider or bootstrap settings. Create a new cloud to use them."
        },
    ))
}

/// Advances the journal from whichever phase it records to a committed replacement.
fn drive(steps: &impl Steps, store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<Driven> {
    let journal = state.image_replacement.clone().ok_or(Error::Invalid(NOTHING_PENDING))?;
    match journal.phase {
        ReplacementPhase::Prepared {} => {
            let image = steps.build(state, &journal.recipe_revision, &journal.tag)?;
            let adds_credential = journal.previous_registry_auth_id.is_none() && image.registry_auth_id.is_some();
            if image.digest == journal.previous_digest || adds_credential {
                state.discard_replacement()?;
                store.save(state)?;
                if adds_credential {
                    return Err(Error::Invalid(ADDS_CREDENTIAL));
                }
                return Ok(Driven::Unchanged);
            }
            state.replacement_built(image)?;
            store.save(state)?;
            steps.checkpoint(Boundary::Built)?;
        }
        ReplacementPhase::Built(image) => {
            emit(activity("Verifying the built image and its pull binding"));
            steps.verify(state, &image)?;
        }
        ReplacementPhase::Requested(_) => {
            resume(steps, store, state, emit)?;
            return Ok(Driven::Committed);
        }
    }
    request(steps, store, state, emit)?;
    Ok(Driven::Committed)
}

/// Journals the update as possibly sent, sends it and commits once the provider reports it.
fn request(steps: &impl Steps, store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    if state.requires_browserstack_release() {
        emit(activity("Releasing hosted devices before the worker restarts"));
        steps.release_devices(state)?;
        state.browserstack_released = true;
        store.save(state)?;
    }
    let previous = state.request_replacement()?;
    store.save(state)?;
    steps.checkpoint(Boundary::Requested)?;
    emit(Event::stage(Stage::Replace));
    let (worker_id, current, next) = pair(state)?;
    if let Err(error) = send(steps, &worker_id, &current, &next, emit) {
        // A definite refusal left the worker on its image; the update can be sent again.
        state.refuse_replacement(previous)?;
        store.save(state)?;
        return Err(error);
    }
    steps.checkpoint(Boundary::Patched)?;
    await_image(steps, state, Observed::Next, emit)?;
    commit(steps, store, state)
}

/// Sends the update again only while the provider still reports the previous image, so a
/// worker that already switched is not reset twice.
fn resume(steps: &impl Provider, store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    emit(Event::stage(Stage::Replace));
    if steps.observe(state, None)? == Observed::Previous {
        let (worker_id, current, next) = pair(state)?;
        send(steps, &worker_id, &current, &next, emit)?;
    }
    await_image(steps, state, Observed::Next, emit)?;
    commit(steps, store, state)
}

fn commit(steps: &impl Provider, store: &Store, state: &mut Deployment) -> Result<()> {
    steps.checkpoint(Boundary::Observed)?;
    super::commit_with(store, state, || steps.checkpoint(Boundary::Rebound)).map(|_| ())
}

fn discard(store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    if state.image_replacement.is_none() {
        return Err(Error::Invalid(NOTHING_PENDING));
    }
    state.discard_replacement()?;
    store.save(state)?;
    emit(Event::Output(
        "Image replacement cancelled; the worker keeps its image".into(),
    ));
    Ok(())
}

/// Switches a worker whose update may have been sent back to its previous image.
fn revert(steps: &impl Provider, store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    emit(Event::stage(Stage::Replace));
    let (worker_id, current, next) = pair(state)?;
    let operation = state
        .image_replacement
        .as_ref()
        .ok_or(Error::Invalid(NOTHING_PENDING))?
        .operation;
    if steps.observe(state, None)? != Observed::Previous {
        send(steps, &worker_id, &next, &current, emit)?;
        await_image(steps, state, Observed::Previous, emit)?;
    }
    steps.checkpoint(Boundary::Reverted)?;
    // An interrupted commit may have rebound the storage journal to the new image.
    super::storage::rebind(store, &next, &current)?;
    state.image_replacement = None;
    state.stage = Stage::Readiness;
    // Either update may have reset the container; relaunching a running session is a no-op.
    state.session_restart = Some(operation);
    store.save(state)
}

/// Sends a pod update. An outcome that may have applied is observed afterwards rather
/// than returned, so only a definite refusal is an error.
fn send(
    steps: &impl Provider,
    worker_id: &str,
    from: &WorkerSpec,
    to: &WorkerSpec,
    emit: &dyn Fn(Event),
) -> Result<()> {
    emit(activity("Switching the worker's image"));
    match steps.replace(worker_id, from, to) {
        Err(Error::Provider(error)) if may_have_applied(&error) => {
            emit(Event::Output(format!(
                "{error}; checking which image the provider records"
            )));
            Ok(())
        }
        result => result,
    }
}

/// Polls until every provider API reports `target`, within the profile's readiness
/// budget. A third image or a lost worker fails closed and keeps the journal.
fn await_image(steps: &impl Provider, state: &Deployment, target: Observed, emit: &dyn Fn(Event)) -> Result<()> {
    emit(activity("Waiting for the provider to report the switched image"));
    let deadline = Instant::now() + Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds));
    loop {
        match steps.observe(state, Some(deadline)) {
            Ok(observed) if observed == target => return Ok(()),
            Ok(_) | Err(Error::Provider(CloudError::Transport | CloudError::Http(..))) => {}
            Err(error) => return Err(error),
        }
        if !steps.pause(deadline)? {
            return Err(Error::Invalid(UNSETTLED));
        }
    }
}

/// The bound worker's ID with its recorded and replacement specifications.
fn pair(state: &Deployment) -> Result<(String, WorkerSpec, WorkerSpec)> {
    let next = state.replacement_worker()?.ok_or(Error::Invalid(NOTHING_PENDING))?;
    let (CreateState::Bound { worker_id }, Some(current)) = (&state.operation, &state.spec) else {
        return Err(Error::Invalid("Only a bound worker's image can be replaced"));
    };
    Ok((worker_id.clone(), current.clone(), next))
}

fn finish(
    request: &Request,
    driven: Driven,
    state: Deployment,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    if driven == Driven::Unchanged {
        emit(Event::Output("Image unchanged; nothing to restart".into()));
        if state.stage == Stage::Ready {
            emit(Event::ready(Box::new(state.clone())));
            return Ok(state);
        }
    }
    tail(request, cancel, emit)
}

/// Reconnects through the ordinary deployment path, which verifies the recorded image
/// strictly, waits for readiness and relaunches the sessions.
fn tail(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    super::deploy(request, cancel, &|event| {
        if !matches!(event, Event::Stage(Stage::Validate, _)) {
            emit(event);
        }
    })
}

/// Hosted devices released for a switch that never happened stay released until a reconnect.
fn devices_released(state: &Deployment) -> bool {
    state.profile.capabilities.browserstack.is_some() && state.browserstack_released
}

fn activity(detail: &'static str) -> Event {
    Event::Progress(Progress::activity(detail))
}
