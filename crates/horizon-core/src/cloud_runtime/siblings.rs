//! Same-worker sibling repositories chosen on this machine for one cloud.
//!
//! A committed `placement: same_worker` declaration only makes a sibling available. The
//! machine-local [`Binding`] authorizes it and names its checkout; paths never enter
//! committed configuration. Resolution pins each chosen sibling's committed revision and
//! recipe before the image is built, and the deployment record keeps the result.
use super::{
    Error, Result,
    command::{Runner, TIMED_OUT},
    repository,
    state::Deployment,
};
use horizon_cloud::{Build, CloudConfig, Profile, companions::Placement};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

mod worker;
pub use worker::{git_grants, session_worktree};

/// Printed by `horizon-worker-check` when the worker lays out sibling checkouts per session.
pub const CONTRACT: &str = "horizon-siblings-contract=1";
/// The deployment record version that carries siblings. Earlier Horizon versions refuse it
/// instead of saving the record back without them. Version 2 of `deployment.json` already
/// names a migration barrier or a migrated project record.
pub(super) const RECORD_VERSION: u32 = 3;
/// The worker's manifest limit.
const MAX_SIBLINGS: usize = 16;

/// A machine-local choice to deploy the declared same-worker `alias` from `local_repository`,
/// an absolute path to the checkout or a directory inside it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub alias: String,
    pub local_repository: PathBuf,
    /// The commit the checkout was reviewed at, when one was. Resolution then pins exactly
    /// this commit and refuses a checkout whose `HEAD` moved away from it. Omitted when
    /// none, so earlier records keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// One sibling pinned for a deployment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sibling {
    pub alias: String,
    /// GitHub `owner/name`, as declared.
    pub repository: String,
    /// Its checkout directory beside the primary's on the worker.
    pub directory: String,
    /// The committed `HEAD` of `local_repository` when the sibling was chosen, and the
    /// revision its checkout on the worker was imported at.
    pub revision: String,
    /// The commit whose recipe the current image layers, once an image rebuild moved past
    /// `revision`. The checkout on the worker stays at `revision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_revision: Option<String>,
    /// Canonical checkout on this machine; never sent to a worker.
    pub local_repository: PathBuf,
    /// The profile in the sibling's own committed configuration whose recipe is layered.
    pub profile: String,
}

/// The siblings of one deployment, in the order their recipes are layered. It keeps local
/// paths, so a worker receives only a manifest built from its aliases, directories and
/// revisions, never the set itself.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    /// The primary checkout's directory: its repository name without the owner.
    pub primary_directory: String,
    pub members: Vec<Sibling>,
}

/// A declared sibling a user may choose, with a checkout to suggest when one fits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub alias: String,
    pub repository: String,
    pub directory: String,
    /// `../<directory>` beside the primary checkout when its origin is the declared repository.
    pub suggested: Option<PathBuf>,
}

/// Each chosen sibling pinned or refused on its own, and any refusal of the choice as a
/// whole, for New cloud to show beside its rows before anything is deployed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Review {
    /// By alias.
    pub siblings: BTreeMap<String, std::result::Result<Sibling, SiblingError>>,
    /// Such as a primary without a recipe to layer onto, which refuses every sibling alike.
    pub choice: Option<SiblingError>,
}

impl Review {
    /// Whether a launch would pin these siblings as reviewed.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.choice.is_none() && self.siblings.values().all(std::result::Result::is_ok)
    }
}

/// Why chosen siblings cannot be deployed. Each names what to change.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SiblingError {
    #[error("At most {MAX_SIBLINGS} same-worker siblings fit on one worker")]
    TooMany,
    #[error("Sibling `{0}` was chosen twice")]
    Duplicate(String),
    #[error("Sibling `{0}` is not declared under companions in the primary .horizon/cloud.yml")]
    Undeclared(String),
    #[error("Companion `{0}` runs on its own cloud; declare it with placement: same_worker to use it as a sibling")]
    SeparateCloud(String),
    #[error("Same-worker siblings need a GitHub origin on the primary checkout")]
    PrimaryOrigin,
    #[error("Same-worker siblings layer onto the primary recipe; add a build section to the primary profile")]
    PrimaryImageOnly,
    #[error("Sibling `{0}` needs the absolute path of its local checkout")]
    Relative(String),
    #[error("Sibling `{0}` needs a Git checkout with a GitHub origin")]
    Origin(String),
    #[error("Sibling `{alias}` checkout comes from {found}, but its declaration names {declared}")]
    OriginMismatch {
        alias: String,
        declared: String,
        found: String,
    },
    #[error("Sibling `{0}` is the primary repository itself")]
    Primary(String),
    #[error(
        "Sibling `{0}` would share a checkout directory with the primary or another sibling; directory names ignore case"
    )]
    Directory(String),
    #[error(
        "Repository name `{0}` cannot name a checkout directory on the worker; it must not start with a dot or hyphen"
    )]
    DirectoryName(String),
    #[error("Sibling `{0}` checkout has no commit to deploy")]
    NoCommit(String),
    #[error("Sibling `{0}` checkout no longer has the commit pinned for this cloud")]
    RevisionUnavailable(String),
    #[error("Sibling `{0}` has no readable .horizon/cloud.yml at its committed HEAD")]
    MissingConfig(String),
    #[error("Sibling `{0}` has an invalid .horizon/cloud.yml at its committed HEAD")]
    InvalidConfig(String),
    #[error("Sibling `{alias}` has no profile `{profile}` in its committed .horizon/cloud.yml")]
    MissingProfile { alias: String, profile: String },
    #[error("Sibling `{alias}` profile `{profile}` uses a prebuilt image; only a build section can be layered")]
    ImageOnly { alias: String, profile: String },
    #[error("Sibling `{alias}` builds for {sibling}, but the primary builds for {primary}")]
    Platform {
        alias: String,
        sibling: String,
        primary: String,
    },
    #[error(
        "Sibling `{0}` recipe does not build on the image before it; declare `ARG HORIZON_BASE` before `FROM ${{HORIZON_BASE}}`"
    )]
    Base(String),
    #[error(
        "This cloud's same-worker siblings and their checkouts were pinned when it was first deployed; create a new cloud to change them"
    )]
    Rebound,
    #[error("This cloud's image was built without same-worker siblings; create a new cloud to add them")]
    Late,
    #[error("Sibling `{0}` checkout is no longer where it was pinned from; restore it there or create a new cloud")]
    Moved(String),
    #[error("Sibling `{0}` checkout moved to another commit after it was checked; check it again")]
    Advanced(String),
    #[error(
        "The latest commit changes this cloud's same-worker siblings or their repositories; create a new cloud to use them"
    )]
    Changed,
}

impl SiblingError {
    /// The chosen sibling this refusal names, if it names one.
    #[must_use]
    pub fn alias(&self) -> Option<&str> {
        match self {
            Self::Duplicate(alias)
            | Self::Undeclared(alias)
            | Self::SeparateCloud(alias)
            | Self::Relative(alias)
            | Self::Origin(alias)
            | Self::Primary(alias)
            | Self::Directory(alias)
            | Self::NoCommit(alias)
            | Self::RevisionUnavailable(alias)
            | Self::MissingConfig(alias)
            | Self::InvalidConfig(alias)
            | Self::Base(alias)
            | Self::Moved(alias)
            | Self::Advanced(alias)
            | Self::OriginMismatch { alias, .. }
            | Self::MissingProfile { alias, .. }
            | Self::ImageOnly { alias, .. }
            | Self::Platform { alias, .. } => Some(alias),
            Self::TooMany
            | Self::PrimaryOrigin
            | Self::PrimaryImageOnly
            | Self::DirectoryName(_)
            | Self::Rebound
            | Self::Late
            | Self::Changed => None,
        }
    }
}

impl Sibling {
    /// Refuses a checkout whose `HEAD` moved since this sibling was resolved, so a launch
    /// pins only the commit that was reviewed.
    /// # Errors
    /// [`SiblingError::Advanced`] for a moved `HEAD`, a refusal naming the checkout when Git
    /// cannot resolve it, and Git that could not run, timed out or was cancelled.
    pub fn unmoved(&self, runner: &Runner<'_>) -> Result<()> {
        let head = repository::resolve_with_runner(&self.local_repository, "HEAD", runner)
            .map_err(|error| refused(error, SiblingError::NoCommit(self.alias.clone())))?;
        if head == self.revision {
            Ok(())
        } else {
            Err(SiblingError::Advanced(self.alias.clone()).into())
        }
    }

    /// The pinned checkout, which must still be where it was chosen.
    /// # Errors
    /// The checkout was moved or removed.
    pub fn checkout(&self) -> Result<&Path> {
        if self.local_repository.is_dir() {
            Ok(&self.local_repository)
        } else {
            Err(SiblingError::Moved(self.alias.clone()).into())
        }
    }

    /// The recipe committed at `revision`, checked against the primary platform.
    /// # Errors
    /// The checkout lost that commit, its configuration, profile or build section is
    /// missing or incompatible, Git could not run or timed out, or the runner was cancelled.
    pub fn recipe(&self, revision: &str, primary: &Build, runner: &Runner<'_>) -> Result<Build> {
        recipe(
            &self.alias,
            &self.local_repository,
            revision,
            &self.profile,
            primary,
            runner,
        )
    }
}

/// Pins the siblings chosen for a deployment whose image is not built yet. A later deploy
/// without bindings, such as a reconnect, keeps the recorded set; a different choice is
/// refused, as for a different primary revision. Returns whether `state` changed.
/// # Errors
/// Any resolution error, and a choice made after the image was built.
pub(super) fn bind(bindings: &[Binding], state: &mut Deployment, runner: &Runner<'_>) -> Result<bool> {
    if bindings.is_empty() {
        return Ok(false);
    }
    if let Some(set) = &state.siblings {
        return if set.chosen_by(bindings, runner)? {
            Ok(false)
        } else {
            Err(SiblingError::Rebound.into())
        };
    }
    if state.spec.is_some() {
        return Err(SiblingError::Late.into());
    }
    let config = repository::launch::committed_config(&state.repository, &state.revision, runner)?.ok_or(
        Error::Invalid("The primary revision has no readable .horizon/cloud.yml"),
    )?;
    let Some(set) = resolve(&state.repository, &config, &state.profile, bindings, runner)? else {
        return Ok(false);
    };
    state.siblings = Some(set);
    state.version = RECORD_VERSION;
    Ok(true)
}

/// The latest committed revision of each recorded sibling for an image rebuild, resolved
/// from its recorded checkout exactly as a deploy resolves a choice, against `config`, the
/// primary's configuration at the commit being rebuilt. Only revisions may move: the
/// worker's checkout directories and repositories are fixed.
/// # Errors
/// Any resolution refusal, and [`SiblingError::Changed`] when the siblings resolve to
/// anything but new revisions of the recorded ones.
pub fn latest(
    set: &Set,
    primary: &Path,
    config: &CloudConfig,
    profile: &Profile,
    runner: &Runner<'_>,
) -> Result<Vec<String>> {
    let bindings: Vec<_> = set
        .members
        .iter()
        .map(|sibling| {
            Ok(Binding {
                alias: sibling.alias.clone(),
                local_repository: sibling.checkout()?.to_path_buf(),
                revision: None,
            })
        })
        .collect::<Result<_>>()?;
    // A recorded set has members, so resolution always returns one.
    let resolved = resolve(primary, config, profile, &bindings, runner)
        .map_err(|error| match error {
            // The latest commit declares or names the siblings differently than this cloud.
            Error::Sibling(
                SiblingError::Undeclared(_)
                | SiblingError::SeparateCloud(_)
                | SiblingError::Primary(_)
                | SiblingError::Directory(_)
                | SiblingError::DirectoryName(_)
                | SiblingError::OriginMismatch { .. }
                | SiblingError::PrimaryOrigin,
            ) => SiblingError::Changed.into(),
            other => other,
        })?
        .ok_or(SiblingError::Changed)?;
    let same = |recorded: &Sibling, latest: &Sibling| {
        (
            &recorded.alias,
            &recorded.repository,
            &recorded.directory,
            &recorded.local_repository,
            &recorded.profile,
        ) == (
            &latest.alias,
            &latest.repository,
            &latest.directory,
            &latest.local_repository,
            &latest.profile,
        )
    };
    if resolved.primary_directory != set.primary_directory
        || resolved.members.len() != set.members.len()
        || !set
            .members
            .iter()
            .zip(&resolved.members)
            .all(|(recorded, latest)| same(recorded, latest))
    {
        return Err(SiblingError::Changed.into());
    }
    Ok(resolved.members.into_iter().map(|sibling| sibling.revision).collect())
}

impl Set {
    /// A recorded set names at least one sibling, since one without members decodes as
    /// none, and no more than a worker accepts, each at commit IDs.
    #[must_use]
    pub fn fits(&self) -> bool {
        (1..=MAX_SIBLINGS).contains(&self.members.len())
            && self.members.iter().all(|sibling| {
                repository::is_commit_id(&sibling.revision)
                    && sibling
                        .image_revision
                        .as_deref()
                        .is_none_or(|revision| repository::is_commit_id(revision) && revision != sibling.revision)
            })
    }

    /// What an image rebuild at `latest`, in layering order, changes for each sibling
    /// whose recipe moved past the one the current image layers.
    #[must_use]
    pub fn moved(&self, latest: &[String]) -> Vec<String> {
        let short = |revision: &str| revision.get(..12).unwrap_or(revision).to_owned();
        self.members
            .iter()
            .zip(latest)
            .filter(|(sibling, latest)| sibling.image_revision.as_ref().unwrap_or(&sibling.revision) != *latest)
            .map(|(sibling, latest)| {
                format!(
                    "Sibling {} has a newer recipe at {}: a changed image layers it, and its checkout on the worker stays at {}",
                    sibling.alias,
                    short(latest),
                    short(&sibling.revision)
                )
            })
            .collect()
    }

    /// Whether `bindings` name the recorded aliases, in order, at the same checkouts. A
    /// binding inside a checkout names its top level, as it did when the set was resolved.
    fn chosen_by(&self, bindings: &[Binding], runner: &Runner<'_>) -> Result<bool> {
        if self.members.len() != bindings.len() {
            return Ok(false);
        }
        for (sibling, binding) in self.members.iter().zip(bindings) {
            if sibling.alias != binding.alias
                || binding
                    .revision
                    .as_ref()
                    .is_some_and(|expected| *expected != sibling.revision)
            {
                return Ok(false);
            }
            let checkout = top_level(&binding.local_repository, runner)
                .map_err(|error| refused(error, SiblingError::Origin(binding.alias.clone())))?;
            if checkout != sibling.local_repository {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Decodes a recorded set, treating one without members as no siblings.
    /// # Errors
    /// The value is not a set.
    pub(super) fn decode<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Self>, D::Error> {
        Ok(Option::<Self>::deserialize(deserializer)?.filter(|set| !set.members.is_empty()))
    }
}

/// Pins each binding's committed `HEAD` and recipe, in binding order. `config` is the
/// primary's committed configuration at the revision being deployed, which declares the
/// siblings. Without bindings there are no siblings and the primary is not inspected.
/// # Errors
/// A [`SiblingError`] for a choice that cannot be deployed, or Git that could not run,
/// timed out or was cancelled.
pub fn resolve(
    primary: &Path,
    config: &CloudConfig,
    profile: &Profile,
    bindings: &[Binding],
    runner: &Runner<'_>,
) -> Result<Option<Set>> {
    if bindings.is_empty() {
        return Ok(None);
    }
    if bindings.len() > MAX_SIBLINGS {
        return Err(SiblingError::TooMany.into());
    }
    let primary_build = profile.build.as_ref().ok_or(SiblingError::PrimaryImageOnly)?;
    let primary_repository = origin(primary, runner, || SiblingError::PrimaryOrigin)?;
    let primary_directory = checkout_directory(&primary_repository)?.to_owned();
    let primary_checkout = top_level(primary, runner).map_err(|error| refused(error, SiblingError::PrimaryOrigin))?;
    let mut aliases = HashSet::new();
    let mut directories = HashSet::from([primary_directory.to_ascii_lowercase()]);
    let mut members = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let alias = &binding.alias;
        if !aliases.insert(alias) {
            return Err(SiblingError::Duplicate(alias.clone()).into());
        }
        let declaration = config
            .companions
            .get(alias)
            .ok_or_else(|| SiblingError::Undeclared(alias.clone()))?;
        if declaration.placement != Placement::SameWorker {
            return Err(SiblingError::SeparateCloud(alias.clone()).into());
        }
        if declaration.repository.eq_ignore_ascii_case(&primary_repository) {
            return Err(SiblingError::Primary(alias.clone()).into());
        }
        if !binding.local_repository.is_absolute() {
            return Err(SiblingError::Relative(alias.clone()).into());
        }
        let checkout = top_level(&binding.local_repository, runner)
            .map_err(|error| refused(error, SiblingError::Origin(alias.clone())))?;
        if checkout == primary_checkout {
            return Err(SiblingError::Primary(alias.clone()).into());
        }
        let found = origin(&checkout, runner, || SiblingError::Origin(alias.clone()))?;
        if !found.eq_ignore_ascii_case(&declaration.repository) {
            return Err(SiblingError::OriginMismatch {
                alias: alias.clone(),
                declared: declaration.repository.clone(),
                found,
            }
            .into());
        }
        let directory = checkout_directory(&declaration.repository)?;
        if !directories.insert(directory.to_ascii_lowercase()) {
            return Err(SiblingError::Directory(alias.clone()).into());
        }
        let revision = repository::resolve_with_runner(&checkout, "HEAD", runner)
            .map_err(|error| refused(error, SiblingError::NoCommit(alias.clone())))?;
        if binding.revision.as_ref().is_some_and(|expected| *expected != revision) {
            return Err(SiblingError::Advanced(alias.clone()).into());
        }
        recipe(alias, &checkout, &revision, &declaration.profile, primary_build, runner)?;
        members.push(Sibling {
            alias: alias.clone(),
            repository: declaration.repository.clone(),
            directory: directory.to_owned(),
            revision,
            image_revision: None,
            local_repository: checkout,
            profile: declaration.profile.clone(),
        });
    }
    Ok(Some(Set {
        primary_directory,
        members,
    }))
}

/// Declared same-worker siblings of a primary checkout. Listing authorizes nothing.
/// # Errors
/// Only cancellation; an unreadable neighbouring checkout is simply not suggested.
pub fn candidates(primary: &Path, config: &CloudConfig, runner: &Runner<'_>) -> Result<Vec<Candidate>> {
    let parent = match top_level(primary, runner) {
        Ok(checkout) => checkout.parent().map(Path::to_path_buf),
        Err(error @ Error::Provider(horizon_cloud::CloudError::Cancelled)) => return Err(error),
        Err(_) => None,
    };
    config
        .same_worker_siblings()
        .map(|(alias, declaration)| {
            runner.cancel.check()?;
            let directory = declaration.directory_name();
            let suggested = parent.as_ref().map(|parent| parent.join(directory)).filter(|path| {
                checkout_directory(&declaration.repository).is_ok()
                    && path.is_dir()
                    && origin(path, runner, || SiblingError::Origin(alias.to_owned()))
                        .is_ok_and(|found| found.eq_ignore_ascii_case(&declaration.repository))
            });
            runner.cancel.check()?;
            Ok(Candidate {
                alias: alias.to_owned(),
                repository: declaration.repository.clone(),
                directory: directory.to_owned(),
                suggested,
            })
        })
        .collect()
}

/// Resolves each binding on its own, so every chosen sibling gets its own refusal, then the
/// whole choice as [`resolve`] would at launch. Resolving pins nothing.
/// # Errors
/// Git that could not run, timed out or was cancelled.
pub fn review(
    primary: &Path,
    config: &CloudConfig,
    profile: &Profile,
    bindings: &[Binding],
    runner: &Runner<'_>,
) -> Result<Review> {
    let mut review = Review::default();
    for binding in bindings {
        let outcome = match resolve(primary, config, profile, std::slice::from_ref(binding), runner) {
            Ok(set) => set
                .and_then(|set| set.members.into_iter().next())
                .ok_or(Error::Invalid("A chosen sibling resolved to nothing"))?,
            Err(Error::Sibling(refusal @ (SiblingError::PrimaryOrigin | SiblingError::PrimaryImageOnly))) => {
                review.choice = Some(refusal);
                return Ok(review);
            }
            Err(Error::Sibling(refusal)) => {
                review.siblings.insert(binding.alias.clone(), Err(refusal));
                continue;
            }
            Err(error) => return Err(error),
        };
        review.siblings.insert(binding.alias.clone(), Ok(outcome));
    }
    if review.passed() && bindings.len() > 1 {
        match resolve(primary, config, profile, bindings, runner) {
            Ok(_) => {}
            Err(Error::Sibling(refusal)) => match refusal.alias() {
                Some(alias) => {
                    review.siblings.insert(alias.to_owned(), Err(refusal));
                }
                None => review.choice = Some(refusal),
            },
            Err(error) => return Err(error),
        }
    }
    Ok(review)
}

fn recipe(
    alias: &str,
    repository: &Path,
    revision: &str,
    profile: &str,
    primary: &Build,
    runner: &Runner<'_>,
) -> Result<Build> {
    repository::resolve_with_runner(repository, revision, runner)
        .map_err(|error| refused(error, SiblingError::RevisionUnavailable(alias.to_owned())))?;
    let config = repository::launch::committed_config(repository, revision, runner)
        .map_err(|error| match error {
            Error::Invalid(repository::launch::INVALID_CONFIG) => SiblingError::InvalidConfig(alias.to_owned()).into(),
            // Git that could not run, timed out or produced too much output is not the sibling's fault.
            other => other,
        })?
        .ok_or_else(|| SiblingError::MissingConfig(alias.to_owned()))?;
    let selected = config
        .profiles
        .get(profile)
        .ok_or_else(|| SiblingError::MissingProfile {
            alias: alias.to_owned(),
            profile: profile.to_owned(),
        })?;
    let build = selected.build.clone().ok_or_else(|| SiblingError::ImageOnly {
        alias: alias.to_owned(),
        profile: profile.to_owned(),
    })?;
    if build.platform != primary.platform {
        return Err(SiblingError::Platform {
            alias: alias.to_owned(),
            sibling: build.platform,
            primary: primary.platform.clone(),
        }
        .into());
    }
    Ok(build)
}

/// The GitHub `owner/name` of a checkout's origin.
fn origin(path: &Path, runner: &Runner<'_>, refusal: impl FnOnce() -> SiblingError) -> Result<String> {
    super::companions::inventory::identity(path, runner).map_err(|error| refused(error, refusal()))
}

fn top_level(path: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    let root = runner.run(
        "Find sibling Git repository",
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"]),
        Duration::from_secs(30),
    )?;
    Ok(Path::new(root.trim_end_matches(['\r', '\n'])).canonicalize()?)
}

/// The repository name without its owner, which the worker accepts as a checkout directory
/// only when it does not start with a dot or hyphen.
fn checkout_directory(repository: &str) -> std::result::Result<&str, SiblingError> {
    let name = repository.split_once('/').map_or(repository, |(_, name)| name);
    if name.is_empty() || name.starts_with(['.', '-']) {
        return Err(SiblingError::DirectoryName(name.to_owned()));
    }
    Ok(name)
}

/// Keeps cancellation, timeouts and I/O failures, such as a missing `git`; any other
/// failure, a Git command that refused or output that is not what was asked for, becomes
/// the actionable `refusal`.
fn refused(error: Error, refusal: SiblingError) -> Error {
    match error {
        Error::Provider(horizon_cloud::CloudError::Cancelled) | Error::Io(_) | Error::Invalid(TIMED_OUT) => error,
        _ => refusal.into(),
    }
}

#[cfg(test)]
mod tests;
