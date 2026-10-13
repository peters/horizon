//! The latest committed recipe a rebuild builds, and the profile its cloud binds with it:
//! what [`plan`] shows a person before they confirm, and what a rebuild journals.
use super::{Deployment, Error, Event, NO_RECIPE, Result, activity, compatibility, quick_start, rebuildable};
use crate::cloud_runtime::command::Runner;
use horizon_cloud::{Cancellation, CloudConfig, Profile};
use std::path::Path;

const COMMITTED_BASE: &str = "This cloud runs the public base image without a build section, and the latest commit has its own .horizon/cloud.yml. Only a cloud of a repository without one moves to the pinned base image; create a new cloud to use the committed settings.";
/// Starts each output line that names a change of the cloud's settings.
pub const PROFILE_CHANGE: &str = "Profile change: ";

/// The repository's committed HEAD and its `.horizon/cloud.yml`, if readable.
#[derive(Clone)]
pub(super) struct Head {
    pub(super) revision: String,
    pub(super) config: Option<CloudConfig>,
}

/// The rest of a rebuild outside its record.
/// Reads the repository's committed recipe; needs no credentials.
pub(super) trait Recipe {
    fn head(&self, repository: &Path) -> Result<Head>;
    /// The latest committed revision of each of the deployment's siblings, whose
    /// declarations `config`, the primary's latest committed configuration, holds for
    /// the primary's `profile`.
    fn siblings(&self, state: &Deployment, config: &CloudConfig, profile: &Profile) -> Result<Vec<String>>;
}

/// The latest committed recipes a rebuild layers, the primary's and each sibling's, and
/// the profile the cloud binds with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Recipes {
    pub(super) revision: String,
    pub(super) siblings: Vec<String>,
    pub(super) profile: Profile,
}

/// What a rebuild of a cloud would bind from its latest commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The cloud's profile after the rebuild.
    pub profile: Profile,
    /// What that changes, one plain sentence each, such as `Adds agent codex`; empty
    /// when the settings stay as they are.
    pub changes: Vec<String>,
}

/// What a rebuild of a cloud bound to `bound` in `repository` would bind from its latest
/// commit, for a person to confirm. Reads the committed recipe only, so the cloud's
/// record is not locked.
/// # Errors
/// Refuses as [`super::rebuild`] refuses the committed profile named `profile_name`.
pub fn plan(repository: &Path, bound: &Profile, profile_name: &str, cancel: &Cancellation) -> Result<Plan> {
    if !rebuildable(bound) {
        return Err(Error::Invalid(NO_RECIPE));
    }
    let checkout = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let profile = next_profile(&checkout.head(repository)?, bound, profile_name)?;
    Ok(Plan {
        changes: compatibility::changes(bound, &profile),
        profile,
    })
}

/// The committed recipes to rebuild and the profile the cloud binds with them
/// ([`next_profile`]): the primary's latest commit and each sibling's.
pub(super) fn committed_recipes(
    recipe: &impl Recipe,
    state: &Deployment,
    profile_name: &str,
    emit: &dyn Fn(Event),
) -> Result<Recipes> {
    emit(activity("Reading the latest committed recipe"));
    let head = recipe.head(&state.repository)?;
    let profile = next_profile(&head, &state.profile, profile_name)?;
    if quick_start::on_public_base(&state.profile) {
        emit(activity("Using the base image that this Horizon version pins"));
    }
    let (Some(set), Some(config)) = (&state.siblings, &head.config) else {
        return Ok(Recipes {
            revision: head.revision,
            siblings: Vec::new(),
            profile,
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
    })
}

/// The profile a cloud bound to `bound` binds once a rebuild to `head` commits: the
/// committed profile named `profile_name` with the choices bound at creation kept
/// ([`compatibility::next_profile`]). A cloud on the public base image has no recipe;
/// while the latest commit has no `.horizon/cloud.yml`, as for quick start, its image is
/// the pin of this Horizon version and its profile stays. Committed settings are never
/// passed over: with them such a cloud is refused.
fn next_profile(head: &Head, bound: &Profile, profile_name: &str) -> Result<Profile> {
    if !quick_start::on_public_base(bound) {
        return compatibility::next_profile(bound, head.config.as_ref(), profile_name);
    }
    if head.config.is_some() {
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
