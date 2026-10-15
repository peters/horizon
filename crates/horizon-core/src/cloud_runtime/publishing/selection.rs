//! The nonsecret destination shown before Start and checked again by launch.
use super::{Account, Destination, Error, Publisher, Result, grant, grant_id, selected};
use crate::cloud_runtime::{command::Runner, github::publish, settings::Settings};
use horizon_cloud::{Cancellation, Profile};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    Recipe {
        image: String,
    },
    Automatic {
        login: Option<String>,
        repository: Option<String>,
    },
    Registry {
        destination: Destination,
    },
    Personal {
        destination: Destination,
        account: Account,
    },
}

impl Selection {
    #[must_use]
    pub fn repository(&self) -> Option<&str> {
        match self {
            Self::Recipe { image } => Some(image),
            Self::Automatic { repository, .. } => repository.as_deref(),
            Self::Registry { destination } | Self::Personal { destination, .. } => Some(&destination.repository),
        }
    }

    /// Confirms a new launch still uses the destination and account shown to its caller.
    /// Existing deployment records retain their own publication instead.
    /// # Errors
    /// Changed local intent, an invalid grant, cancelled lookup or repository identity failure.
    pub fn confirm_current(
        &self,
        settings: &Settings,
        repository: &Path,
        name: &str,
        profile: &Profile,
        cancel: &Cancellation,
    ) -> Result<()> {
        if selection(settings, repository, name, profile, cancel)? != *self {
            return Err(Error::Invalid(
                "The image destination or publishing account changed since it was shown; review it before starting",
            ));
        }
        Ok(())
    }

    /// # Errors
    /// A stale publishing grant or malformed bound destination.
    pub fn check_binding(&self, settings: &Settings) -> Result<()> {
        if let Self::Registry { destination } = self {
            destination
                .validate()
                .map_err(|_| Error::Invalid("Invalid selected image destination"))?;
            let Publisher::Registry { binding } = &destination.publisher else {
                return Err(Error::Invalid(
                    "A local destination needs its configured registry binding",
                ));
            };
            if grant_id(grant(settings, &destination.repository)?)? != *binding {
                return Err(Error::Invalid(
                    "The publishing grant changed; choose this destination again before saving",
                ));
            }
        }
        if let Self::Personal { destination, account } = self {
            account.check(account)?;
            destination
                .validate()
                .map_err(|_| Error::Invalid("Invalid personal image destination"))?;
            if destination.publisher
                != (Publisher::GitHub {
                    login: account.login.clone(),
                })
            {
                return Err(Error::Invalid(
                    "The personal image destination belongs to another account",
                ));
            }
        }
        Ok(())
    }
}

/// Reads local metadata and a credential-free repository identity. Run on a worker thread.
/// # Errors
/// An invalid profile scope, missing publishing grant or repository identity failure.
pub fn selection(
    settings: &Settings,
    repository: &Path,
    name: &str,
    profile: &Profile,
    cancel: &Cancellation,
) -> Result<Selection> {
    cancel.check()?;
    if !horizon_cloud::valid_id(name) {
        return Err(Error::Invalid(
            "Choose a named cloud profile before selecting its image destination",
        ));
    }
    if profile.build.is_none() {
        return Ok(Selection::Recipe {
            image: profile.image.clone(),
        });
    }
    let repository = repository.canonicalize()?;
    if let Some(choice) = selected(settings, &repository, name) {
        return choice.selection(settings);
    }
    if !profile.image.is_empty() {
        return Ok(Selection::Recipe {
            image: profile.image.clone(),
        });
    }
    let login = publish::publisher(&settings.docker_config);
    let target = login
        .as_deref()
        .map(|login| {
            let runner = Runner {
                cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            super::source::automatic(login, &repository, name, &runner).map(|destination| destination.repository)
        })
        .transpose()?;
    Ok(Selection::Automatic {
        login,
        repository: target,
    })
}

/// Chooses an automatic package for an account authenticated by the host.
/// # Errors
/// A malformed account, invalid profile scope, cancelled lookup or repository identity error.
pub fn personal(mut account: Account, repository: &Path, name: &str, cancel: &Cancellation) -> Result<Selection> {
    account.check(&account)?;
    account.login.make_ascii_lowercase();
    let repository = repository.canonicalize()?;
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let destination = super::source::automatic(&account.login, &repository, name, &runner)?;
    Ok(Selection::Personal { destination, account })
}

/// Existing authorized registry destinations. No credential is read or copied.
/// # Errors
/// Invalid configured grants or ambiguous registry scopes.
pub fn choices(settings: &Settings) -> Result<Vec<Selection>> {
    let Some(config) = &settings.registries else {
        return Ok(Vec::new());
    };
    config.validate()?;
    config
        .bindings
        .iter()
        .filter(|binding| binding.publish.is_some())
        .map(|binding| {
            let destination = Destination::bound(binding.repository.clone(), grant_id(binding)?)
                .map_err(|_| Error::Invalid("Invalid configured publishing destination"))?;
            Ok(Selection::Registry { destination })
        })
        .collect()
}
