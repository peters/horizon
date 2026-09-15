//! Portable remote browser profiles: the shareable half of `browser.remote`.
//!
//! A portable profile is the document a user hands to a second computer:
//! providers, targets, limits and authentication references, never a
//! credential binding or value. Import merges it through
//! [`RemoteBrowserConfig::import_portable`], so a shared file can add or
//! update definitions but never redirect a trusted endpoint or carry a
//! binding; the second computer enters its own credentials afterwards, and
//! the readiness of each reference stays a live query on that machine.

use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use horizon_browser::remote::{ImportSummary, RemoteBrowserConfig, RemoteConfigError};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::config_migration;

/// File name suggested next to the configuration file.
pub const PORTABLE_PROFILE_FILE_NAME: &str = "remote-browser-profile.yaml";
/// Largest profile document that is read. A profile is a few kilobytes; the
/// bound keeps a mistaken path from pulling an arbitrary file into memory.
pub const MAX_PORTABLE_PROFILE_BYTES: u64 = 256 * 1024;
/// Format marker at the top of the document.
pub const PORTABLE_PROFILE_FORMAT: u32 = 1;

/// The document on disk: a format marker and the portable definition. The
/// definition is nested rather than flattened so both layers can reject
/// unknown keys.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PortableProfileDocument {
    horizon_remote_browser_profile: u32,
    remote: RemoteBrowserConfig,
}

/// Why a profile could not be exported or imported. Never carries a
/// credential value: the document has none, and configuration errors name
/// providers, targets and references only.
#[derive(Debug)]
pub enum RemoteProfileError {
    /// The local configuration has no remote providers or targets.
    NothingToExport,
    /// The document's format marker is not one this build reads.
    UnsupportedFormat { found: u32 },
    /// The file is larger than [`MAX_PORTABLE_PROFILE_BYTES`].
    TooLarge { bytes: u64 },
    /// Reading or writing a file failed.
    Io { path: PathBuf, source: io::Error },
    /// The document is not a portable profile.
    Parse(String),
    /// The definition failed validation or conflicts with the local one.
    Config(RemoteConfigError),
    /// The configuration file could not be loaded or serialised.
    ConfigFile(String),
    /// The profile path is the configuration file itself.
    IsConfigPath { path: PathBuf },
}

impl fmt::Display for RemoteProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingToExport => write!(formatter, "no remote providers or targets are configured"),
            Self::UnsupportedFormat { found } => write!(
                formatter,
                "portable profile format {found} is not supported; this build reads format {PORTABLE_PROFILE_FORMAT}"
            ),
            Self::TooLarge { bytes } => write!(
                formatter,
                "portable profile is {bytes} bytes; the limit is {MAX_PORTABLE_PROFILE_BYTES}"
            ),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Parse(message) => write!(formatter, "not a portable profile: {message}"),
            Self::Config(error) => write!(formatter, "{error}"),
            Self::ConfigFile(message) => write!(formatter, "configuration file: {message}"),
            Self::IsConfigPath { path } => write!(
                formatter,
                "{} is the configuration file; a portable profile is a separate document",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RemoteProfileError {}

impl From<RemoteConfigError> for RemoteProfileError {
    fn from(error: RemoteConfigError) -> Self {
        Self::Config(error)
    }
}

/// The portable document for the local definition, bindings stripped.
///
/// # Errors
/// [`RemoteProfileError::NothingToExport`] when nothing remote is configured.
pub fn export_portable(remote: &RemoteBrowserConfig) -> Result<String, RemoteProfileError> {
    if remote.is_empty() {
        return Err(RemoteProfileError::NothingToExport);
    }
    let document = PortableProfileDocument {
        horizon_remote_browser_profile: PORTABLE_PROFILE_FORMAT,
        remote: remote.export_portable(),
    };
    serde_yaml::to_string(&document).map_err(|error| RemoteProfileError::Parse(error.to_string()))
}

/// The definition carried by a portable document.
///
/// # Errors
/// A document that is not a portable profile, or one in another format.
pub fn parse_portable(yaml: &str) -> Result<RemoteBrowserConfig, RemoteProfileError> {
    if yaml.len() as u64 > MAX_PORTABLE_PROFILE_BYTES {
        return Err(RemoteProfileError::TooLarge {
            bytes: yaml.len() as u64,
        });
    }
    let document: PortableProfileDocument =
        serde_yaml::from_str(yaml).map_err(|error| RemoteProfileError::Parse(error.to_string()))?;
    if document.horizon_remote_browser_profile != PORTABLE_PROFILE_FORMAT {
        return Err(RemoteProfileError::UnsupportedFormat {
            found: document.horizon_remote_browser_profile,
        });
    }
    Ok(document.remote)
}

/// Merge a portable document into the local definition. Nothing changes on
/// any error.
///
/// # Errors
/// Parse failures, an unsupported format, a document that carries bindings,
/// or an endpoint or authentication conflict with a local provider.
pub fn import_portable(remote: &mut RemoteBrowserConfig, yaml: &str) -> Result<ImportSummary, RemoteProfileError> {
    let incoming = parse_portable(yaml)?;
    Ok(remote.import_portable(&incoming)?)
}

/// Read a portable document from `path`. At most one byte past
/// [`MAX_PORTABLE_PROFILE_BYTES`] is ever read, whatever the file reports
/// or grows to.
///
/// # Errors
/// I/O failures, an oversized file, and a file that is not UTF-8.
pub fn read_portable_profile(path: &Path) -> Result<String, RemoteProfileError> {
    let io_error = |source| RemoteProfileError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = std::fs::File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_PORTABLE_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_PORTABLE_PROFILE_BYTES {
        return Err(RemoteProfileError::TooLarge {
            bytes: bytes.len() as u64,
        });
    }
    String::from_utf8(bytes).map_err(|_| RemoteProfileError::Parse("not UTF-8".to_string()))
}

/// Refuse a profile path that names the configuration file itself: an
/// export would replace the configuration with a profile document.
///
/// # Errors
/// [`RemoteProfileError::IsConfigPath`] when both name the same file.
pub fn refuse_config_path(config_path: &Path, profile_path: &Path) -> Result<(), RemoteProfileError> {
    if normalized(config_path) == normalized(profile_path) {
        return Err(RemoteProfileError::IsConfigPath {
            path: profile_path.to_path_buf(),
        });
    }
    Ok(())
}

/// One spelling for a path that may not exist yet: the longest existing
/// prefix is canonicalised (symlinks and, where the file system keeps it,
/// case resolved), and the remaining components are normalised lexically,
/// so `dir/./a`, `dir/sub/../a` and `dir/a` compare equal.
fn normalized(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let components: Vec<Component<'_>> = absolute.components().collect();
    let mut existing = components.len();
    let mut base = None;
    while existing > 0 {
        let prefix: PathBuf = components[..existing].iter().collect();
        if let Ok(canonical) = prefix.canonicalize() {
            base = Some(canonical);
            break;
        }
        existing -= 1;
    }
    let mut result = base.unwrap_or_default();
    for component in &components[existing..] {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// The configuration file parsed, migrated and validated in memory: nothing
/// is written back, so a caller can fail after this without changing the
/// file.
fn load_config_in_memory(config_path: &Path) -> Result<Config, RemoteProfileError> {
    let config_error = |error: crate::error::Error| RemoteProfileError::ConfigFile(error.to_string());
    let contents = std::fs::read_to_string(config_path).map_err(|source| RemoteProfileError::Io {
        path: config_path.to_path_buf(),
        source,
    })?;
    let mut config: Config =
        serde_yaml::from_str(&contents).map_err(|error| RemoteProfileError::ConfigFile(error.to_string()))?;
    config_migration::migrate_in_memory(&mut config).map_err(config_error)?;
    config.validate().map_err(config_error)?;
    Ok(config)
}

/// Write the portable document for `remote` to `path`, replacing the file
/// atomically.
///
/// # Errors
/// Nothing to export, or an I/O failure.
pub fn write_portable_profile(path: &Path, remote: &RemoteBrowserConfig) -> Result<(), RemoteProfileError> {
    let document = export_portable(remote)?;
    write_atomically(path, &document)
}

/// Merge the portable document at `profile_path` into the configuration file
/// at `config_path`, creating the file from defaults when it does not exist.
/// The file is rewritten only after the merged configuration validates.
///
/// # Errors
/// Any read, parse, merge, validation or write failure; the configuration
/// file is untouched on every error.
pub fn import_portable_file_into_config(
    config_path: &Path,
    profile_path: &Path,
) -> Result<ImportSummary, RemoteProfileError> {
    refuse_config_path(config_path, profile_path)?;
    let yaml = read_portable_profile(profile_path)?;
    let mut config = if config_path.exists() {
        load_config_in_memory(config_path)?
    } else {
        Config::default()
    };
    let summary = import_portable(&mut config.browser.remote, &yaml)?;
    let serialised = config
        .to_yaml()
        .map_err(|error| RemoteProfileError::ConfigFile(error.to_string()))?;
    Config::from_yaml(&serialised).map_err(|error| RemoteProfileError::ConfigFile(error.to_string()))?;
    write_atomically(config_path, &serialised)?;
    Ok(summary)
}

/// Write the portable document for the configuration file at `config_path`
/// to `profile_path`.
///
/// # Errors
/// A configuration that cannot be loaded, nothing to export, or an I/O
/// failure.
pub fn export_portable_file_from_config(
    config_path: &Path,
    profile_path: &Path,
) -> Result<RemoteBrowserConfig, RemoteProfileError> {
    refuse_config_path(config_path, profile_path)?;
    let config = load_config_in_memory(config_path)?;
    write_portable_profile(profile_path, &config.browser.remote)?;
    Ok(config.browser.remote.export_portable())
}

/// One line describing an import, for a status row or a log.
#[must_use]
pub fn summary_line(summary: &ImportSummary) -> String {
    format!(
        "added {} provider(s) and {} target(s), updated {} provider(s) and {} target(s)",
        summary.providers_added.len(),
        summary.targets_added.len(),
        summary.providers_updated.len(),
        summary.targets_updated.len()
    )
}

/// Stage in a uniquely named file created exclusively in the destination
/// directory (never through a pre-existing name or symlink), then persist it
/// over the destination.
fn write_atomically(path: &Path, contents: &str) -> Result<(), RemoteProfileError> {
    let io_error = |source| RemoteProfileError::Io {
        path: path.to_path_buf(),
        source,
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(io_error)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".remote-profile-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(io_error)?;
    staged.write_all(contents.as_bytes()).map_err(io_error)?;
    staged.flush().map_err(io_error)?;
    staged.persist(path).map_err(|error| io_error(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests;
