//! The recipe a rebuild builds, from the checkout's latest commit with its uncommitted
//! changes, and the profile its cloud binds with it: what [`plan`] shows a person before
//! they confirm, and what a rebuild journals.
use super::{Deployment, Error, Event, NO_RECIPE, Result, activity, compatibility, quick_start, rebuildable};
use crate::cloud_runtime::{command::Runner, repository::uncommitted::Uncommitted};
use horizon_cloud::{Build, Cancellation, CloudConfig, Profile};
use std::path::Path;

const COMMITTED_BASE: &str = "This cloud runs the public base image without a build section, and the latest commit has its own .horizon/cloud.yml. Only a cloud of a repository without one moves to the pinned base image; create a new cloud to use the committed settings.";
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
    /// The latest committed revision of each of the deployment's siblings, whose
    /// declarations `config`, the primary's configuration with its committed companions,
    /// holds for the primary's `profile`.
    fn siblings(&self, state: &Deployment, config: &CloudConfig, profile: &Profile) -> Result<Vec<String>>;
    /// The uncommitted files of the working copy against `revision` that `build` reads.
    fn uncommitted(&self, repository: &Path, revision: &str, build: &Build) -> Result<Uncommitted>;
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
}

impl Plan {
    /// A SHA-256 over the profile the rebuild binds and the hash of its uncommitted files,
    /// as 64 lowercase hexadecimal digits: a person who saw a printed plan confirms
    /// exactly that plan by giving it back (`cloud_deploy rebuild`).
    #[must_use]
    pub fn token(&self) -> String {
        crate::cloud_runtime::repository::uncommitted::token(&self.profile, &self.uncommitted)
    }
}

/// What a rebuild of a cloud bound to `bound` in `repository` would bind and build from
/// its latest commit and working copy, for a person to confirm. Reads the checkout only,
/// so the cloud's record is not locked.
/// # Errors
/// Refuses as [`super::rebuild`] refuses the profile named `profile_name` and the
/// uncommitted files.
pub fn plan(repository: &Path, bound: &Profile, profile_name: &str, cancel: &Cancellation) -> Result<Plan> {
    if !rebuildable(bound) {
        return Err(Error::Invalid(NO_RECIPE));
    }
    let checkout = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let head = checkout.head(repository)?;
    let profile = next_profile(&head, bound, profile_name)?;
    let uncommitted = uncommitted(&checkout, repository, &head, &profile)?;
    let mut notes = notes(bound, head.notes);
    notes.extend(crate::cloud_runtime::repository::launch::idle_period_note(
        profile_name,
        &profile,
    ));
    Ok(Plan {
        changes: compatibility::changes(bound, &profile),
        profile,
        notes,
        uncommitted,
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
    let head = recipe.head(&state.repository)?;
    let profile = next_profile(&head, &state.profile, profile_name)?;
    let uncommitted = uncommitted(recipe, &state.repository, &head, &profile)?;
    for note in notes(&state.profile, head.notes) {
        emit(Event::Output(note));
    }
    if quick_start::on_public_base(&state.profile) {
        emit(activity("Using the base image that this Horizon version pins"));
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
/// settings are never passed over: with them such a cloud is refused.
fn next_profile(head: &Head, bound: &Profile, profile_name: &str) -> Result<Profile> {
    if !quick_start::on_public_base(bound) {
        return compatibility::next_profile(bound, head.config.as_ref(), profile_name);
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
