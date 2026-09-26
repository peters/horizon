//! Same-worker sibling repositories chosen on this machine for one cloud.
//!
//! A committed `placement: same_worker` declaration only makes a sibling available. The
//! machine-local [`Binding`] authorizes it and names its checkout; paths never enter
//! committed configuration. Resolution pins each chosen sibling's committed revision and
//! recipe before the image is built.
use super::{
    Error, Result,
    command::{Runner, TIMED_OUT},
    repository,
};
use horizon_cloud::{Build, CloudConfig, Profile, companions::Placement};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Printed by `horizon-worker-check` when the worker lays out sibling checkouts per session.
pub const CONTRACT: &str = "horizon-siblings-contract=1";
/// The worker's manifest limit.
const MAX_SIBLINGS: usize = 16;

/// A machine-local choice to deploy the declared same-worker `alias` from `local_repository`,
/// an absolute path to the checkout or a directory inside it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub alias: String,
    pub local_repository: PathBuf,
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
    /// The committed `HEAD` of `local_repository` when the sibling was chosen.
    pub revision: String,
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
}

impl Sibling {
    /// The recipe committed at the pinned revision, checked against the primary platform.
    /// # Errors
    /// The checkout lost the pinned commit, its configuration, profile or build section is
    /// missing or incompatible, Git could not run or timed out, or the runner was cancelled.
    pub fn recipe(&self, primary: &Build, runner: &Runner<'_>) -> Result<Build> {
        recipe(
            &self.alias,
            &self.local_repository,
            &self.revision,
            &self.profile,
            primary,
            runner,
        )
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
        recipe(alias, &checkout, &revision, &declaration.profile, primary_build, runner)?;
        members.push(Sibling {
            alias: alias.clone(),
            repository: declaration.repository.clone(),
            directory: directory.to_owned(),
            revision,
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
    let config = committed_config(alias, repository, revision, runner)?;
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

/// The configuration committed at `revision`. Only a failed lookup means it is missing;
/// its content is never emitted, since it may hold invalid secret-bearing fields.
fn committed_config(alias: &str, repository: &Path, revision: &str, runner: &Runner<'_>) -> Result<CloudConfig> {
    let yaml = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Read sibling cloud configuration",
        Command::new("git").arg("-C").arg(repository).args([
            "cat-file",
            "blob",
            &format!("{revision}:.horizon/cloud.yml"),
        ]),
        Duration::from_secs(30),
    )
    .map_err(|error| refused(error, SiblingError::MissingConfig(alias.to_owned())))?;
    CloudConfig::parse(&yaml).map_err(|_| SiblingError::InvalidConfig(alias.to_owned()).into())
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
