//! The optional Hetzner part of the machine settings form. The token is written as a
//! private secret beside the others and never enters `settings.json`.
use super::{storage::Transaction, validate_input};
use crate::cloud_runtime::{
    Error, Result,
    settings::{self, Hetzner},
};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Server types and locations a new binding starts with: the cheapest orderable x86
/// types, in the EU locations where Hetzner is cheapest.
const DEFAULT_SERVER_TYPES: &str = "cx43, cx33, cpx42";
const DEFAULT_LOCATIONS: &str = "hel1, nbg1, fsn1";

/// Editable Hetzner settings; deliberately neither serializable nor debug-printable.
#[derive(Clone)]
pub struct Draft {
    pub enabled: bool,
    pub token: Zeroizing<String>,
    /// Comma-separated, in order of preference, such as `cx43, cpx42`.
    pub server_types: String,
    /// Comma-separated, in order of preference, such as `hel1, nbg1`.
    pub locations: String,
}

impl Draft {
    pub(super) fn from_settings(saved: Option<&Hetzner>) -> Self {
        Self {
            enabled: saved.is_some(),
            token: Zeroizing::new(String::new()),
            server_types: saved.map_or_else(
                || DEFAULT_SERVER_TYPES.to_owned(),
                |saved| saved.server_types.join(", "),
            ),
            locations: saved.map_or_else(|| DEFAULT_LOCATIONS.to_owned(), |saved| saved.locations.join(", ")),
        }
    }

    /// # Errors
    /// Requires a token, entered or saved, and server types and locations Hetzner names.
    /// `default_token` is where an entered token will be written.
    pub(super) fn validate(&self, saved: Option<&Hetzner>, default_token: &Path) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let token_file = saved.map_or_else(|| default_token.to_owned(), |saved| saved.token_file.clone());
        self.binding(saved, token_file).validate()?;
        if self.token.is_empty() {
            let saved = saved.ok_or(Error::Invalid("Enter your Hetzner Cloud API token"))?;
            settings::validate_private_key_file(&saved.token_file)?;
        } else {
            validate_input(&self.token, None)?;
            horizon_cloud::Credential::new(self.token.trim().to_owned())?;
        }
        Ok(())
    }

    /// The binding to save, writing an entered token as a private secret. `None` when
    /// Hetzner is turned off; a saved token file is then left in place, like other keys.
    pub(super) fn save(&self, saved: Option<&Hetzner>, write: &mut Transaction) -> Result<Option<Hetzner>> {
        if !self.enabled {
            return Ok(None);
        }
        let token_file = if self.token.trim().is_empty() {
            saved
                .map(|saved| saved.token_file.clone())
                .ok_or(Error::Invalid("Enter your Hetzner Cloud API token"))?
        } else {
            write.secret("hetzner", &self.token)?
        };
        Ok(Some(self.binding(saved, token_file)))
    }

    fn binding(&self, saved: Option<&Hetzner>, token_file: PathBuf) -> Hetzner {
        let list = |value: &str| -> Vec<String> {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        };
        Hetzner {
            token_file,
            server_types: list(&self.server_types),
            locations: list(&self.locations),
            // A registry pull binding is kept as it was; this form does not edit it.
            registry_pull: saved.and_then(|saved| saved.registry_pull.clone()),
        }
    }
}
