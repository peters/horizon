//! Availability snapshot for form hints, loaded with the draft off the render thread.
use super::settings::{self, Settings};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Default)]
pub struct SavedCredentials(BTreeSet<PathBuf>);

impl SavedCredentials {
    pub(super) fn load(settings: &Settings) -> Self {
        let providers = std::iter::once(&settings.runpod_key_file)
            .chain(settings.hetzner.iter().map(|provider| &provider.token_file));
        let agents = settings
            .openai_api_key_file
            .iter()
            .chain(settings.anthropic_api_key_file.iter());
        let registries = settings
            .registries
            .iter()
            .flat_map(|config| &config.bindings)
            .flat_map(|binding| {
                std::iter::once(&binding.pull.secret_file).chain(binding.publish.iter().map(|auth| &auth.secret_file))
            });
        Self(
            providers
                .chain(agents)
                .chain(registries)
                .filter(|path| settings::validate_private_key_file(path).is_ok())
                .cloned()
                .collect(),
        )
    }

    /// Availability when the draft was loaded; saving still validates the live file.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.0.contains(path)
    }
}
