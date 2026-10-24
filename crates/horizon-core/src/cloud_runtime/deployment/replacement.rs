//! Rebuilds a dedicated cloud's image from its recipe in the checkout, the latest commit
//! with its uncommitted changes, and switches the bound worker to it, binding the rebuild
//! settings of the checkout's profile with it (`Profile::with_rebuild_settings`). A cloud
//! on the public base image without a recipe, such as a quick-start cloud, switches to
//! the base image this Horizon version pins instead.
//! Each provider mutation is journaled first (`ImageReplacement`), and
//! the steps outside the record sit behind `Steps` so every persistence boundary is
//! tested offline. Only dedicated clouds have a `deployment.json` record; a migrated
//! allocation does not load here.
mod compatibility;
mod live;
mod new_server;
mod recipes;
mod sessions;
#[cfg(all(test, unix))]
mod tests;

use super::{
    super::{
        WorkerContract,
        progress::Progress,
        state::{Deployment, ImageReplacement, OperationId, ReplacementImage, ReplacementPhase, Session},
    },
    Error, Event, Request, Result, Settings, Stage, Store,
};
use crate::cloud_runtime::{command::Runner, repository::launch::quick_start};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, Profile, WorkerSpec,
    provider::{Description, Rebuild},
    runpod::{
        RunPod,
        replacement::{Observed, may_have_applied},
    },
};
use std::time::{Duration, Instant};

use recipes::{Head, Recipe, Recipes, latest_recipes, report_changes};
pub use recipes::{PROFILE_CHANGE, Plan, plan, plan_for_cloud};
pub(super) use sessions::relaunch_sessions;

const NOTHING_PENDING: &str = "No image replacement is pending";
const NOT_READY: &str = "Only a ready, running cloud without a pending replacement can rebuild its image";
const NO_RECIPE: &str = "This cloud's profile has no build section and does not run the public base image, so there is no recipe to rebuild its image from";
const UNSETTLED: &str =
    "The provider has not reported the switched image yet; the replacement stays pending. Continue or cancel it.";
const ADDS_CREDENTIAL: &str = "The rebuilt image needs a registry credential that this worker was created without, and a switch that adds one cannot be undone. Create a new cloud to use it.";
const CONFIRMED_OTHER: &str = "The checkout changes this cloud's settings differently from what was confirmed; review the changes and rebuild again";
const UNCONFIRMED_PUBLISHER: &str =
    "The cloud's publishing identity changed or was not confirmed; review its rebuild plan again";
const UNCONFIRMED_FILES: &str = "The uncommitted files in the image's build context are not the ones that were confirmed; review them and rebuild again";
const SAME_IMAGE: &str = "The rebuilt image is the worker's current image, so the changed settings cannot reach the worker. Nothing changed on the worker and nothing is pending; rebuild again";

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

trait Steps: Provider {
    /// Builds `revision`'s recipe, with each sibling's recipe at `siblings` layered on it,
    /// under `tag`, validates the worker contract, pushes the image and verifies that the
    /// worker's pull binding can read it.
    fn build(&self, state: &Deployment, revision: &str, siblings: &[String], tag: &str) -> Result<ReplacementImage>;
    /// Verifies again that a built image and its pull binding are unchanged.
    fn verify(&self, state: &Deployment, image: &ReplacementImage) -> Result<()>;
    /// The unpacked size in bytes of the image that `build` made under `tag` and pushed as
    /// `digest`, as local Docker reports it; `None` when nothing was built, as for the
    /// pinned base image. Refuses when `tag` no longer names that image.
    fn unpacked_bytes(&self, state: &Deployment, tag: &str, digest: &str) -> Result<Option<u64>>;
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

/// Rebuilds the image from the repository's `.horizon` recipe, its latest commit with the
/// uncommitted files of the build context, with the newest agent CLIs, moves the worker
/// onto it and relaunches its sessions. The cloud binds the rebuild settings of the
/// working copy's profile, which [`plan`] lists, once the worker runs the new image;
/// sessions of an agent the profile no longer has are reported and not relaunched. With
/// `confirmed`, the plan a person confirmed, a checkout that binds another profile or
/// builds from other uncommitted files is refused; without it, any uncommitted file in the
/// build context is refused, as no person saw it. A provider
/// that rebuilds in place keeps the worker ID; one that rebuilds on a new server
/// (`provider::Rebuild::NewServer`) releases its server and starts a new one with a new
/// ID on the same workspace volume. `/workspace` is kept either way; the container is reset.
/// A cloud on the public base image without a recipe ([`rebuildable`]), whose latest commit
/// has no `.horizon/cloud.yml`, moves to the base image this Horizon version pins and
/// builds nothing.
/// # Errors
/// Refuses unless the cloud is ready with nothing pending and the committed profile named
/// `profile_name` matches the bound one apart from its rebuild settings and the choices
/// bound at creation, such as the provider and the storage, and any agent of that profile
/// whose configured credential is unusable. Refuses a built image that the worker's container
/// disk cannot hold before anything changes on the worker, and drops its unsent journal,
/// so nothing stays pending. After any other failure once the replacement is journaled,
/// the journal stays, so the replacement can be continued or cancelled.
pub fn rebuild(
    request: &Request,
    profile_name: &str,
    confirmed: Option<&Plan>,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    emit(Event::stage(Stage::Validate));
    let (store, mut state) = open(request)?;
    ready(&state)?;
    confirm_publication(&state, confirmed)?;
    // Checked before credentials load, so a changed profile is refused with its reason.
    let checkout = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let recipes = latest_recipes(&checkout, &state, profile_name, emit)?;
    accept(&recipes.profile, confirmed.map(|plan| &plan.profile), &request.settings)?;
    confirm_files(&recipes.uncommitted, confirmed)?;
    report_changes(&state.profile, &recipes.profile, emit);
    let steps = live::Live::new(request, &store, &state, true, cancel, emit)?;
    begin(&steps, &store, &mut state, recipes)?;
    let driven = drive(&steps, &store, &mut state, emit)?;
    drop(steps);
    drop(store);
    finish(request, driven, state, cancel, emit)
}

/// Resumes a pending replacement from its next step: a prepared one builds, a built one
/// verifies its image again and that the worker's container disk holds it, and a
/// requested one is sent again only while the provider still reports the previous image.
/// # Errors
/// Refuses without a pending replacement, an unsent one whose agents have an unusable
/// configured credential, and a built image that the worker's container
/// disk cannot hold; keeps the journal after a failure, so it can be cancelled.
pub fn continue_replacement(request: &Request, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    emit(Event::stage(Stage::Validate));
    let (store, mut state) = open(request)?;
    let journal = state
        .image_replacement
        .as_ref()
        .ok_or(Error::Invalid(NOTHING_PENDING))?;
    report_changes(&state.profile, state.replacement_profile(), emit);
    if journal.requested() {
        // Finishing a switch needs only the provider, not the checkout or registry.
        match Description::of(&state.profile).rebuild {
            Rebuild::InPlace => {
                let provider = RunPod::new(request.settings.credential()?);
                resume(&live::Pod::new(&provider, &store, cancel), &store, &mut state, emit)?;
            }
            Rebuild::NewServer => {
                let server = live::Released::of(&state.profile, &request.settings, cancel)?;
                new_server::switch(&server, &store, &mut state, emit)?;
            }
            Rebuild::Unsupported => return Err(Error::Invalid(UNSUPPORTED)),
        }
        drop(store);
        return finish(request, Driven::Committed, state, cancel, emit);
    }
    // Checked again while the update is unsent: the credentials may have changed since.
    accept(state.replacement_profile(), None, &request.settings)?;
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
        return tail(request, &state.profile, cancel, emit);
    }
    match Description::of(&state.profile).rebuild {
        Rebuild::InPlace => {
            let provider = RunPod::new(request.settings.credential()?);
            revert(&live::Pod::new(&provider, &store, cancel), &store, &mut state, emit)?;
        }
        Rebuild::NewServer => {
            let server = live::Released::of(&state.profile, &request.settings, cancel)?;
            if new_server::cancel(&server, &store, &mut state, emit)? == new_server::Cancelled::Untouched {
                return Ok(state);
            }
        }
        Rebuild::Unsupported => return Err(Error::Invalid(UNSUPPORTED)),
    }
    drop(store);
    tail(request, &state.profile, cancel, emit)
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

fn open(request: &Request) -> Result<(Store, Deployment)> {
    let store = Store::lock(&request.state_root)?;
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    if state.cloud_id != request.cloud_id
        || state.repository != request.repository
        || state.revision != request.revision
        || state.profile != crate::cloud_runtime::publishing::requested_profile(&state, &request.profile)
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
    Description::of(profile).rebuild != Rebuild::Unsupported
        && (profile.build.is_some() || quick_start::on_public_base(profile))
}

/// Why a cloud on a provider without rebuilds cannot rebuild.
const UNSUPPORTED: &str = "A cloud on this provider keeps the image it was created with; Rebuild is not available";

fn ready(state: &Deployment) -> Result<()> {
    state.refuse_pending_replacement()?;
    if Description::of(&state.profile).rebuild == Rebuild::Unsupported {
        return Err(Error::Invalid(UNSUPPORTED));
    }
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

/// Refuses to bind `next` when a person `confirmed` another profile, or when any of its
/// agents, not only an added one, has a configured credential that is unusable: a
/// reconnect needs each one, so the refusal comes before a long build.
fn accept(next: &Profile, confirmed: Option<&Profile>, settings: &Settings) -> Result<()> {
    if confirmed.is_some_and(|confirmed| confirmed != next) {
        return Err(Error::Invalid(CONFIRMED_OTHER));
    }
    super::validate_agent_auth(settings, &next.capabilities, true)
}

fn confirm_publication(state: &Deployment, confirmed: Option<&Plan>) -> Result<()> {
    if confirmed.map_or(state.publication.is_some(), |plan| {
        plan.publication.as_ref() != state.publication.as_deref()
    }) {
        return Err(Error::Invalid(UNCONFIRMED_PUBLISHER));
    }
    Ok(())
}

/// Refuses uncommitted files that are not exactly the ones a person `confirmed`: an image
/// can go to a public registry.
fn confirm_files(
    uncommitted: &crate::cloud_runtime::repository::uncommitted::Uncommitted,
    confirmed: Option<&Plan>,
) -> Result<()> {
    if confirmed.map_or(!uncommitted.is_empty(), |plan| plan.uncommitted != *uncommitted) {
        return Err(Error::Invalid(UNCONFIRMED_FILES));
    }
    Ok(())
}

fn begin(steps: &impl Steps, store: &Store, state: &mut Deployment, recipes: Recipes) -> Result<()> {
    state.begin_layered_replacement(OperationId::generate(), recipes.revision, recipes.siblings)?;
    state.replacement_changes_profile(recipes.profile)?;
    state.replacement_takes_uncommitted(recipes.uncommitted.hash())?;
    store.save(state)?;
    steps.checkpoint(Boundary::Begun)
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
                // A build labels its image with these settings (`image::settings_label`),
                // so only a recipe without a build gives back the same image.
                if journal.profile.is_some() {
                    return Err(Error::Invalid(SAME_IMAGE));
                }
                return Ok(Driven::Unchanged);
            }
            // Checked before the image is journaled, so every built replacement fits its worker.
            require_held(steps, store, state, &journal.tag, &image.digest, emit)?;
            state.replacement_built(image)?;
            store.save(state)?;
            steps.checkpoint(Boundary::Built)?;
        }
        ReplacementPhase::Built(image) => {
            emit(activity("Verifying the built image and its pull binding"));
            steps.verify(state, &image)?;
            // Checked again, as an earlier Horizon version journaled a built image unchecked.
            // The journal stays: hosted devices may already be released, and cancelling the
            // replacement arms them again.
            if !held(steps, state, &journal.tag, &image.digest, emit)? {
                return Err(Error::Invalid(compatibility::BUILT_IMAGE_TOO_LARGE));
            }
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

/// Refuses an image built under `tag` and pushed as `digest` that the worker's container
/// disk cannot hold, and drops the replacement, whose update was never sent.
fn require_held(
    steps: &impl Steps,
    store: &Store,
    state: &mut Deployment,
    tag: &str,
    digest: &str,
    emit: &dyn Fn(Event),
) -> Result<()> {
    if held(steps, state, tag, digest, emit)? {
        return Ok(());
    }
    state.discard_replacement()?;
    store.save(state)?;
    Err(Error::Invalid(compatibility::IMAGE_TOO_LARGE))
}

/// Whether the worker's container disk holds the image built under `tag` and pushed as
/// `digest` ([`compatibility::holds_image`]), after reporting both sizes.
fn held(steps: &impl Steps, state: &Deployment, tag: &str, digest: &str, emit: &dyn Fn(Event)) -> Result<bool> {
    let Some(bytes) = steps.unpacked_bytes(state, tag, digest)? else {
        return Ok(true);
    };
    // Rounded up to a tenth, so an image just over the disk never reads as equal to it.
    let tenths = bytes.div_ceil(compatibility::BYTES_PER_GB / 10);
    emit(Event::Output(format!(
        "The rebuilt image is {}.{} GB unpacked; this cloud's container disk (storage.container_gb) is {} GB",
        tenths / 10,
        tenths % 10,
        state.profile.storage.container_gb
    )));
    Ok(compatibility::holds_image(&state.profile, bytes))
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

/// Polls until every provider API reports `target`, within the longer readiness budget
/// of the bound and the replacement profile. A third image or a lost worker fails
/// closed and keeps the journal.
fn await_image(steps: &impl Provider, state: &Deployment, target: Observed, emit: &dyn Fn(Event)) -> Result<()> {
    emit(activity("Waiting for the provider to report the switched image"));
    // A rebuild that raises the readiness timeout gets the longer one for its own switch.
    let seconds = state
        .profile
        .bootstrap
        .readiness_seconds
        .max(state.replacement_profile().bootstrap.readiness_seconds);
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
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
    tail(request, &state.profile, cancel, emit)
}

/// Reconnects through the ordinary deployment path, which verifies the recorded image
/// strictly, waits for readiness and relaunches the sessions. The request binds
/// `profile`, the recorded one, which a committed replacement may have changed.
fn tail(request: &Request, profile: &Profile, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<Deployment> {
    let mut request = request.clone();
    request.profile.clone_from(profile);
    super::deploy(&request, cancel, &|event| {
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
