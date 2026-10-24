//! The settings that the native App Automate modes read.
//!
//! These modes run without a window, so they have no [`horizon_core::Config`] in
//! memory. They read the settings of the machine instead: the store first, and a
//! YAML copy only when no start has imported it yet.

use std::path::Path;

use horizon_core::Config;
use horizon_core::browser::remote::RemoteProviderProfile;

use crate::Error;

/// The settings that the store at `path` holds, else the copy that no start has
/// imported yet.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when there are no settings or when the store
/// cannot be read.
pub(crate) fn settings_at(path: &Path) -> Result<Config, Error> {
    Config::read(path)
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Unavailable)
}

/// The remote-browser provider profile `provider` of the settings at `path`.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when there are no settings or when they have
/// no such provider.
pub(crate) fn provider_at(path: &Path, provider: &str) -> Result<RemoteProviderProfile, Error> {
    let config = settings_at(path)?;
    config
        .browser
        .remote
        .providers
        .get(provider)
        .cloned()
        .ok_or(Error::Unavailable)
}

/// The settings of this machine, from the path that this run uses.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when this machine has no configuration path.
pub(crate) fn settings_path() -> Result<std::path::PathBuf, Error> {
    Config::resolve_path(None).ok_or(Error::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::config_store::ConfigStore;

    fn home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("native-settings-")
            .tempdir()
            .expect("temp dir")
    }

    fn provider_yaml(provider: &str) -> String {
        format!(
            "browser:\n  remote:\n    providers:\n      {provider}:\n        endpoint: https://{provider}.example.net\n"
        )
    }

    fn write_copy(root: &Path, provider: &str) -> std::path::PathBuf {
        let path = root.join("config.yaml");
        std::fs::write(&path, provider_yaml(provider)).expect("write the copy");
        path
    }

    #[test]
    fn a_store_alone_reaches_the_provider() {
        let root = home();
        let path = root.path().join("config.yaml");
        Config::save_text(&path, &provider_yaml("browserstack")).expect("import");
        assert!(!path.exists(), "a text save writes the store and no copy");

        let profile = provider_at(&path, "browserstack").expect("the store answers without a copy");

        assert_eq!(
            profile.endpoint.as_str().trim_end_matches('/'),
            "https://browserstack.example.net"
        );
        assert!(ConfigStore::resolve(&path).path().exists());
    }

    #[test]
    fn a_copy_before_the_first_start_answers() {
        let root = home();
        let path = write_copy(root.path(), "grid");

        let profile = provider_at(&path, "grid").expect("the copy answers before the import");

        assert_eq!(
            profile.endpoint.as_str().trim_end_matches('/'),
            "https://grid.example.net"
        );
    }

    #[test]
    fn a_missing_provider_is_unavailable() {
        let root = home();
        let path = root.path().join("config.yaml");
        Config::save_text(&path, &provider_yaml("grid")).expect("import");

        assert!(matches!(provider_at(&path, "browserstack"), Err(Error::Unavailable)));
    }
}
