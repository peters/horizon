//! Host-selected publishing intent. Repository metadata can name an image, never a credential.
pub(in crate::cloud_runtime) const STAGED: &str = "Portable image publishing is not enabled for this legacy request; use the current publishing flow or an explicit image";
pub mod management;
pub mod operations;
mod preparation;
mod publication;
mod selection;
mod source;
#[cfg(test)]
mod tests;

use super::{Error, Result, github::publish::publication::Account, registry, settings::Settings};
use horizon_cloud::Profile;
use horizon_cloud::image_publishing::{Destination, Publisher};
pub(in crate::cloud_runtime) use preparation::{authorize, bind_initial};
pub use preparation::{check_auth_storage, prepare, preview};
pub use publication::Publication;
pub use selection::{Selection, choices, personal, selection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A machine-local override retaining its personal account or exact registry publication grant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Override {
    pub repository: PathBuf,
    pub profile: String,
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

impl Override {
    /// # Errors
    /// Rejects non-local scopes and destinations that have no exact publishing grant.
    pub fn validate(&self, settings: &Settings) -> Result<()> {
        self.selection(settings).map(drop)
    }

    /// # Errors
    /// A malformed recorded account, another namespace or a missing publishing grant.
    pub fn selection(&self, settings: &Settings) -> Result<Selection> {
        self.validate_scope()?;
        let selection = match &self.account {
            Some(account) => Selection::Personal {
                destination: Destination {
                    repository: self.destination.clone(),
                    publisher: Publisher::GitHub {
                        login: account.login.clone(),
                    },
                },
                account: account.clone(),
            },
            None => Selection::Registry {
                destination: Destination::bound(
                    self.destination.clone(),
                    self.binding.clone().ok_or(Error::Invalid(
                        "The local destination has no recorded publication grant",
                    ))?,
                )
                .map_err(|_| Error::Invalid("Invalid local image destination"))?,
            },
        };
        selection.check_binding(settings)?;
        Ok(selection)
    }

    pub(super) fn validate_scope(&self) -> Result<()> {
        if self.account.is_some() == self.binding.is_some()
            || self
                .binding
                .as_ref()
                .is_some_and(|binding| !horizon_cloud::valid_id(binding))
            || !self.repository.is_absolute()
            || !horizon_cloud::valid_id(&self.profile)
            || !horizon_cloud::image_publishing::valid_repository(&self.destination)
        {
            return Err(Error::Invalid(
                "An image destination needs a local repository and profile",
            ));
        }
        Ok(())
    }
}

fn selected<'a>(settings: &'a Settings, repository: &Path, profile: &str) -> Option<&'a Override> {
    settings
        .image_publishing
        .iter()
        .find(|choice| choice.repository == repository && choice.profile == profile)
}

/// Restores the recorded target for automatic or original recipe image requests.
pub(super) fn requested_profile(state: &super::state::Deployment, requested: &Profile) -> Profile {
    let mut profile = requested.clone();
    if requested.automatic_image()
        || state
            .publication
            .as_ref()
            .is_some_and(|record| requested.image == record.requested_image)
    {
        profile.image.clone_from(&state.profile.image);
    }
    profile
}

fn grant<'a>(settings: &'a Settings, destination: &str) -> Result<&'a registry::Binding> {
    settings
        .registries
        .as_ref()
        .and_then(|config| {
            config
                .bindings
                .iter()
                .find(|binding| binding.repository == destination && binding.publish.is_some())
        })
        .ok_or(Error::Invalid(
            "Add separate publishing and read-only pull credentials for this destination in Container registry",
        ))
}

pub(in crate::cloud_runtime) fn grant_id(binding: &registry::Binding) -> Result<String> {
    use std::fmt::Write as _;
    let auth = binding
        .publish
        .as_ref()
        .ok_or(Error::Invalid("The image destination has no publishing credential"))?;
    // Pull rotation does not change the publishing account. Credential files are immutable
    // grants; an intentional change uses a new file, not another account in the same file.
    let bytes = serde_json::to_vec(&(&auth.username, &auth.secret_file)).map_err(|_| Error::Json)?;
    let hash = ring::digest::digest(&ring::digest::SHA256, &bytes);
    Ok(hash.as_ref().iter().fold(String::with_capacity(64), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    }))
}
