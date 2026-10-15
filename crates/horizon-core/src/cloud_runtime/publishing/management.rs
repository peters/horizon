//! Destination management shared by native and headless host adapters.
use super::{Override, Selection, choices, personal, selection};
use crate::cloud_runtime::{
    Cancellation, Error, Result, command::Runner, github::publish::publication, registry, repository::launch,
    settings::Settings, setup,
};
use horizon_cloud::Profile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub selection: Selection,
    pub username: String,
}

/// Historical configuration and nonsecret identity, never proof of current access.
#[derive(Clone, Debug, Serialize)]
pub struct Info {
    pub enabled: bool,
    pub selection: Option<Selection>,
    pub selection_error: Option<String>,
    pub choices: Vec<Choice>,
    pub shown_override: Option<Override>,
    pub pull_saved: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List {},
    Select {
        shown: Option<Override>,
        destination: Selection,
    },
    Reset {
        shown: Option<Override>,
    },
    Authenticate {
        shown: Option<Override>,
        switch: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub repository: PathBuf,
    pub profile: String,
    pub action: Action,
}

/// Read metadata for a profile already resolved by a trusted host adapter.
/// Run off the render thread. Existing sign-in metadata may be read, but this
/// never authenticates, makes a network request, writes settings or returns tokens.
/// # Errors
/// Invalid settings, cancellation or a repository that cannot be canonicalized.
pub fn inspect(root: &Path, repository: &Path, name: &str, profile: &Profile, cancel: &Cancellation) -> Result<Info> {
    inspect_with(root, repository, name, profile, cancel, |settings, repository| {
        selection(settings, repository, name, profile, cancel)
    })
}

fn inspect_with(
    root: &Path,
    repository: &Path,
    name: &str,
    profile: &Profile,
    cancel: &Cancellation,
    select: impl FnOnce(&Settings, &Path) -> Result<Selection>,
) -> Result<Info> {
    cancel.check()?;
    let settings = setup::load_settings(root)?;
    let repository = repository.canonicalize()?;
    cancel.check()?;
    if !settings.portable_image_publishing {
        return Ok(Info {
            enabled: false,
            selection: (!profile.automatic_image()).then(|| Selection::Recipe {
                image: profile.image.clone(),
            }),
            selection_error: None,
            choices: Vec::new(),
            shown_override: None,
            pull_saved: false,
        });
    }
    let selected = select(&settings, &repository);
    cancel.check()?;
    let (selection, selection_error) = match selected {
        Ok(selection) => (Some(selection), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let choices = choices(&settings)?
        .into_iter()
        .map(|selection| {
            let username = settings
                .registries
                .as_ref()
                .and_then(|config| {
                    config
                        .bindings
                        .iter()
                        .find(|binding| Some(binding.repository.as_str()) == selection.repository())
                })
                .and_then(|binding| binding.publish.as_ref())
                .map_or_else(String::new, |auth| auth.username.clone());
            Choice { selection, username }
        })
        .collect();
    let shown_override = settings
        .image_publishing
        .iter()
        .find(|choice| choice.repository == repository && choice.profile == name)
        .cloned();
    let pull_saved = selection
        .as_ref()
        .and_then(Selection::repository)
        .and_then(registry::repository_of)
        .is_some_and(|target| {
            settings.registries.as_ref().is_some_and(|config| {
                config
                    .bindings
                    .iter()
                    .any(|binding| binding.repository == target && binding.read_only_confirmed)
            })
        });
    cancel.check()?;
    Ok(Info {
        enabled: true,
        selection,
        selection_error,
        choices,
        shown_override,
        pull_saved,
    })
}

/// Resolve the working-copy profile and manage only its machine-local destination.
/// Repository YAML cannot choose a credential or supply an authentication endpoint.
/// Authentication changes the computer's current publisher; retained cloud accounts stay pinned.
/// # Errors
/// Invalid scope, stale shown choice, disabled feature or refused/cancelled authentication.
pub fn manage(root: &Path, request: &Request, operation_id: &str, runner: &Runner<'_>) -> Result<Info> {
    manage_with(root, request, operation_id, None, runner, |settings, key, switch| {
        authenticate(settings, key, switch, runner)
    })
}

pub(super) fn manage_owned(
    root: &Path,
    request: &Request,
    operation_id: &str,
    repository: &Path,
    runner: &Runner<'_>,
) -> Result<Info> {
    manage_with(
        root,
        request,
        operation_id,
        Some(repository),
        runner,
        |settings, key, switch| authenticate(settings, key, switch, runner),
    )
}

fn authenticate(settings: &Settings, key: &str, switch: bool, runner: &Runner<'_>) -> Result<publication::Account> {
    if switch {
        publication::select_account(&settings.docker_config, key, runner)
    } else {
        publication::prepare(&settings.docker_config, key, None, runner).map(|prepared| prepared.account().clone())
    }
}

fn manage_with(
    root: &Path,
    request: &Request,
    operation_id: &str,
    expected_repository: Option<&Path>,
    runner: &Runner<'_>,
    authenticate: impl FnOnce(&Settings, &str, bool) -> Result<publication::Account>,
) -> Result<Info> {
    runner.cancel.check()?;
    let directory = request
        .repository
        .to_str()
        .ok_or(Error::Invalid("Invalid repository path"))?;
    let prepared = launch::prepare_with_configuration(directory, "HEAD", launch::Configuration::WorkingCopy, runner)?;
    if let Some(repository) = expected_repository
        && prepared.repository.canonicalize()? != repository.canonicalize()?
    {
        return Err(Error::Invalid(
            "Use the checkout's Git root to manage its image destination",
        ));
    }
    let profile = prepared
        .config
        .profiles
        .get(&request.profile)
        .ok_or(Error::Invalid("Profile does not exist"))?;
    let info = inspect(root, &prepared.repository, &request.profile, profile, runner.cancel)?;
    if matches!(request.action, Action::List {}) {
        return Ok(info);
    }
    if !info.enabled {
        return Err(Error::Invalid(super::STAGED));
    }
    if profile.build.is_none() {
        return Err(Error::Invalid("Choose a build profile before changing its destination"));
    }
    let (shown, destination) = match &request.action {
        Action::List {} => return Ok(info),
        Action::Select {
            shown,
            destination: destination @ Selection::Registry { .. },
        } => (shown, Some(destination.clone())),
        Action::Select { .. } => {
            return Err(Error::Invalid(
                "Select a configured registry choice; use authenticate for a derived personal destination",
            ));
        }
        Action::Reset { shown } => (shown, None),
        Action::Authenticate { shown, switch } => {
            if shown != &info.shown_override {
                return Err(Error::Invalid(
                    "The image destination changed; list it again before signing in",
                ));
            }
            if !horizon_cloud::valid_id(operation_id) {
                return Err(Error::Invalid("Invalid authentication operation"));
            }
            let settings = setup::load_settings(root)?;
            super::check_auth_storage(&settings, &prepared.repository)?;
            // Normalize the untrusted origin and refuse ambiguity before opening sign-in.
            super::source::automatic("publisher", &prepared.repository, &request.profile, runner)?;
            // Caller-named receipts must not share a cloud's process-global Skip key.
            let key = format!("image-destination:{}", uuid::Uuid::new_v4());
            let account = authenticate(&settings, &key, *switch)?;
            let destination = personal(account, &prepared.repository, &request.profile, runner.cancel)?;
            (shown, Some(destination))
        }
    };
    setup::save_image_destination(
        root,
        &prepared.repository,
        &request.profile,
        shown.as_ref(),
        destination.as_ref(),
        runner.cancel,
    )?;
    inspect(root, &prepared.repository, &request.profile, profile, runner.cancel)
}

#[cfg(test)]
mod tests;
