use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceConfig {
    pub name: String,
    pub color: Option<String>,
    pub position: Option<[f32; 2]>,
    #[serde(default)]
    pub terminals: Vec<TerminalConfig>,
}

#[derive(Debug, Deserialize)]
pub struct TerminalConfig {
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_cols")]
    pub cols: u16,
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
        if let Some(p) = path {
            let contents = std::fs::read_to_string(p)?;
            return serde_yaml::from_str(&contents).map_err(|e| Error::Config(e.to_string()));
        }

        // Search standard locations
        for candidate in config_candidates() {
            if candidate.exists() {
                let contents = std::fs::read_to_string(&candidate)?;
                tracing::info!("loaded config from {}", candidate.display());
                return serde_yaml::from_str(&contents).map_err(|e| Error::Config(e.to_string()));
            }
        }

        // Default: one workspace with one shell terminal
        tracing::info!("no config found, using defaults");
        Ok(Self::default())
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

impl Default for Config {
    fn default() -> Self {
        Self {
            workspaces: vec![WorkspaceConfig {
                name: "default".to_string(),
                color: None,
                position: None,
                terminals: vec![TerminalConfig {
                    name: "shell".to_string(),
                    command: None,
                    args: Vec::new(),
                    cwd: None,
                    rows: default_rows(),
                    cols: default_cols(),
                }],
            }],
        }
    }
}

fn config_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(&xdg).join("termgalore/config.yaml"));
        paths.push(PathBuf::from(&xdg).join("termgalore/config.yml"));
    }

    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".config/termgalore/config.yaml"));
        paths.push(home.join(".config/termgalore/config.yml"));
        paths.push(home.join(".termgalore.yaml"));
        paths.push(home.join(".termgalore.yml"));
    }

    // Current directory
    paths.push(PathBuf::from("termgalore.yaml"));
    paths.push(PathBuf::from("termgalore.yml"));

    paths
}
