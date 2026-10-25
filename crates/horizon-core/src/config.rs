use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config_migration::{self, CURRENT_CONFIG_VERSION};
use crate::config_store::{ConfigStore, StoreFault, StoredSettings};
use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::panel::{PanelKind, PanelResume};
use crate::shortcuts::{AppShortcuts, ShortcutBinding};
pub use crate::speech_config::{SpeechBackend, SpeechConfig, SpeechHotkeyMode, SpeechProfile, SpeechTask};
use crate::ssh::{DiscoveredSshHost, SshConnection, discover_ssh_hosts};

mod browser_host;
mod presets;
mod remote_hosts;

pub use browser_host::{BrowserHostConfig, browser_host, browser_mcp_args, configure_browser_host};
pub use presets::PresetConfig;
use presets::default_presets;
pub(crate) use presets::{
    default_claude_preset, default_codex_preset, default_kilo_preset, default_opencode_preset,
    insert_missing_browser_preset, insert_missing_gemini_presets, insert_missing_grok_presets,
    insert_missing_kilo_presets, insert_missing_opencode_presets, insert_missing_pi_presets,
};
pub use remote_hosts::RemoteHostsConfig;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub shortcuts: ShortcutsConfig,
    #[serde(default)]
    pub overlays: OverlaysConfig,
    #[serde(default)]
    pub features: FeaturesConfig,
    #[serde(default)]
    pub browser: crate::browser::BrowserConfig,
    #[serde(default)]
    pub remote_hosts: RemoteHostsConfig,
    #[serde(default)]
    pub browser_host: BrowserHostConfig,
    #[serde(default = "default_presets")]
    pub presets: Vec<PresetConfig>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceConfig>,
}

/// Existing config files without a `version` field are treated as v1
/// so they enter the migration path.  `Config::default()` uses
/// `CURRENT_CONFIG_VERSION` for freshly generated configs.
fn default_version() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_CONFIG_VERSION,
            window: WindowConfig::default(),
            appearance: AppearanceConfig::default(),
            shortcuts: ShortcutsConfig::default(),
            overlays: OverlaysConfig::default(),
            features: FeaturesConfig::default(),
            browser: crate::browser::BrowserConfig::default(),
            remote_hosts: RemoteHostsConfig::default(),
            browser_host: BrowserHostConfig::default(),
            presets: default_presets(),
            workspaces: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceTheme {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: AppearanceTheme,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: AppearanceTheme::Auto,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1600.0,
            height: 1000.0,
            x: None,
            y: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ShortcutsConfig {
    #[serde(alias = "quick_nav")]
    pub command_palette: String,
    pub new_terminal: String,
    pub focus_active_workspace: String,
    pub fit_active_workspace: String,
    pub open_remote_hosts: String,
    pub toggle_sessions: String,
    pub toggle_sidebar: String,
    pub toggle_hud: String,
    pub toggle_minimap: String,
    pub align_workspaces_horizontally: String,
    pub toggle_settings: String,
    #[serde(alias = "reset_view")]
    pub zoom_reset: String,
    pub zoom_in: String,
    pub zoom_out: String,
    pub fullscreen_panel: String,
    pub exit_fullscreen_panel: String,
    pub fullscreen_window: String,
    pub save_editor: String,
    pub search: String,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            command_palette: "Ctrl+Shift+K".to_string(),
            new_terminal: "Ctrl+Shift+N".to_string(),
            focus_active_workspace: "Ctrl+Shift+W".to_string(),
            fit_active_workspace: "Ctrl+Shift+9".to_string(),
            open_remote_hosts: "Ctrl+Shift+H".to_string(),
            toggle_sessions: "Ctrl+Shift+J".to_string(),
            toggle_sidebar: "Ctrl+Shift+B".to_string(),
            toggle_hud: "Ctrl+Shift+U".to_string(),
            toggle_minimap: "Ctrl+Shift+M".to_string(),
            align_workspaces_horizontally: "Ctrl+Shift+A".to_string(),
            toggle_settings: "Ctrl+Shift+Comma".to_string(),
            zoom_reset: "Ctrl+0".to_string(),
            zoom_in: "Ctrl+Plus".to_string(),
            zoom_out: "Ctrl+Minus".to_string(),
            fullscreen_panel: "F11".to_string(),
            exit_fullscreen_panel: "Escape".to_string(),
            fullscreen_window: "Ctrl+Shift+F11".to_string(),
            save_editor: "Ctrl+Shift+S".to_string(),
            search: "Ctrl+Shift+F".to_string(),
        }
    }
}

impl ShortcutsConfig {
    /// Parse and validate the configured app shortcuts.
    ///
    /// # Errors
    ///
    /// Returns an error if any shortcut string is invalid or duplicated.
    pub fn resolve(&self) -> Result<AppShortcuts> {
        let shortcuts = AppShortcuts {
            command_palette: parse_shortcut("command_palette", &self.command_palette)?,
            new_terminal: parse_shortcut("new_terminal", &self.new_terminal)?,
            focus_active_workspace: parse_shortcut("focus_active_workspace", &self.focus_active_workspace)?,
            fit_active_workspace: parse_shortcut("fit_active_workspace", &self.fit_active_workspace)?,
            open_remote_hosts: parse_shortcut("open_remote_hosts", &self.open_remote_hosts)?,
            toggle_sessions: parse_shortcut("toggle_sessions", &self.toggle_sessions)?,
            toggle_sidebar: parse_shortcut("toggle_sidebar", &self.toggle_sidebar)?,
            toggle_hud: parse_shortcut("toggle_hud", &self.toggle_hud)?,
            toggle_minimap: parse_shortcut("toggle_minimap", &self.toggle_minimap)?,
            align_workspaces_horizontally: parse_shortcut(
                "align_workspaces_horizontally",
                &self.align_workspaces_horizontally,
            )?,
            toggle_settings: parse_shortcut("toggle_settings", &self.toggle_settings)?,
            zoom_reset: parse_shortcut("zoom_reset", &self.zoom_reset)?,
            zoom_in: parse_shortcut("zoom_in", &self.zoom_in)?,
            zoom_out: parse_shortcut("zoom_out", &self.zoom_out)?,
            fullscreen_panel: parse_shortcut("fullscreen_panel", &self.fullscreen_panel)?,
            exit_fullscreen_panel: parse_shortcut("exit_fullscreen_panel", &self.exit_fullscreen_panel)?,
            fullscreen_window: parse_shortcut("fullscreen_window", &self.fullscreen_window)?,
            save_editor: parse_shortcut("save_editor", &self.save_editor)?,
            search: parse_shortcut("search", &self.search)?,
        };

        validate_distinct_shortcuts([
            ("command_palette", shortcuts.command_palette),
            ("new_terminal", shortcuts.new_terminal),
            ("focus_active_workspace", shortcuts.focus_active_workspace),
            ("fit_active_workspace", shortcuts.fit_active_workspace),
            ("open_remote_hosts", shortcuts.open_remote_hosts),
            ("toggle_sessions", shortcuts.toggle_sessions),
            ("toggle_sidebar", shortcuts.toggle_sidebar),
            ("toggle_hud", shortcuts.toggle_hud),
            ("toggle_minimap", shortcuts.toggle_minimap),
            ("align_workspaces_horizontally", shortcuts.align_workspaces_horizontally),
            ("toggle_settings", shortcuts.toggle_settings),
            ("zoom_reset", shortcuts.zoom_reset),
            ("zoom_in", shortcuts.zoom_in),
            ("zoom_out", shortcuts.zoom_out),
            ("fullscreen_panel", shortcuts.fullscreen_panel),
            ("exit_fullscreen_panel", shortcuts.exit_fullscreen_panel),
            ("fullscreen_window", shortcuts.fullscreen_window),
            ("save_editor", shortcuts.save_editor),
            ("search", shortcuts.search),
        ])?;

        Ok(shortcuts)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OverlaysConfig {
    pub minimap_height: f32,
    pub minimap_width: f32,
}

impl Default for OverlaysConfig {
    fn default() -> Self {
        Self {
            minimap_height: 180.0,
            minimap_width: 320.0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FeaturesConfig {
    #[serde(alias = "organize_workspaces_on_startup")]
    pub organize_workspaces_on_session_load: bool,
    pub sidebar_accordion: bool,
    pub speech: SpeechConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkspaceConfig {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub position: Option<[f32; 2]>,
    #[serde(default)]
    pub terminals: Vec<TerminalConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalConfig {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default)]
    pub kind: PanelKind,
    #[serde(default)]
    pub resume: PanelResume,
    #[serde(default)]
    pub position: Option<[f32; 2]>,
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: None,
            args: Vec::new(),
            cwd: None,
            rows: default_rows(),
            cols: default_cols(),
            kind: PanelKind::default(),
            resume: PanelResume::default(),
            position: None,
            size: None,
            ssh_connection: None,
        }
    }
}

fn default_rows() -> u16 {
    24
}

fn default_cols() -> u16 {
    80
}

impl Config {
    /// Load the settings of `path`, or of the standard locations, or a default
    /// configuration.
    ///
    /// The settings live in one SQLite store next to the configuration file:
    /// `settings.sqlite` beside `config.yaml`, or `<name>.settings.sqlite`
    /// beside another name. `config.yaml` is not a live settings file.
    ///
    /// A copy that is found with no store is imported one time, and the copy is
    /// then kept as `config.yaml.imported-<time>`, with every byte that a person
    /// wrote. Horizon never writes YAML again, and it never reads the copy after
    /// that. [`Config::export`] writes a YAML copy for a person or for a tool.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read and no copy carries the
    /// settings, when a copy does not parse, or when migrated settings do not
    /// validate. A named configuration with neither a store nor a copy is an
    /// error: an isolated instance must not start with the settings of the user.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let named = path.is_some();
        let Some(resolved) = path.map(Path::to_path_buf).or_else(|| {
            config_candidates()
                .into_iter()
                .find(|candidate| settings_exist(candidate))
                .or_else(Self::default_path)
        }) else {
            tracing::info!("no settings found, using defaults");
            return Ok(Self::default());
        };

        let store = ConfigStore::resolve(&resolved);
        let mut copy = read_config_copy(&resolved)?;
        let stored = match store.open() {
            Ok(stored) => stored,
            // Another Horizon version owns this store. It may hold settings that
            // this build does not know, so nothing is written: the copy carries
            // the settings of the person for this start.
            Err(fault @ StoreFault::OtherVersion(_)) => {
                let Some(text) = copy.as_deref() else {
                    return Err(store.fault_error(&fault));
                };
                return start_from_copy_without_writing(&store, &resolved, text, &fault);
            }
            // The file is damaged. A copy, or the newest copy that was kept at an
            // earlier import, carries the settings: use it and write the store
            // again. Without one there is nothing to carry them, and defaults
            // must not replace them.
            Err(fault) => {
                if copy.is_none() {
                    copy = read_latest_backup(&resolved);
                }
                if copy.is_none() {
                    return Err(store.fault_error(&fault));
                }
                repair_damaged_store(&store, &resolved, &fault);
                None
            }
        };

        let (mut config, migrated, origin) = start_settings(stored, copy.as_deref(), named, &store, &resolved)?;
        let migrated = config_migration::migrate_in_memory(&mut config)? || migrated;
        config.validate()?;
        finish_start(&resolved, &config, copy.is_some(), migrated, origin);
        Ok(config)
    }

    /// Read the settings of `path` without writing anything, or `None` when
    /// neither the store nor a copy exists.
    ///
    /// This is the read for a caller that merges the settings and writes them
    /// itself, such as a portable profile import, and for [`Config::export`].
    /// The store comes first, because it is the authority: a copy is only the
    /// import source of a run that has no store yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the store or the copy cannot be read, when the copy
    /// does not parse, or when the settings do not validate. A damaged store
    /// falls back to the copy, and a store of another version is an error.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let store = ConfigStore::resolve(path);
        let copy = read_config_copy(path)?;
        let mut config = match store.open() {
            Ok(Some(stored)) => stored.config,
            Ok(None) => match copy {
                Some(text) => Config::deserialize_yaml(&text)?,
                None => return Ok(None),
            },
            Err(fault @ StoreFault::OtherVersion(_)) => return Err(store.fault_error(&fault)),
            Err(fault) => match copy {
                Some(text) => {
                    tracing::error!(
                        "settings store {} is damaged: {fault}; reading {} instead",
                        store.path().display(),
                        path.display()
                    );
                    Config::deserialize_yaml(&text)?
                }
                None => return Err(store.fault_error(&fault)),
            },
        };
        config_migration::migrate_in_memory(&mut config)?;
        config.validate()?;
        Ok(Some(config))
    }

    /// Save `config` to the store of `path`.
    ///
    /// This is the only write of a setting. No YAML file is written: a copy is
    /// made on request by [`Config::export`], and Horizon never reads it.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be written, and when it belongs to
    /// another Horizon version.
    pub fn save(path: &Path, config: &Config) -> Result<()> {
        let store = ConfigStore::resolve(path);
        // A store of another Horizon version may hold settings that this build
        // does not know, so a save must not replace it.
        if let Err(fault @ StoreFault::OtherVersion(_)) = store.open() {
            return Err(store.fault_error(&fault));
        }
        // The store keeps the hash of the canonical YAML of this save, so a
        // caller can tell whether a copy on disk is still the export of it.
        let mirror = sha256_hex(&config.to_yaml()?);
        store.save(config, Some(&mirror))
    }

    /// Save the YAML text that a person typed in the Settings window.
    ///
    /// The store gets the settings that the text parses to, and no file is
    /// written.
    ///
    /// # Errors
    ///
    /// Returns an error when the text does not parse or validate, or when the
    /// store cannot be written.
    pub fn save_text(path: &Path, text: &str) -> Result<()> {
        let (config, _) = import_config_copy(text)?;
        Self::save(path, &config)
    }

    /// Write the settings of `path` as YAML to `destination`, for a person or a
    /// tool that does not read the store.
    ///
    /// Horizon never reads the file that this writes. A person changes a setting
    /// in the Settings window, or in the YAML tab of that window.
    ///
    /// The settings come from the store only, never from a YAML copy, so an
    /// export can never replace a copy before the start that imports it. An
    /// existing `destination` is replaced, with the usual temp file and rename.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read, when `path` has no store,
    /// or when the file cannot be written.
    pub fn export(path: &Path, destination: &Path) -> Result<()> {
        // The store is the only source. Reading a copy here would let an export
        // to the configuration path replace the hand-written file of a person
        // with the canonical YAML, and the comments and unknown keys would be
        // lost before the start that imports them.
        let store = ConfigStore::resolve(path);
        let Some(stored) = store.open().map_err(|fault| store.fault_error(&fault))? else {
            return Err(Error::Config(format!(
                "no settings to export: {} does not exist",
                store.path().display()
            )));
        };
        let mut config = stored.config;
        config_migration::migrate_in_memory(&mut config)?;
        config.validate()?;
        let yaml = config.to_yaml()?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        crate::session_store::writer::durable_write(destination, yaml.as_bytes())
    }

    /// Parse and validate config YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization or semantic validation fails.
    pub fn from_yaml(contents: &str) -> Result<Self> {
        let config = Self::deserialize_yaml(contents)?;
        config.validate()?;
        Ok(config)
    }

    fn deserialize_yaml(contents: &str) -> Result<Self> {
        let contents = crate::retired_panel_kinds::without_retired(contents);
        serde_yaml::from_str(contents.as_ref()).map_err(|e| Error::Config(e.to_string()))
    }

    /// Serialize this config to YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::Config(e.to_string()))
    }

    /// Return the default config file path: `config.yaml` in the Horizon home, which is
    /// `HORIZON_HOME` when set, else `~/.horizon`.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        Some(HorizonHome::resolve().config_path())
    }

    #[must_use]
    pub fn resolve_path(path: Option<&Path>) -> Option<PathBuf> {
        if let Some(path) = path {
            return Some(path.to_path_buf());
        }

        config_candidates()
            .into_iter()
            .find(|candidate| settings_exist(candidate))
            .or_else(Self::default_path)
    }

    #[must_use]
    pub fn expand_tilde(s: &str) -> PathBuf {
        // Single implementation shared with speech model paths, so every
        // path-taking config field expands `~` identically.
        crate::dir_search::expand_tilde(s)
    }

    /// Validate semantic config rules that deserialization alone cannot catch.
    ///
    /// # Errors
    ///
    /// Returns an error if a configured shortcut, feature, connection or provider profile is invalid.
    pub fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.browser.quality) {
            return Err(Error::Config(format!(
                "browser.quality must be between 1 and 100 (got {})",
                self.browser.quality
            )));
        }
        if let Err(message) = self.browser.video.validate() {
            return Err(Error::Config(format!("browser.video: {message}")));
        }
        // Bindings are checked at allocation time so a missing credential is a
        // readiness state rather than a configuration that refuses to load.
        self.browser
            .remote
            .validate_definition()
            .map_err(|error| Error::Config(format!("browser.remote: {error}")))?;
        let shortcuts = self.shortcuts.resolve()?;
        crate::speech_config::validate_speech(&self.features.speech, &shortcuts)?;
        self.remote_hosts.validate()?;
        self.browser_host.validate()?;
        validate_ssh_connections(&self.presets, &self.workspaces)?;
        Ok(())
    }

    #[must_use]
    pub fn resolved_presets(&self) -> Vec<PresetConfig> {
        let mut presets = self.presets.clone();
        match discover_ssh_hosts(None) {
            Ok(discovered_hosts) => merge_discovered_ssh_presets(&mut presets, discovered_hosts),
            Err(error) => tracing::warn!(%error, "failed to discover ssh presets"),
        }
        presets
    }
}

/// Where the settings of a start come from, so the start knows what to write
/// afterwards.
#[derive(Clone, Copy)]
enum Origin {
    /// The store gave the settings.
    Stored,
    /// A copy of an earlier version gave the settings.
    Imported,
    /// Nothing gave settings.
    Default,
}

/// Use the copy for this start because the store belongs to another version.
fn start_from_copy_without_writing(
    store: &ConfigStore,
    resolved: &Path,
    text: &str,
    fault: &StoreFault,
) -> Result<Config> {
    tracing::error!(
        "settings store {} cannot be read: {fault}; starting from {} without writing anything",
        store.path().display(),
        resolved.display()
    );
    import_config_copy(text).map(|(config, _)| config)
}

/// Move a damaged store aside so it is written again. The caller has already
/// found a copy that carries the settings.
fn repair_damaged_store(store: &ConfigStore, resolved: &Path, fault: &StoreFault) {
    tracing::error!(
        "settings store {} is damaged: {fault}; reading {} instead",
        store.path().display(),
        resolved.display()
    );
    match store.move_damaged_aside() {
        Ok(kept) => tracing::error!(
            "settings store moved to {}; it is written again from {}",
            kept.display(),
            resolved.display()
        ),
        Err(error) => tracing::error!(
            "settings store {} stays damaged, because it cannot be moved: {error}",
            store.path().display()
        ),
    }
}

/// The settings of this start, the flag that says whether the copy was of an
/// older version, and where they came from.
fn start_settings(
    stored: Option<StoredSettings>,
    copy: Option<&str>,
    named: bool,
    store: &ConfigStore,
    resolved: &Path,
) -> Result<(Config, bool, Origin)> {
    match stored {
        // A store is the settings of this computer. A YAML copy next to it is
        // not read, not imported and not rewritten: it stays exactly as the
        // person left it, and the log says which file holds the settings.
        Some(stored) => {
            if copy.is_some() {
                tracing::info!(
                    "the settings of this computer are in {}; {} is not read",
                    store.path().display(),
                    resolved.display()
                );
            }
            Ok((stored.config, false, Origin::Stored))
        }
        None => match copy {
            Some(text) => {
                tracing::info!("importing {} into {}", resolved.display(), store.path().display());
                let (config, migrated) = import_config_copy(text)?;
                Ok((config, migrated, Origin::Imported))
            }
            None if named => Err(Error::Config(format!(
                "no settings store or configuration file at {}",
                resolved.display()
            ))),
            None => {
                tracing::info!("no settings found, using defaults");
                Ok((Config::default(), false, Origin::Default))
            }
        },
    }
}

/// Write what the start owes, then keep a copy that this run read under a
/// backup name so a person still finds their words.
fn finish_start(resolved: &Path, config: &Config, had_copy: bool, migrated: bool, origin: Origin) {
    let owes_store = matches!(origin, Origin::Imported) || (matches!(origin, Origin::Stored) && migrated);
    if owes_store && let Err(error) = Config::save(resolved, config) {
        // The copy is then the only place that holds settings the store does
        // not have, so it stays where it is.
        tracing::warn!(%error, "could not save the settings of {}", resolved.display());
        return;
    }
    // A copy that this start imported is kept aside, with every byte, so it
    // cannot come back as a setting. A copy that sits beside a store that was
    // not imported from it stays where the person left it.
    if had_copy && matches!(origin, Origin::Imported) {
        keep_copy_aside(resolved);
    }
}

/// Keep a copy that this run read under `config.yaml.imported-<time>`.
///
/// A rename keeps every byte: the comments, the order and the keys that this
/// build does not model survive exactly as the person wrote them.
fn keep_copy_aside(path: &Path) {
    let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup = free_backup_name(path, &format!("{name}.imported-{stamp}"));
    match std::fs::rename(path, &backup) {
        Ok(()) => tracing::info!(
            "{} is kept as {}; the settings of this computer live in the store from now on",
            path.display(),
            backup.display()
        ),
        Err(error) => tracing::warn!("could not keep {} as {}: {error}", path.display(), backup.display()),
    }
}

/// The SHA-256 of a text, as the store records the export that it describes.
fn sha256_hex(text: &str) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;

    let digest = sha2::Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A backup name that no file uses, so a second import in the same second keeps
/// both copies. A rename replaces its destination, and a kept copy must never
/// be lost.
fn free_backup_name(path: &Path, name: &str) -> PathBuf {
    let first = path.with_file_name(name);
    if !first.exists() {
        return first;
    }
    (1..1000)
        .map(|index| path.with_file_name(format!("{name}-{index}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// The newest `config.yaml.imported-<time>` copy of `path`, when one exists.
fn read_latest_backup(path: &Path) -> Option<String> {
    let name = path.file_name().and_then(std::ffi::OsStr::to_str)?;
    let prefix = format!("{name}.imported-");
    let directory = path.parent()?;
    let mut newest: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(directory).ok()?.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(stamp) = file_name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(stamp) = stamp.parse::<u64>() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(newest, _)| stamp > *newest) {
            newest = Some((stamp, entry.path()));
        }
    }
    let (_, backup) = newest?;
    match std::fs::read_to_string(&backup) {
        Ok(text) => {
            tracing::warn!(
                "the settings store is damaged; reading the copy that was kept at {}",
                backup.display()
            );
            Some(text)
        }
        Err(error) => {
            tracing::warn!(%error, "could not read {}", backup.display());
            None
        }
    }
}

/// The `config.yaml` copy that a run may import, or `None` when it is absent.
///
/// A copy is read only while this build has no store, and once more when a store
/// is damaged. [`Config::export`] writes a copy that is never read.
fn read_config_copy(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(error)),
    }
}

/// Read a configuration copy into memory, with the migrations and the checks
/// that the file itself would get. The flag says whether the text was of an
/// older version.
fn import_config_copy(text: &str) -> Result<(Config, bool)> {
    let mut config = Config::deserialize_yaml(text)?;
    let migrated = config_migration::migrate_in_memory(&mut config)?;
    config.validate()?;
    Ok((config, migrated))
}

/// Whether `path` holds settings: a store, or a copy to import.
fn settings_exist(path: &Path) -> bool {
    path.exists() || ConfigStore::resolve(path).path().exists()
}

/// Add an `SSH: <alias>` preset for each discovered host unless the config
/// already names it or already has an SSH preset for the same target. Only
/// SSH presets count for the target check: a Device preset that tunnels VNC
/// through the same host is a different panel and must not hide the terminal.
fn merge_discovered_ssh_presets(presets: &mut Vec<PresetConfig>, discovered_hosts: Vec<DiscoveredSshHost>) {
    let mut known_names: std::collections::HashSet<String> =
        presets.iter().map(|preset| preset.name.to_ascii_lowercase()).collect();
    let mut known_targets: std::collections::HashSet<String> = presets
        .iter()
        .filter(|preset| preset.kind == PanelKind::Ssh)
        .filter_map(|preset| preset.ssh_connection.as_ref())
        .map(normalized_ssh_target)
        .collect();

    for host in discovered_hosts {
        let name = format!("SSH: {}", host.alias);
        if !known_names.insert(name.to_ascii_lowercase()) {
            continue;
        }

        let target = normalized_ssh_target(&host.connection);
        if !known_targets.insert(target) {
            continue;
        }

        presets.push(PresetConfig {
            name,
            alias: None,
            kind: PanelKind::Ssh,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: Some(host.connection),
        });
    }
}

fn validate_ssh_connections(presets: &[PresetConfig], workspaces: &[WorkspaceConfig]) -> Result<()> {
    for (index, preset) in presets.iter().enumerate() {
        if let Some(connection) = &preset.ssh_connection
            && !connection.is_valid()
        {
            return Err(Error::Config(format!(
                "presets[{index}].ssh_connection.host cannot be empty"
            )));
        }
    }

    for (workspace_index, workspace) in workspaces.iter().enumerate() {
        for (terminal_index, terminal) in workspace.terminals.iter().enumerate() {
            if let Some(connection) = &terminal.ssh_connection
                && !connection.is_valid()
            {
                return Err(Error::Config(format!(
                    "workspaces[{workspace_index}].terminals[{terminal_index}].ssh_connection.host cannot be empty"
                )));
            }
        }
    }

    Ok(())
}

fn normalized_ssh_target(connection: &SshConnection) -> String {
    connection.display_label().to_ascii_lowercase()
}

fn config_candidates() -> Vec<PathBuf> {
    config_candidates_with_env(
        horizon_browser_control::paths::configured_home(),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// A `HORIZON_HOME` override is the only place a config is looked for, so an isolated
/// instance never loads or saves the user's own config.
fn config_candidates_with_env(
    horizon_home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(horizon_home) = horizon_home {
        push_config_dir_candidates(&mut paths, &horizon_home);
        return paths;
    }

    if let Some(home) = home {
        push_config_dir_candidates(&mut paths, &home.join(".horizon"));
    }

    if let Some(xdg) = xdg_config_home {
        push_config_dir_candidates(&mut paths, &xdg.join("horizon"));
    }

    paths.push(PathBuf::from("horizon.yaml"));
    paths.push(PathBuf::from("horizon.yml"));

    paths
}

fn push_config_dir_candidates(paths: &mut Vec<PathBuf>, base: &Path) {
    paths.push(base.join("config.yaml"));
    paths.push(base.join("config.yml"));
}

fn parse_shortcut(name: &str, value: &str) -> Result<ShortcutBinding> {
    ShortcutBinding::parse(value).map_err(|error| {
        Error::Config(format!(
            "invalid shortcuts.{name}: {}",
            error.to_string().trim_start_matches("Config error: ")
        ))
    })
}

fn validate_distinct_shortcuts<const N: usize>(bindings: [(&str, ShortcutBinding); N]) -> Result<()> {
    for index in 0..N {
        let (name, binding) = bindings[index];
        for (previous, previous_binding) in bindings[..index].iter().copied() {
            if binding == previous_binding {
                return Err(Error::Config(format!(
                    "duplicate shortcut `{binding}` for shortcuts.{previous} and shortcuts.{name}"
                )));
            }
            if binding.overlaps(previous_binding) {
                return Err(Error::Config(format!(
                    "shortcut `{binding}` for shortcuts.{name} conflicts with shortcuts.{previous} (`{previous_binding}`)"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod store;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Config, FeaturesConfig, PresetConfig, config_candidates_with_env, merge_discovered_ssh_presets};
    use crate::panel::PanelKind;
    use crate::ssh::{DiscoveredSshHost, SshConnection};

    #[test]
    fn a_vnc_preset_for_a_host_does_not_hide_its_discovered_ssh_preset() {
        let lab = SshConnection {
            host: "lab".into(),
            user: Some("deploy".into()),
            ..SshConnection::default()
        };
        let preset = |name: &str, kind: PanelKind, command: Option<&str>| PresetConfig {
            name: name.into(),
            alias: None,
            kind,
            command: command.map(str::to_string),
            args: Vec::new(),
            resume: crate::panel::PanelResume::Fresh,
            ssh_connection: Some(lab.clone()),
        };
        let discovered = || {
            vec![DiscoveredSshHost {
                alias: "lab".into(),
                connection: lab.clone(),
            }]
        };

        let mut presets = vec![preset("VNC: lab", PanelKind::Device, Some("127.0.0.1:5900"))];
        merge_discovered_ssh_presets(&mut presets, discovered());
        assert_eq!(
            presets.iter().map(|preset| preset.name.as_str()).collect::<Vec<_>>(),
            vec!["VNC: lab", "SSH: lab"],
            "a Device preset is a different panel from the discovered terminal"
        );

        let mut presets = vec![preset("Lab shell", PanelKind::Ssh, None)];
        merge_discovered_ssh_presets(&mut presets, discovered());
        assert_eq!(presets.len(), 1, "an SSH preset for the same target still stands in");

        let mut presets = vec![preset("ssh: LAB", PanelKind::Device, Some("127.0.0.1:5900"))];
        merge_discovered_ssh_presets(&mut presets, discovered());
        assert_eq!(
            presets.len(),
            1,
            "a preset already using the discovered name still wins"
        );
    }

    #[test]
    fn a_config_that_still_names_a_removed_panel_kind_keeps_the_rest() {
        let yaml = "\
presets:
  - name: Upkeep
    kind: upkeep
  - name: Shell
    alias: sh
    kind: shell
workspaces:
  - name: Upkeep
    terminals:
      - name: Upkeep
        kind: upkeep
  - name: Work
    terminals:
      - name: Shell
        kind: shell
";
        let config = Config::from_yaml(yaml).expect("a removed panel kind must not refuse the whole config");
        assert_eq!(
            config
                .presets
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Shell"],
            "the preset of the removed kind goes; the others stay"
        );
        assert_eq!(
            config
                .workspaces
                .iter()
                .map(|workspace| (workspace.name.as_str(), workspace.terminals.len()))
                .collect::<Vec<_>>(),
            vec![("Upkeep", 0), ("Work", 1)],
            "the terminal of the removed kind goes; the workspace keeps its other content"
        );
    }

    #[test]
    fn includes_horizon_config_candidates() {
        let temp_home = PathBuf::from("/tmp/horizon-home");
        let candidates = config_candidates_with_env(None, Some(temp_home.join(".config")), Some(temp_home));

        assert_eq!(
            candidates.first(),
            Some(&PathBuf::from("/tmp/horizon-home/.horizon/config.yaml"))
        );
        assert!(candidates.iter().any(|path| path.ends_with(".horizon/config.yml")));
        assert!(
            candidates
                .iter()
                .any(|path| path.ends_with(".config/horizon/config.yaml"))
        );
        assert!(candidates.iter().any(|path| path == &PathBuf::from("horizon.yaml")));
    }

    #[test]
    fn a_horizon_home_override_is_the_only_config_location() {
        let user_home = PathBuf::from("/tmp/horizon-user");
        let private = PathBuf::from("/tmp/horizon-private");

        let candidates =
            config_candidates_with_env(Some(private.clone()), Some(user_home.join(".config")), Some(user_home));

        assert_eq!(
            candidates,
            vec![private.join("config.yaml"), private.join("config.yml")]
        );
    }

    #[test]
    fn session_load_workspace_organization_defaults_disabled() {
        assert!(!FeaturesConfig::default().organize_workspaces_on_session_load);
        assert!(!Config::default().features.organize_workspaces_on_session_load);
    }

    #[test]
    fn sidebar_accordion_defaults_disabled() {
        assert!(!FeaturesConfig::default().sidebar_accordion);
        assert!(!Config::default().features.sidebar_accordion);
    }

    #[test]
    fn missing_sidebar_accordion_setting_keeps_it_disabled() {
        let config =
            Config::from_yaml("features:\n  speech:\n    enabled: false\n").expect("config should deserialize");

        assert!(!config.features.sidebar_accordion);
    }

    #[test]
    fn explicit_sidebar_accordion_true_is_preserved() {
        let config = Config::from_yaml("features:\n  sidebar_accordion: true\n").expect("config should deserialize");

        assert!(config.features.sidebar_accordion);
        let yaml = config.to_yaml().expect("config should serialize");
        let round_tripped = Config::from_yaml(&yaml).expect("serialized config should deserialize");
        assert!(round_tripped.features.sidebar_accordion);
    }

    #[test]
    fn default_config_includes_one_pi_preset() {
        let config = Config::default();
        let pi_presets: Vec<_> = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::Pi)
            .collect();

        assert_eq!(pi_presets.len(), 1);
        assert_eq!(pi_presets[0].name, "Pi");
        assert_eq!(pi_presets[0].alias.as_deref(), Some("pi"));
        assert_eq!(pi_presets[0].command, None);
        assert!(pi_presets[0].args.is_empty());
        assert_eq!(pi_presets[0].resume, super::PanelResume::Fresh);
    }

    #[test]
    fn missing_session_load_workspace_organization_setting_keeps_it_disabled() {
        let config =
            Config::from_yaml("features:\n  speech:\n    enabled: false\n").expect("config should deserialize");

        assert!(!config.features.organize_workspaces_on_session_load);
    }

    #[test]
    fn explicit_session_load_workspace_organization_true_is_preserved() {
        let config = Config::from_yaml("features:\n  organize_workspaces_on_session_load: true\n")
            .expect("config should deserialize");

        assert!(config.features.organize_workspaces_on_session_load);
        let yaml = config.to_yaml().expect("config should serialize");
        let round_tripped = Config::from_yaml(&yaml).expect("serialized config should deserialize");
        assert!(round_tripped.features.organize_workspaces_on_session_load);
    }

    #[test]
    fn startup_workspace_organization_key_is_accepted_as_an_alias() {
        let config = Config::from_yaml("features:\n  organize_workspaces_on_startup: true\n")
            .expect("legacy key should deserialize");

        assert!(config.features.organize_workspaces_on_session_load);
        let yaml = config.to_yaml().expect("config should serialize");
        assert!(yaml.contains("organize_workspaces_on_session_load: true"));
        assert!(!yaml.contains("organize_workspaces_on_startup:"));
    }

    #[test]
    fn appearance_defaults_to_auto_theme() {
        let config: Config = serde_yaml::from_str("{}\n").expect("config should deserialize");

        assert_eq!(config.appearance.theme, super::AppearanceTheme::Auto);
        assert_eq!(Config::default().appearance.theme, super::AppearanceTheme::Auto);
    }

    #[test]
    fn explicit_auto_theme_is_preserved() {
        let config: Config = serde_yaml::from_str("appearance:\n  theme: auto\n").expect("config should deserialize");

        assert_eq!(config.appearance.theme, super::AppearanceTheme::Auto);
    }

    #[test]
    fn explicit_light_theme_is_preserved() {
        let config: Config = serde_yaml::from_str("appearance:\n  theme: light\n").expect("config should deserialize");

        assert_eq!(config.appearance.theme, super::AppearanceTheme::Light);
    }

    #[test]
    fn retired_attention_feed_keys_are_ignored_and_dropped_on_save() {
        let config =
            Config::from_yaml("features:\n  attention_feed: false\noverlays:\n  attention_feed_width: 400.0\n")
                .expect("config should deserialize");

        let yaml = config.to_yaml().expect("config should serialize");
        assert!(!yaml.contains("attention_feed"));
    }

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        let error = Config::from_yaml("shortcuts:\n  command_palette: Ctrl+Shift+K\n  new_terminal: Ctrl+Shift+K\n")
            .expect_err("config should reject duplicate shortcuts");

        assert!(error.to_string().contains("duplicate shortcut"));
    }

    #[test]
    fn legacy_quick_nav_alias_is_accepted() {
        let config = Config::from_yaml("shortcuts:\n  quick_nav: Alt+K\n").expect("config should deserialize");

        assert_eq!(config.shortcuts.command_palette, "Alt+K");
        assert_eq!(
            config
                .shortcuts
                .resolve()
                .expect("shortcuts should resolve")
                .command_palette,
            crate::shortcuts::ShortcutBinding::parse("Alt+K").expect("shortcut should parse")
        );
    }

    #[test]
    fn legacy_reset_view_alias_is_accepted() {
        let config = Config::from_yaml("shortcuts:\n  reset_view: Alt+0\n").expect("config should deserialize");

        assert_eq!(config.shortcuts.zoom_reset, "Alt+0");
        assert_eq!(
            config.shortcuts.resolve().expect("shortcuts should resolve").zoom_reset,
            crate::shortcuts::ShortcutBinding::parse("Alt+0").expect("shortcut should parse")
        );
    }

    #[test]
    fn workspace_navigation_shortcuts_resolve() {
        let config = Config::from_yaml(
            "shortcuts:\n  focus_active_workspace: Alt+W\n  fit_active_workspace: Alt+9\n  toggle_sessions: Alt+J\n",
        )
        .expect("config should deserialize");

        let shortcuts = config.shortcuts.resolve().expect("shortcuts should resolve");

        assert_eq!(
            shortcuts.focus_active_workspace,
            crate::shortcuts::ShortcutBinding::parse("Alt+W").expect("shortcut should parse")
        );
        assert_eq!(
            shortcuts.fit_active_workspace,
            crate::shortcuts::ShortcutBinding::parse("Alt+9").expect("shortcut should parse")
        );
        assert_eq!(
            shortcuts.toggle_sessions,
            crate::shortcuts::ShortcutBinding::parse("Alt+J").expect("shortcut should parse")
        );
    }

    #[test]
    fn overlapping_shortcuts_are_rejected() {
        let error = Config::from_yaml("shortcuts:\n  toggle_sidebar: Ctrl+K\n  command_palette: Ctrl+Shift+K\n")
            .expect_err("config should reject overlapping shortcuts");

        assert!(error.to_string().contains("conflicts with"));
        assert!(error.to_string().contains("toggle_sidebar"));
    }

    #[test]
    fn chromium_screencast_quality_must_fit_protocol_range() {
        let error = Config::from_yaml("browser:\n  quality: 101\n").expect_err("quality must be rejected");
        assert!(error.to_string().contains("browser.quality must be between 1 and 100"));

        Config::from_yaml("browser:\n  quality: 1\n").expect("minimum quality should be accepted");
        Config::from_yaml("browser:\n  quality: 100\n").expect("maximum quality should be accepted");
    }

    #[test]
    fn firefox_system_access_stays_off_unless_the_config_sets_it() {
        let off = Config::from_yaml("browser:\n  backend: firefox\n").expect("firefox section loads");
        assert!(!off.browser.firefox_system_access);
        let on = Config::from_yaml("browser:\n  backend: firefox\n  firefox_system_access: true\n")
            .expect("explicit system access loads");
        assert!(on.browser.firefox_system_access);
        assert_eq!(on.browser.backend, crate::browser::BackendKind::FirefoxBidi);
    }

    #[test]
    fn browser_video_options_must_fit_engine_range() {
        let error = Config::from_yaml("browser:\n  video:\n    fps: 0\n").expect_err("fps must be rejected");
        assert!(error.to_string().contains("browser.video"));
        let auto = Config::from_yaml("browser:\n  video:\n    quality: 90\n    compression_level: 4\n    fps: 10\n")
            .expect("omitted max_width should be accepted as auto viewport width");
        assert!(auto.browser.video.max_width.is_none());
        let explicit = Config::from_yaml("browser:\n  video:\n    quality: 90\n    fps: 10\n    max_width: 1280\n")
            .expect("explicit max_width should still be accepted");
        assert_eq!(explicit.browser.video.max_width, Some(1280));
        let too_wide = Config::from_yaml("browser:\n  video:\n    max_width: 4096\n")
            .expect_err("out-of-range max_width must be rejected");
        assert!(too_wide.to_string().contains("browser.video"));
    }

    #[test]
    fn browser_remote_section_round_trips_without_credential_values() {
        let yaml = r#"
browser:
  remote:
    providers:
      device_cloud:
        adapter: webdriver
        endpoint: https://grid.example.net/wd/hub
        authentication:
          kind: basic
          username_ref: device-cloud-user
          password_ref: device-cloud-key
        credential_bindings:
          device-cloud-user:
            store: os_keychain
            slot: remote-browser/device-cloud/username
          device-cloud-key:
            store: session
        limits:
          max_sessions: 1
          allocation_timeout_seconds: 120
          idle_release_seconds: 180
          max_session_seconds: 1800
    targets:
      ios_phone:
        provider: device_cloud
        browser_name: safari
        platform_name: iOS
        device:
          kind: physical
          model: iPhone 16
          os_version: "18"
        capability_extensions:
          appium:automationName: XCUITest
"#;
        let config = Config::from_yaml(yaml).expect("remote section parses");
        assert_eq!(
            config.browser.backend,
            crate::browser::BackendKind::ChromiumCdp,
            "local defaults are untouched"
        );
        let target = config.browser.remote.targets.get("ios_phone").expect("target");
        assert_eq!(target.provider, "device_cloud");
        let encoded = config.to_yaml().expect("serializes");
        let decoded = Config::from_yaml(&encoded).expect("re-parses");
        assert_eq!(decoded.browser.remote, config.browser.remote);
        assert!(!encoded.contains("password:"), "no credential value field exists");
    }

    #[test]
    fn browser_remote_section_is_absent_by_default_and_validated_on_load() {
        let encoded = Config::default().to_yaml().expect("serializes");
        assert!(!encoded.contains("remote:"), "{encoded}");
        let legacy = Config::from_yaml("browser:\n  backend: firefox\n").expect("legacy browser section loads");
        assert!(legacy.browser.remote.is_empty());
        let unbound = Config::from_yaml(
            "browser:\n  remote:\n    providers:\n      grid:\n        endpoint: https://grid.example.net\n        authentication:\n          kind: bearer\n          token_ref: grid-token\n",
        )
        .expect("an unbound reference still loads; readiness reports it");
        let presence = unbound.browser.remote.binding_presence();
        assert_eq!(presence["grid"].values().filter(|present| !**present).count(), 1);
        let error = Config::from_yaml(
            "browser:\n  remote:\n    providers:\n      grid:\n        endpoint: https://grid.example.net\n        authentication:\n          kind: bearer\n          token_ref: grid-token\n        credential_bindings:\n          grid-token:\n            store: os_keychain\n",
        )
        .expect_err("a present binding without a slot is malformed configuration");
        assert!(error.to_string().contains("needs a slot"), "{error}");
        let error = Config::from_yaml(
            "browser:\n  remote:\n    providers:\n      grid:\n        endpoint: https://grid.example.net\n    targets:\n      phone:\n        provider: nowhere\n        browser_name: safari\n        platform_name: iOS\n",
        )
        .expect_err("unknown provider is rejected");
        let message = error.to_string();
        assert!(message.contains("browser.remote"), "{message}");
        assert!(message.contains("nowhere"), "{message}");
        let error = Config::from_yaml(
            "browser:\n  remote:\n    providers:\n      grid:\n        endpoint: https://me:pw@grid.example.net\n",
        )
        .expect_err("userinfo is rejected");
        assert!(!error.to_string().contains("pw@"), "{error}");
    }

    #[test]
    fn preset_ssh_connection_round_trips_from_yaml() {
        let config = Config::from_yaml(
            r"
presets:
  - name: prod-api
    kind: ssh
    ssh_connection:
      host: prod-api
      user: deploy
      port: 2222
",
        )
        .expect("config should deserialize");

        let preset = config.presets.first().expect("ssh preset");
        assert_eq!(preset.kind, PanelKind::Ssh);
        assert_eq!(
            preset.ssh_connection.as_ref().map(|conn| conn.host.as_str()),
            Some("prod-api")
        );
        assert_eq!(
            preset.ssh_connection.as_ref().and_then(|conn| conn.user.as_deref()),
            Some("deploy")
        );
        assert_eq!(preset.ssh_connection.as_ref().and_then(|conn| conn.port), Some(2222));
    }

    #[test]
    fn ssh_presets_skip_workspace_directory_prompt() {
        let preset = PresetConfig {
            name: "prod-api".to_string(),
            alias: None,
            kind: PanelKind::Ssh,
            command: None,
            args: Vec::new(),
            resume: super::PanelResume::Fresh,
            ssh_connection: None,
        };

        assert!(!preset.requires_workspace_cwd());
    }
}
