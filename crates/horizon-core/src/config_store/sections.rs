//! The sections of a configuration, and their encoding inside the store.
//!
//! One row holds one section, so a later change can write only the section that
//! changed. A section that this build does not know stays in the store and is
//! not read, so an older Horizon does not drop the settings of a newer one.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::super::config::Config;
use crate::error::{Error, Result};

/// The format of the `data` column. A build refuses a row of another format
/// instead of guessing.
pub(super) const FORMAT: &str = "json";

pub(super) struct Row {
    pub(super) section: &'static str,
    pub(super) data: String,
}

pub(super) fn encode(config: &Config) -> Result<Vec<Row>> {
    Ok(vec![
        Row {
            section: "version",
            data: json(&config.version)?,
        },
        Row {
            section: "window",
            data: json(&config.window)?,
        },
        Row {
            section: "appearance",
            data: json(&config.appearance)?,
        },
        Row {
            section: "shortcuts",
            data: json(&config.shortcuts)?,
        },
        Row {
            section: "overlays",
            data: json(&config.overlays)?,
        },
        Row {
            section: "features",
            data: json(&config.features)?,
        },
        Row {
            section: "browser",
            data: json(&config.browser)?,
        },
        Row {
            section: "remote_hosts",
            data: json(&config.remote_hosts)?,
        },
        Row {
            section: "presets",
            data: json(&config.presets)?,
        },
        Row {
            section: "workspaces",
            data: json(&config.workspaces)?,
        },
    ])
}

pub(super) fn decode(rows: &BTreeMap<String, String>) -> Result<Config> {
    let Config {
        version,
        window,
        appearance,
        shortcuts,
        overlays,
        features,
        browser,
        remote_hosts,
        presets,
        workspaces,
    } = Config::default();
    Ok(Config {
        version: section(rows, "version")?.unwrap_or(version),
        window: section(rows, "window")?.unwrap_or(window),
        appearance: section(rows, "appearance")?.unwrap_or(appearance),
        shortcuts: section(rows, "shortcuts")?.unwrap_or(shortcuts),
        overlays: section(rows, "overlays")?.unwrap_or(overlays),
        features: section(rows, "features")?.unwrap_or(features),
        browser: section(rows, "browser")?.unwrap_or(browser),
        remote_hosts: section(rows, "remote_hosts")?.unwrap_or(remote_hosts),
        presets: section(rows, "presets")?.unwrap_or(presets),
        workspaces: section(rows, "workspaces")?.unwrap_or(workspaces),
    })
}

fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::Config(format!("settings section: {error}")))
}

fn section<T: DeserializeOwned>(rows: &BTreeMap<String, String>, name: &str) -> Result<Option<T>> {
    let Some(data) = rows.get(name) else {
        return Ok(None);
    };
    serde_json::from_str(data)
        .map(Some)
        .map_err(|error| Error::State(format!("settings section {name} cannot be read: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, section};
    use crate::config::{Config, PresetConfig, WorkspaceConfig};
    use crate::panel::{PanelKind, PanelResume};
    use std::collections::BTreeMap;

    fn rows(config: &Config) -> BTreeMap<String, String> {
        encode(config)
            .expect("encode")
            .into_iter()
            .map(|row| (row.section.to_string(), row.data))
            .collect()
    }

    #[test]
    fn every_section_round_trips() {
        let mut config = Config::default();
        config.window.width = 1234.0;
        config.appearance.theme = crate::config::AppearanceTheme::Light;
        config.presets.push(PresetConfig {
            name: "Custom".to_string(),
            alias: Some("cu".to_string()),
            kind: PanelKind::Shell,
            command: Some("/bin/zsh".to_string()),
            args: vec!["-l".to_string()],
            resume: PanelResume::Fresh,
            ssh_connection: None,
        });
        config.workspaces.push(WorkspaceConfig {
            name: "Work".to_string(),
            color: None,
            cwd: Some("/work".to_string()),
            position: None,
            terminals: Vec::new(),
        });

        let decoded = decode(&rows(&config)).expect("decode");

        assert_eq!(decoded.to_yaml().expect("yaml"), config.to_yaml().expect("yaml"));
    }

    #[test]
    fn a_missing_section_uses_its_default() {
        let config = Config::default();
        let mut stored = rows(&config);
        stored.remove("window");

        let decoded = decode(&stored).expect("decode");

        assert_eq!(
            decoded.window.width,
            Config::default().window.width,
            "a missing section keeps the default"
        );
        assert_eq!(decoded.presets.len(), config.presets.len());
    }

    #[test]
    fn an_unknown_section_is_ignored() {
        let config = Config::default();
        let mut stored = rows(&config);
        stored.insert("from_a_newer_horizon".to_string(), "{}".to_string());

        assert!(decode(&stored).is_ok(), "an unknown section does not stop the read");
    }

    #[test]
    fn a_section_of_the_wrong_shape_is_a_readable_error() {
        let config = Config::default();
        let mut stored = rows(&config);
        stored.insert("window".to_string(), "\"not an object\"".to_string());

        let error = decode(&stored).expect_err("a section of the wrong shape fails");

        assert!(
            error.to_string().contains("settings section window"),
            "the error names the section: {error}"
        );
        assert!(section::<u32>(&stored, "version").is_ok(), "other sections still read");
    }
}
