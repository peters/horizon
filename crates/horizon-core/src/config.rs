use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config_migration::{self, CURRENT_CONFIG_VERSION};
use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::shortcuts::{AppShortcuts, ShortcutBinding};
use crate::ssh::{SshConnection, discover_ssh_hosts};

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
pub struct PresetConfig {
    pub name: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub kind: PanelKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub resume: PanelResume,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
}

impl PresetConfig {
    /// Convert this preset into `PanelOptions` for panel creation.
    #[must_use]
    pub fn to_panel_options(&self) -> PanelOptions {
        PanelOptions {
            name: Some(self.name.clone()),
            command: self.command.clone(),
            args: self.args.clone(),
            ssh_connection: self.ssh_connection.clone(),
            kind: self.kind,
            resume: self.resume.clone(),
            ..PanelOptions::default()
        }
    }

    #[must_use]
    pub fn requires_workspace_cwd(&self) -> bool {
        !matches!(self.kind, PanelKind::Ssh)
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

pub(crate) fn default_opencode_presets() -> [PresetConfig; 2] {
    [
        PresetConfig {
            name: "OpenCode".to_string(),
            alias: Some("oc".to_string()),
            kind: PanelKind::OpenCode,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Last,
            ssh_connection: None,
        },
        PresetConfig {
            name: "OpenCode (Fresh)".to_string(),
            alias: Some("ocf".to_string()),
            kind: PanelKind::OpenCode,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
    ]
}

pub(crate) fn default_gemini_presets() -> [PresetConfig; 1] {
    [PresetConfig {
        name: "Gemini CLI".to_string(),
        alias: Some("gm".to_string()),
        kind: PanelKind::Gemini,
        command: None,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }]
}

pub(crate) fn default_kilo_presets() -> [PresetConfig; 2] {
    [
        PresetConfig {
            name: "KiloCode".to_string(),
            alias: Some("kc".to_string()),
            kind: PanelKind::KiloCode,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Last,
            ssh_connection: None,
        },
        PresetConfig {
            name: "KiloCode (Fresh)".to_string(),
            alias: Some("kcf".to_string()),
            kind: PanelKind::KiloCode,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
    ]
}

fn insert_missing_agent_presets(presets: &mut Vec<PresetConfig>, defaults: impl IntoIterator<Item = PresetConfig>) {
    for default_preset in defaults {
        let expected_name = default_preset.name.to_ascii_lowercase();
        let expected_alias = default_preset.alias.as_deref().map(str::to_ascii_lowercase);
        let exists = presets.iter().any(|preset| {
            preset.name.eq_ignore_ascii_case(&default_preset.name)
                || preset
                    .alias
                    .as_deref()
                    .zip(expected_alias.as_deref())
                    .is_some_and(|(alias, expected)| alias.eq_ignore_ascii_case(expected))
                || (preset.kind == default_preset.kind && preset.resume == default_preset.resume)
                || preset.name.to_ascii_lowercase() == expected_name
        });

        if !exists {
            presets.push(default_preset);
        }
    }
}

pub(crate) fn insert_missing_opencode_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_opencode_presets());
}

pub(crate) fn insert_missing_gemini_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_gemini_presets());
}

pub(crate) fn insert_missing_kilo_presets(presets: &mut Vec<PresetConfig>) {
    insert_missing_agent_presets(presets, default_kilo_presets());
}

fn default_presets() -> Vec<PresetConfig> {
    let mut presets = vec![
        PresetConfig {
            name: "Shell".to_string(),
            alias: Some("sh".to_string()),
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Codex".to_string(),
            alias: Some("cx".to_string()),
            kind: PanelKind::Codex,
            command: None,
            args: vec!["--no-alt-screen".to_string()],
            resume: PanelResume::Last,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Codex (YOLO)".to_string(),
            alias: Some("cxy".to_string()),
            kind: PanelKind::Codex,
            command: None,
            args: vec!["--full-auto".to_string(), "--no-alt-screen".to_string()],
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Claude Code".to_string(),
            alias: Some("cc".to_string()),
            kind: PanelKind::Claude,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Last,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Claude Code (Auto)".to_string(),
            alias: Some("cca".to_string()),
            kind: PanelKind::Claude,
            command: None,
            args: vec!["--permission-mode".to_string(), "auto".to_string()],
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
    ];
    presets.extend(default_opencode_presets());
    presets.extend(default_gemini_presets());
    presets.extend(default_kilo_presets());
    presets.extend([
        PresetConfig {
            name: "Git Changes".to_string(),
            alias: Some("gc".to_string()),
            kind: PanelKind::GitChanges,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Markdown".to_string(),
            alias: Some("md".to_string()),
            kind: PanelKind::Editor,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
        PresetConfig {
            name: "Usage".to_string(),
            alias: Some("u".to_string()),
            kind: PanelKind::Usage,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
    ]);
    presets
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
    pub attention_feed_height: f32,
    pub attention_feed_width: f32,
    pub minimap_height: f32,
    pub minimap_width: f32,
}

impl Default for OverlaysConfig {
    fn default() -> Self {
        Self {
            attention_feed_height: 600.0,
            attention_feed_width: 320.0,
            minimap_height: 180.0,
            minimap_width: 320.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct FeaturesConfig {
    pub attention_feed: bool,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self { attention_feed: true }
    }
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
    /// Load config from an explicit path, or search standard locations,
    /// or return a default config with one workspace and one shell.
    ///
    /// # Errors
    ///
    /// Returns an error if a discovered config file cannot be read or parsed.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let (mut config, resolved_path) = if let Some(p) = path {
            let contents = std::fs::read_to_string(p)?;
            (Self::deserialize_yaml(&contents)?, Some(p.to_path_buf()))
        } else {
            let mut found = None;
            for candidate in config_candidates() {
                if candidate.exists() {
                    let contents = std::fs::read_to_string(&candidate)?;
                    tracing::info!("loaded config from {}", candidate.display());
                    found = Some((Self::deserialize_yaml(&contents)?, candidate));
                    break;
                }
            }
            if let Some((config, path)) = found {
                (config, Some(path))
            } else {
                tracing::info!("no config found, using defaults");
                (Self::default(), None)
            }
        };

        if let Some(ref config_path) = resolved_path {
            config_migration::migrate_if_needed(&mut config, config_path)?;
        }

        config.validate()?;
        Ok(config)
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
        serde_yaml::from_str(contents).map_err(|e| Error::Config(e.to_string()))
    }

    /// Serialize this config to YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::Config(e.to_string()))
    }

    /// Return the default config file path (`~/.horizon/config.yaml`).
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
            .find(|candidate| candidate.exists())
            .or_else(Self::default_path)
    }

    #[must_use]
    pub fn expand_tilde(s: &str) -> PathBuf {
        if let Some(rest) = s.strip_prefix("~/")
            && let Ok(home) = std::env::var("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
        PathBuf::from(s)
    }

    /// Validate semantic config rules that deserialization alone cannot catch.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured shortcut is invalid or duplicated.
    pub fn validate(&self) -> Result<()> {
        self.shortcuts.resolve()?;
        validate_ssh_connections(&self.presets, &self.workspaces)?;
        Ok(())
    }

    #[must_use]
    pub fn resolved_presets(&self) -> Vec<PresetConfig> {
        let mut presets = self.presets.clone();
        let mut known_names: std::collections::HashSet<String> =
            presets.iter().map(|preset| preset.name.to_ascii_lowercase()).collect();
        let mut known_targets: std::collections::HashSet<String> = presets
            .iter()
            .filter_map(|preset| preset.ssh_connection.as_ref())
            .map(normalized_ssh_target)
            .collect();

        match discover_ssh_hosts(None) {
            Ok(discovered_hosts) => {
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
            Err(error) => tracing::warn!(%error, "failed to discover ssh presets"),
        }

        presets
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
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn config_candidates_with_env(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

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
mod tests {
    use std::path::PathBuf;

    use super::{Config, FeaturesConfig, PresetConfig, config_candidates_with_env};
    use crate::panel::PanelKind;

    #[test]
    fn includes_horizon_config_candidates() {
        let temp_home = PathBuf::from("/tmp/horizon-home");
        let candidates = config_candidates_with_env(Some(temp_home.join(".config")), Some(temp_home));

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
    fn features_default_enables_attention_feed() {
        assert!(FeaturesConfig::default().attention_feed);
        assert!(Config::default().features.attention_feed);
    }

    #[test]
    fn missing_features_block_keeps_attention_feed_enabled() {
        let config: Config = serde_yaml::from_str("{}\n").expect("config should deserialize");

        assert!(config.features.attention_feed);
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
    fn explicit_attention_feed_false_is_preserved() {
        let config: Config =
            serde_yaml::from_str("features:\n  attention_feed: false\n").expect("config should deserialize");

        assert!(!config.features.attention_feed);
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
