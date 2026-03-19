use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::shortcuts::{AppShortcuts, ShortcutBinding};
use crate::ssh::{SshConnection, discover_ssh_hosts};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub window: WindowConfig,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            shortcuts: ShortcutsConfig::default(),
            overlays: OverlaysConfig::default(),
            features: FeaturesConfig::default(),
            presets: default_presets(),
            workspaces: Vec::new(),
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

fn default_presets() -> Vec<PresetConfig> {
    vec![
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
            args: vec!["--yolo".to_string(), "--no-alt-screen".to_string()],
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
            args: vec!["--dangerously-skip-permissions".to_string()],
            resume: PanelResume::Fresh,
            ssh_connection: None,
        },
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
    ]
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ShortcutsConfig {
    #[serde(alias = "quick_nav")]
    pub command_palette: String,
    pub new_terminal: String,
    pub open_remote_hosts: String,
    pub toggle_sidebar: String,
    pub toggle_hud: String,
    pub toggle_minimap: String,
    pub align_workspaces_horizontally: String,
    pub toggle_settings: String,
    pub reset_view: String,
    pub zoom_in: String,
    pub zoom_out: String,
    pub fullscreen_panel: String,
    pub exit_fullscreen_panel: String,
    pub fullscreen_window: String,
    pub save_editor: String,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            command_palette: "Ctrl+K".to_string(),
            new_terminal: "Ctrl+N".to_string(),
            open_remote_hosts: "Ctrl+Shift+H".to_string(),
            toggle_sidebar: "Ctrl+B".to_string(),
            toggle_hud: "Ctrl+Shift+U".to_string(),
            toggle_minimap: "Ctrl+Shift+M".to_string(),
            align_workspaces_horizontally: "Ctrl+Shift+A".to_string(),
            toggle_settings: "Ctrl+,".to_string(),
            reset_view: "Ctrl+0".to_string(),
            zoom_in: "Ctrl+Plus".to_string(),
            zoom_out: "Ctrl+Minus".to_string(),
            fullscreen_panel: "F11".to_string(),
            exit_fullscreen_panel: "Escape".to_string(),
            fullscreen_window: "Ctrl+F11".to_string(),
            save_editor: "Ctrl+S".to_string(),
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
            open_remote_hosts: parse_shortcut("open_remote_hosts", &self.open_remote_hosts)?,
            toggle_sidebar: parse_shortcut("toggle_sidebar", &self.toggle_sidebar)?,
            toggle_hud: parse_shortcut("toggle_hud", &self.toggle_hud)?,
            toggle_minimap: parse_shortcut("toggle_minimap", &self.toggle_minimap)?,
            align_workspaces_horizontally: parse_shortcut(
                "align_workspaces_horizontally",
                &self.align_workspaces_horizontally,
            )?,
            toggle_settings: parse_shortcut("toggle_settings", &self.toggle_settings)?,
            reset_view: parse_shortcut("reset_view", &self.reset_view)?,
            zoom_in: parse_shortcut("zoom_in", &self.zoom_in)?,
            zoom_out: parse_shortcut("zoom_out", &self.zoom_out)?,
            fullscreen_panel: parse_shortcut("fullscreen_panel", &self.fullscreen_panel)?,
            exit_fullscreen_panel: parse_shortcut("exit_fullscreen_panel", &self.exit_fullscreen_panel)?,
            fullscreen_window: parse_shortcut("fullscreen_window", &self.fullscreen_window)?,
            save_editor: parse_shortcut("save_editor", &self.save_editor)?,
        };

        validate_distinct_shortcuts([
            ("command_palette", shortcuts.command_palette),
            ("new_terminal", shortcuts.new_terminal),
            ("open_remote_hosts", shortcuts.open_remote_hosts),
            ("toggle_sidebar", shortcuts.toggle_sidebar),
            ("toggle_hud", shortcuts.toggle_hud),
            ("toggle_minimap", shortcuts.toggle_minimap),
            ("align_workspaces_horizontally", shortcuts.align_workspaces_horizontally),
            ("toggle_settings", shortcuts.toggle_settings),
            ("reset_view", shortcuts.reset_view),
            ("zoom_in", shortcuts.zoom_in),
            ("zoom_out", shortcuts.zoom_out),
            ("fullscreen_panel", shortcuts.fullscreen_panel),
            ("exit_fullscreen_panel", shortcuts.exit_fullscreen_panel),
            ("fullscreen_window", shortcuts.fullscreen_window),
            ("save_editor", shortcuts.save_editor),
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
        let config = if let Some(p) = path {
            let contents = std::fs::read_to_string(p)?;
            Self::from_yaml(&contents)?
        } else {
            let mut found = None;
            for candidate in config_candidates() {
                if candidate.exists() {
                    let contents = std::fs::read_to_string(&candidate)?;
                    tracing::info!("loaded config from {}", candidate.display());
                    found = Some(Self::from_yaml(&contents)?);
                    break;
                }
            }
            found.unwrap_or_else(|| {
                tracing::info!("no config found, using defaults");
                Self::default()
            })
        };

        Ok(config)
    }

    /// Parse and validate config YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization or semantic validation fails.
    pub fn from_yaml(contents: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(contents).map_err(|e| Error::Config(e.to_string()))?;
        config.validate()?;
        Ok(config)
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
    fn explicit_attention_feed_false_is_preserved() {
        let config: Config =
            serde_yaml::from_str("features:\n  attention_feed: false\n").expect("config should deserialize");

        assert!(!config.features.attention_feed);
    }

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        let error = Config::from_yaml("shortcuts:\n  command_palette: Ctrl+K\n  new_terminal: Ctrl+K\n")
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
    fn overlapping_shortcuts_are_rejected() {
        let error =
            Config::from_yaml("shortcuts:\n  toggle_sidebar: Ctrl+B\n  align_workspaces_horizontally: Ctrl+Shift+B\n")
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
