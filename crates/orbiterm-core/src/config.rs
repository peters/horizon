use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::panel::{PanelKind, PanelResume};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub shortcuts: ShortcutsConfig,
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
}

impl PresetConfig {
    /// Convert this preset into `PanelOptions` for panel creation.
    #[must_use]
    pub fn to_panel_options(&self) -> crate::panel::PanelOptions {
        crate::panel::PanelOptions {
            name: Some(self.name.clone()),
            command: self.command.clone(),
            args: self.args.clone(),
            kind: self.kind,
            resume: self.resume.clone(),
            ..crate::panel::PanelOptions::default()
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

fn default_presets() -> Vec<PresetConfig> {
    vec![
        PresetConfig {
            name: "Shell".to_string(),
            alias: Some("sh".to_string()),
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
        },
        PresetConfig {
            name: "Codex".to_string(),
            alias: Some("cx".to_string()),
            kind: PanelKind::Codex,
            command: None,
            args: vec!["--no-alt-screen".to_string()],
            resume: PanelResume::Last,
        },
        PresetConfig {
            name: "Codex (YOLO)".to_string(),
            alias: Some("cxy".to_string()),
            kind: PanelKind::Codex,
            command: None,
            args: vec!["--full-auto".to_string(), "--no-alt-screen".to_string()],
            resume: PanelResume::Fresh,
        },
        PresetConfig {
            name: "Claude Code".to_string(),
            alias: Some("cc".to_string()),
            kind: PanelKind::Claude,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Last,
        },
        PresetConfig {
            name: "Claude Code (Auto)".to_string(),
            alias: Some("cca".to_string()),
            kind: PanelKind::Claude,
            command: None,
            args: vec!["--dangerously-skip-permissions".to_string()],
            resume: PanelResume::Fresh,
        },
    ]
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ShortcutsConfig {
    pub new_terminal: String,
    pub toggle_sidebar: String,
    pub toggle_hud: String,
    pub toggle_settings: String,
    pub reset_view: String,
    pub fullscreen_panel: String,
    pub fullscreen_window: String,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            new_terminal: "Ctrl+N".to_string(),
            toggle_sidebar: "Ctrl+B".to_string(),
            toggle_hud: "Ctrl+H".to_string(),
            toggle_settings: "Ctrl+,".to_string(),
            reset_view: "Ctrl+0".to_string(),
            fullscreen_panel: "F11".to_string(),
            fullscreen_window: "Ctrl+F11".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
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
            serde_yaml::from_str(&contents).map_err(|e| Error::Config(e.to_string()))?
        } else {
            let mut found = None;
            for candidate in config_candidates() {
                if candidate.exists() {
                    let contents = std::fs::read_to_string(&candidate)?;
                    tracing::info!("loaded config from {}", candidate.display());
                    found =
                        Some(serde_yaml::from_str(&contents).map_err(|e| Error::Config(e.to_string()))?);
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

    /// Serialize this config to YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| Error::Config(e.to_string()))
    }

    /// Return the default config file path (`~/.config/orbiterm/config.yaml`).
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".config/orbiterm/config.yaml"))
        } else {
            None
        }
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
}

fn config_candidates() -> Vec<PathBuf> {
    config_candidates_with_env(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn config_candidates_with_env(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(xdg) = xdg_config_home {
        push_config_dir_candidates(&mut paths, &xdg.join("orbiterm"));
    }

    if let Some(home) = home {
        push_config_dir_candidates(&mut paths, &home.join(".config/orbiterm"));
        paths.push(home.join(".orbiterm.yaml"));
        paths.push(home.join(".orbiterm.yml"));
    }

    paths.push(PathBuf::from("orbiterm.yaml"));
    paths.push(PathBuf::from("orbiterm.yml"));

    paths
}

fn push_config_dir_candidates(paths: &mut Vec<PathBuf>, base: &Path) {
    paths.push(base.join("config.yaml"));
    paths.push(base.join("config.yml"));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::config_candidates_with_env;

    #[test]
    fn includes_orbiterm_config_candidates() {
        let temp_home = PathBuf::from("/tmp/orbiterm-home");
        let candidates = config_candidates_with_env(Some(temp_home.join(".config")), Some(temp_home));

        assert!(candidates.iter().any(|path| path.ends_with("orbiterm/config.yaml")));
        assert!(candidates.iter().any(|path| path.ends_with(".orbiterm.yaml")));
        assert!(candidates.iter().any(|path| path == &PathBuf::from("orbiterm.yaml")));
    }
}
