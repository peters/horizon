//! Rebuilds a dedicated cloud's image from its latest committed recipe and switches the
//! bound worker to it. A cloud on the public base image without a recipe, such as a
//! quick-start cloud, switches to the base image this Horizon version pins instead.
//! Each provider mutation is journaled first (`ImageReplacement`), and
//! the steps outside the record sit behind `Steps` so every persistence boundary is
//! tested offline. Only dedicated clouds have a `deployment.json` record; a migrated
//! allocation does not load here.
mod live;
mod new_server;
#[cfg(all(test, unix))]
mod tests;

use super::{
    super::{
        WorkerContract,
        progress::Progress,
        state::{Deployment, ImageReplacement, OperationId, ReplacementImage, ReplacementPhase, Session},
    },
    Error, Event, Request, Result, Stage, Store,
};
use crate::cloud_runtime::{command::Runner, repository::launch::quick_start};
use horizon_cloud::{
    Cancellation, CloudConfig, CloudError, CreateState, Profile, WorkerSpec,
    provider::{Description, Rebuild},
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
const NO_RECIPE: &str = "This cloud's profile has no build section and does not run the public base image, so there is no recipe to rebuild its image from";
const NO_CONFIG: &str =
    "The latest commit has no readable .horizon/cloud.yml; commit the cloud configuration to rebuild";
const NO_PROFILE: &str = "The latest commit's .horizon/cloud.yml no longer defines this cloud's profile";
const COMMITTED_BASE: &str = "This cloud runs the public base image without a build section, and the latest commit has its own .horizon/cloud.yml. Only a cloud of a repository without one moves to the pinned base image; create a new cloud to use the committed settings.";
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
    /// A new-server rebuild released the worker's server; the volume stays.
    Released,
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
/// Reads the repository's committed recipe; needs no credentials.
trait Recipe {
    fn head(&self, repository: &Path) -> Result<Head>;
    /// The latest committed revision of each of the deployment's siblings, whose
    /// declarations `config`, the primary's latest committed configuration, holds.
    fn siblings(&self, state: &Deployment, config: &CloudConfig) -> Result<Vec<String>>;
}

/// The latest committed recipes a rebuild layers: the primary's and each sibling's.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Recipes {
    revision: String,
    siblings: Vec<String>,
}

trait Steps: Provider {
    /// Builds `revision`'s recipe, with each sibling's recipe at `siblings` layered on it,
    /// under `tag`, validates the worker contract, pushes the image and verifies that the
    /// worker's pull binding can read it.
    fn build(&self, state: &Deployment, revision: &str, siblings: &[String], tag: &str) -> Result<ReplacementImage>;
    /// Verifies again that a built image and its pull binding are unchanged.
    fn verify(&self, state: &Deployment, image: &ReplacementImage) -> Result<()>;
    /// Releases hosted devices before the container reset ends the worker's copies.
    fn release_devices(&self, state: &Deployment) -> Result<()>;
    /// For a provider that rebuilds on a new server (`provider::Rebuild::NewServer`),
    /// how its server is released and reopened; `None` when the provider switches the
    /// worker in place.
    fn server(&self) -> Option<&dyn Server> {
        None
    }
}

/// A provider that rebuilds on a new server (`provider::Rebuild::NewServer`): it
/// cannot report a server's image, so the server is released and the next reconnect
/// creates a new one on the rebuilt image and the same workspace volume.
trait Server {
    /// Whether the release of the bound server has begun; before it, the server is untouched.
    fn released(&self, store: &Store, state: &Deployment) -> Result<bool>;
    /// Releases the bound server and keeps the workspace volume and the `Replace` stage.
    fn release(&self, store: &Store, state: &mut Deployment) -> Result<()>;
    /// Saves `state` with the released server's fence cleared.
    fn reopen(&self, store: &Store, state: &mut Deployment) -> Result<()>;
    /// Runs after each durable write.
    fn checkpoint(&self, _boundary: Boundary) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Driven {
    Committed,
    Unchanged,
}

/// Rebuilds the image from the repository's latest committed `.horizon` recipe with the
/// newest agent CLIs, moves the worker onto it and relaunches its sessions. A provider
/// that rebuilds in place keeps the worker ID; one that rebuilds on a new server
/// (`provider::Rebuild::NewServer`) releases its server and starts a new one with a new
/// ID on the same workspace volume. `/workspace` is kept either way; the container is reset.
/// A cloud on the public base image without a recipe ([`rebuildable`]), whose latest commit
/// has no `.horizon/cloud.yml`, moves to the base image this Horizon version pins and
/// builds nothing.
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
    // Checked before credentials load, so a changed profile is refused with its reason.
    let checkout = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let recipes = committed_recipes(&checkout, &state, profile_name, emit)?;
    let steps = live::Live::new(request, &store, &state, true, cancel, emit)?;
    begin(&steps, &store, &mut state, recipes)?;
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
    if journal.requested() {
        // Finishing a switch needs only the provider, not the checkout or registry.
        match Description::of(&state.profile).rebuild {
            Rebuild::InPlace => {
                let provider = RunPod::new(request.settings.credential()?);
                resume(&live::Pod::new(&provider, &store, cancel), &store, &mut state, emit)?;
            }
            Rebuild::NewServer => {
                let server = live::Released::new(&request.settings, cancel);
                new_server::switch(&server, &store, &mut state, emit)?;
            }
        }
        drop(store);
        return finish(request, Driven::Committed, state, cancel, emit);
    }
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
        let rearm = may_have_released_devices(&state);
        discard(&store, &mut state, emit)?;
        if !rearm {
            return Ok(state);
        }
        // Reconnecting arms hosted devices again, as every reconnect does.
        drop(store);
        return tail(request, cancel, emit);
    }
    match Description::of(&state.profile).rebuild {
        Rebuild::InPlace => {
            let provider = RunPod::new(request.settings.credential()?);
            revert(&live::Pod::new(&provider, &store, cancel), &store, &mut state, emit)?;
        }
        Rebuild::NewServer => {
            let server = live::Released::new(&request.settings, cancel);
            if new_server::cancel(&server, &store, &mut state, emit)? == new_server::Cancelled::Untouched {
                return Ok(state);
            }
        }
    }
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
    {
        same_worker(state)?;
        if steps.observe(state, None)? == Observed::Next {
            return commit(steps, store, state);
        }
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
    contract: &WorkerContract,
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
    same_worker(&state)?;
    Ok((store, state))
}

/// The recorded worker specification belongs to this deployment.
fn same_worker(state: &Deployment) -> Result<()> {
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    Ok(())
}

/// Whether a cloud of `profile` can rebuild its image: from its committed recipe, or, on the
/// public base image without one, by moving to the base image this Horizon version pins.
#[must_use]
pub fn rebuildable(profile: &Profile) -> bool {
    profile.build.is_some() || quick_start::on_public_base(profile)
}

fn ready(state: &Deployment) -> Result<()> {
    state.refuse_pending_replacement()?;
    if !rebuildable(&state.profile) {
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

/// The committed recipes to rebuild, once the profile named `profile_name` equals the
/// bound one: the primary's latest commit and each sibling's. A cloud on the public base
/// image has no recipe; while the latest commit has no `.horizon/cloud.yml`, as for quick
/// start, its image is the pin of this Horizon version. Committed settings are never
/// passed over: with them such a cloud is refused.
fn committed_recipes(
    recipe: &impl Recipe,
    state: &Deployment,
    profile_name: &str,
    emit: &dyn Fn(Event),
) -> Result<Recipes> {
    emit(activity("Reading the latest committed recipe"));
    let head = recipe.head(&state.repository)?;
    if quick_start::on_public_base(&state.profile) {
        if head.config.is_some() {
            return Err(Error::Invalid(COMMITTED_BASE));
        }
        emit(activity("Using the base image that this Horizon version pins"));
        return Ok(Recipes {
            revision: head.revision,
            siblings: Vec::new(),
        });
    }
    unchanged_profile(&state.profile, head.config.as_ref(), profile_name)?;
    let (Some(set), Some(config)) = (&state.siblings, &head.config) else {
        return Ok(Recipes {
            revision: head.revision,
            siblings: Vec::new(),
        });
    };
    emit(activity("Reading each same-worker sibling's latest committed recipe"));
    let siblings = recipe.siblings(state, config)?;
    for note in set.moved(&siblings) {
        emit(Event::Output(note));
    }
    Ok(Recipes {
        revision: head.revision,
        siblings,
    })
}

fn begin(steps: &impl Steps, store: &Store, state: &mut Deployment, recipes: Recipes) -> Result<()> {
    state.begin_layered_replacement(OperationId::generate(), recipes.revision, recipes.siblings)?;
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
    // The whole journal is checked before any build, push or provider update.
    state.replacement_worker()?;
    let journal = state.image_replacement.clone().ok_or(Error::Invalid(NOTHING_PENDING))?;
    match journal.phase {
        ReplacementPhase::Prepared {} => {
            let image = steps.build(
                state,
                &journal.recipe_revision,
                &journal.sibling_revisions,
                &journal.tag,
            )?;
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
            match steps.server() {
                Some(server) => new_server::switch(server, store, state, emit)?,
                None => resume(steps, store, state, emit)?,
            }
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
    if let Some(server) = steps.server() {
        return new_server::switch(server, store, state, emit);
    }
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
    // The forward update may still apply after a read of the previous image, so the
    // reverse one is always sent and must be observed before the journal is dropped.
    send(steps, &worker_id, &next, &current, emit)?;
    await_image(steps, state, Observed::Previous, emit)?;
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
            Ok(_) | Err(Error::Provider(CloudError::Transport)) => {}
            // Server errors and rate limits are transient; other statuses are not.
            Err(Error::Provider(CloudError::Http(status, _))) if status >= 500 || status == 429 => {}
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

/// Hosted devices are released once a replacement is built, and a crash can lose the record
/// of that release, so dropping a built replacement reconnects to arm them again.
fn may_have_released_devices(state: &Deployment) -> bool {
    state.profile.capabilities.browserstack.is_some()
        && state
            .image_replacement
            .as_ref()
            .is_some_and(|journal| journal.image().is_some())
}

fn activity(detail: &'static str) -> Event {
    Event::Progress(Progress::activity(detail))
}
