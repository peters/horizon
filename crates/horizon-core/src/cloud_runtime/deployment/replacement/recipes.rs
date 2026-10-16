//! The recipe a rebuild builds, from the checkout's latest commit with its uncommitted
//! changes, and the profile its cloud binds with it: what [`plan`] shows a person before
//! they confirm, and what a rebuild journals.
use super::{Deployment, Error, Event, NO_RECIPE, Result, activity, compatibility, quick_start, rebuildable};
use crate::cloud_runtime::{
    command::Runner,
    repository::uncommitted::{self, Uncommitted},
};
use horizon_cloud::{Build, Cancellation, CloudConfig, Profile};
use std::path::Path;

const COMMITTED_BASE: &str = "This cloud runs the public base image without a build section, and the latest commit has its own .horizon/cloud.yml. Only a cloud of a repository without one moves to the pinned base image; create a new cloud to use the committed settings.";
const UPKEEP_BASE: &str = "This Upkeep moves to the base image that this Horizon version pins. It does not read the repository's .horizon/cloud.yml, as New Upkeep does not.";

/// Whether `cloud_id` is the cloud of an Upkeep: the saved choice of its Upkeep exists.
pub(super) fn upkeep_cloud(cloud_id: &str) -> bool {
    crate::upkeep::worker::directory(cloud_id).join("upkeep.json").is_file()
}

/// The latest commit of `repository` with its settings. For the cloud of an Upkeep on the
/// public base image (`upkeep`), only its revision: New Upkeep reads no settings of the
/// repository, so its rebuild does not either, also when they are not valid.
fn latest(recipe: &impl Recipe, repository: &Path, bound: &Profile, upkeep: bool) -> Result<Head> {
    if upkeep && quick_start::on_public_base(bound) {
        return Ok(Head {
            revision: recipe.revision(repository)?,
            committed: None,
            config: None,
            notes: Vec::new(),
        });
    }
    recipe.head(repository)
}
/// Starts each output line that names a change of the cloud's settings.
pub const PROFILE_CHANGE: &str = "Profile change: ";

/// The repository's HEAD with its committed `.horizon/cloud.yml` and the settings a
/// rebuild reads, the working copy's (`launch::for_rebuild`), if readable.
#[derive(Clone)]
pub(super) struct Head {
    pub(super) revision: String,
    pub(super) committed: Option<CloudConfig>,
    pub(super) config: Option<CloudConfig>,
    /// Why some working-copy settings do not apply.
    pub(super) notes: Vec<String>,
}

/// The rest of a rebuild outside its record.
/// Reads the repository's recipe; needs no credentials.
pub(super) trait Recipe {
    fn head(&self, repository: &Path) -> Result<Head>;
    /// The latest commit of `repository`, without its settings.
    fn revision(&self, repository: &Path) -> Result<String>;
    /// The latest committed revision of each of the deployment's siblings, whose
    /// declarations `config`, the primary's configuration with its committed companions,
    /// holds for the primary's `profile`.
    fn siblings(&self, state: &Deployment, config: &CloudConfig, profile: &Profile) -> Result<Vec<String>>;
    /// The uncommitted files of the working copy against `revision` that `build` reads.
    fn uncommitted(&self, repository: &Path, revision: &str, build: &Build) -> Result<Uncommitted>;
    /// Whether `cloud_id` is the cloud of an Upkeep. New Upkeep starts the quick-start
    /// profile whatever its repository commits, so its rebuild reads no committed settings.
    fn upkeep(&self, cloud_id: &str) -> bool;
}

/// The recipes a rebuild layers, the primary's with its uncommitted files and each
/// sibling's latest commit, and the profile the cloud binds with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Recipes {
    pub(super) revision: String,
    pub(super) siblings: Vec<String>,
    pub(super) profile: Profile,
    pub(super) uncommitted: Uncommitted,
}

/// What a rebuild of a cloud would bind and build from its checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The cloud's profile after the rebuild.
    pub profile: Profile,
    /// What that changes, one plain sentence each, such as `Adds agent codex`; empty
    /// when the settings stay as they are.
    pub changes: Vec<String>,
    /// Why some working-copy settings do not apply.
    pub notes: Vec<String>,
    /// The uncommitted files that go into the image's build context.
    pub uncommitted: Uncommitted,
    /// Exact nonsecret publishing identity shown with this rebuild confirmation.
    pub publication: Option<crate::cloud_runtime::publishing::Publication>,
}

impl Plan {
    /// A SHA-256 over the profile the rebuild binds and the hash of its uncommitted files,
    /// plus its retained publishing identity, as 64 lowercase hexadecimal digits.
    /// A person who saw a printed plan confirms
    /// exactly that plan by giving it back (`cloud_deploy rebuild`).
    #[must_use]
    pub fn token(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let legacy = uncommitted::token(&self.profile, &self.uncommitted);
        let Some(publication) = &self.publication else {
            return legacy;
        };
        let mut hash = Sha256::new();
        hash.update(legacy.as_bytes());
        hash.update([0]);
        hash.update(serde_json::to_vec(publication).unwrap_or_default());
        uncommitted::hex(&hash.finalize())
    }
}

/// What a rebuild of a cloud bound to `bound` in `repository` would bind and build from
/// its latest commit and working copy, for a person to confirm. Reads the checkout only,
/// so the cloud's record is not locked.
/// # Errors
/// Refuses as [`super::rebuild`] refuses the profile named `profile_name` and the
/// uncommitted files.
pub fn plan(repository: &Path, bound: &Profile, profile_name: &str, cancel: &Cancellation) -> Result<Plan> {
    plan_with(repository, bound, profile_name, None, false, cancel)
}

/// The rebuild plan with the destination and recipe image recorded for this cloud.
/// # Errors
/// As [`plan`], including inconsistent publication metadata.
pub fn plan_for_cloud(state: &Deployment, profile_name: &str, cancel: &Cancellation) -> Result<Plan> {
    plan_with(
        &state.repository,
        &state.profile,
        profile_name,
        state.publication.as_deref(),
        upkeep_cloud(&state.cloud_id),
        cancel,
    )
}

/// [`plan`] for a cloud that is the cloud of an Upkeep (`upkeep`) or not.
pub(super) fn plan_with(
    repository: &Path,
    bound: &Profile,
    profile_name: &str,
    publication: Option<&crate::cloud_runtime::publishing::Publication>,
    upkeep: bool,
    cancel: &Cancellation,
) -> Result<Plan> {
    if !rebuildable(bound) {
        return Err(Error::Invalid(NO_RECIPE));
    }
    let checkout = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let head = latest(&checkout, repository, bound, upkeep)?;
    let profile = next_profile(&head, bound, profile_name, publication)?;
    let uncommitted = uncommitted(&checkout, repository, &head, &profile)?;
    // An idle period is no rebuild setting, so the cloud keeps its own; the note names
    // the one the settings that a rebuild reads still set.
    let idle = head
        .config
        .as_ref()
        .and_then(|config| config.profiles.get(profile_name))
        .and_then(|read| crate::cloud_runtime::repository::launch::idle_period_note(profile_name, read));
    let mut notes = notes(bound, head.notes);
    if !quick_start::on_public_base(bound) {
        notes.extend(idle);
    }
    Ok(Plan {
        changes: compatibility::changes(bound, &profile),
        profile,
        notes,
        uncommitted,
        publication: publication.cloned(),
    })
}

/// The recipes to rebuild and the profile the cloud binds with them ([`next_profile`]):
/// the primary's latest commit with its uncommitted files, and each sibling's latest commit.
pub(super) fn latest_recipes(
    recipe: &impl Recipe,
    state: &Deployment,
    profile_name: &str,
    emit: &dyn Fn(Event),
) -> Result<Recipes> {
    emit(activity("Reading the recipe and its uncommitted changes"));
    let upkeep = recipe.upkeep(&state.cloud_id);
    let head = latest(recipe, &state.repository, &state.profile, upkeep)?;
    let profile = next_profile(&head, &state.profile, profile_name, state.publication.as_deref())?;
    let uncommitted = uncommitted(recipe, &state.repository, &head, &profile)?;
    for note in notes(&state.profile, head.notes) {
        emit(Event::Output(note));
    }
    if quick_start::on_public_base(&state.profile) {
        emit(activity("Using the base image that this Horizon version pins"));
        if upkeep {
            emit(Event::Output(UPKEEP_BASE.into()));
        }
    }
    let (Some(set), Some(config)) = (&state.siblings, &head.config) else {
        return Ok(Recipes {
            revision: head.revision,
            siblings: Vec::new(),
            profile,
            uncommitted,
        });
    };
    emit(activity("Reading each same-worker sibling's latest committed recipe"));
    let siblings = recipe.siblings(state, config, &profile)?;
    for note in set.moved(&siblings) {
        emit(Event::Output(note));
    }
    Ok(Recipes {
        revision: head.revision,
        siblings,
        profile,
        uncommitted,
    })
}

/// The uncommitted files the primary recipe of `profile` builds from; none for a cloud on
/// the public base image, which builds nothing.
fn uncommitted(recipe: &impl Recipe, repository: &Path, head: &Head, profile: &Profile) -> Result<Uncommitted> {
    profile.build.as_ref().map_or_else(
        || Ok(Uncommitted::default()),
        |build| recipe.uncommitted(repository, &head.revision, build),
    )
}

/// The working copy's notes; a cloud on the public base image reads only the commit.
fn notes(bound: &Profile, notes: Vec<String>) -> Vec<String> {
    if quick_start::on_public_base(bound) {
        Vec::new()
    } else {
        notes
    }
}

/// The profile a cloud bound to `bound` binds once a rebuild to `head` commits: the
/// profile named `profile_name` in the settings a rebuild reads, with the choices bound
/// at creation kept ([`compatibility::next_profile`]). A cloud on the public base image
/// has no recipe; while the latest commit has no `.horizon/cloud.yml`, as for quick
/// start, its image is the pin of this Horizon version and its profile stays. Committed
/// settings are never passed over: with them such a cloud is refused. The cloud of an
/// Upkeep reads none ([`latest`]).
fn next_profile(
    head: &Head,
    bound: &Profile,
    profile_name: &str,
    publication: Option<&crate::cloud_runtime::publishing::Publication>,
) -> Result<Profile> {
    if !quick_start::on_public_base(bound) {
        return compatibility::next_profile(bound, head.config.as_ref(), profile_name, publication);
    }
    if head.committed.is_some() {
        return Err(Error::Invalid(COMMITTED_BASE));
    }
    Ok(bound.clone())
}

/// Reports each change of the cloud's settings from `bound` to `next` as an output line.
pub(super) fn report_changes(bound: &Profile, next: &Profile, emit: &dyn Fn(Event)) {
    for change in compatibility::changes(bound, next) {
        emit(Event::Output(format!("{PROFILE_CHANGE}{change}")));
    }
}
