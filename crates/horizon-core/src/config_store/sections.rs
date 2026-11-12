//! The sections of a configuration, and their encoding inside the store.
//!
//! One row holds one section, so a later change can write only the section that
//! changed. A section that this build does not know stays in the store and is
//! not read, so an older Horizon does not drop the settings of a newer one.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

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
    crate::settings_sync::SECTIONS
        .iter()
        .map(|section| {
            let data = encode_one(config, section.name)?.ok_or_else(|| {
                Error::Config(format!(
                    "the section {} of the settings is not one that this build encodes",
                    section.name
                ))
            })?;
            Ok(Row {
                section: section.name,
                data,
            })
        })
        .collect()
}

/// The JSON of one section of `config`, or `None` when this build has no such
/// section.
pub(crate) fn encode_one(config: &Config, name: &str) -> Result<Option<String>> {
    match value_of(config, name) {
        Some(value) => Ok(Some(json(&value?)?)),
        None => Ok(None),
    }
}

/// The JSON value of one section of `config`, or `None` when this build has no
/// such section.
pub(crate) fn value_of(config: &Config, name: &str) -> Option<Result<Value>> {
    let value = match name {
        "version" => json_value(&config.version),
        "window" => json_value(&config.window),
        "appearance" => json_value(&config.appearance),
        "shortcuts" => json_value(&config.shortcuts),
        "overlays" => json_value(&config.overlays),
        "features" => json_value(&config.features),
        "browser" => json_value(&config.browser),
        "remote_hosts" => json_value(&config.remote_hosts),
        "browser_host" => json_value(&config.browser_host),
        "server" => json_value(&config.server),
        "secrets" => json_value(&config.secrets),
        "speech" => json_value(&config.speech),
        "presets" => json_value(&config.presets),
        "workspaces" => json_value(&config.workspaces),
        _ => return None,
    };
    Some(value)
}

pub(crate) fn decode(rows: &BTreeMap<String, String>) -> Result<Config> {
    let Config {
        version,
        window,
        appearance,
        shortcuts,
        overlays,
        features,
        browser,
        remote_hosts,
        browser_host,
        server,
        secrets,
        speech,
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
        browser_host: section(rows, "browser_host")?.unwrap_or(browser_host),
        server: section(rows, "server")?.unwrap_or(server),
        secrets: section(rows, "secrets")?.unwrap_or(secrets),
        speech: section(rows, "speech")?.unwrap_or(speech),
        presets: panel_section(rows, "presets")?.unwrap_or(presets),
        workspaces: panel_section(rows, "workspaces")?.unwrap_or(workspaces),
    })
}

fn json(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::Config(format!("settings section: {error}")))
}

fn json_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| Error::Config(format!("settings section: {error}")))
}

fn section<T: DeserializeOwned>(rows: &BTreeMap<String, String>, name: &str) -> Result<Option<T>> {
    let Some(data) = rows.get(name) else {
        return Ok(None);
    };
    serde_json::from_str(data)
        .map(Some)
        .map_err(|error| Error::State(format!("settings section {name} cannot be read: {error}")))
}

/// A section that names a panel kind. An entry of a kind that a later release
/// removed leaves it, as it leaves an imported settings document, so the rest of
/// the section still reads.
fn panel_section<T: DeserializeOwned>(rows: &BTreeMap<String, String>, name: &str) -> Result<Option<T>> {
    let Some(data) = rows.get(name) else {
        return Ok(None);
    };
    let data = crate::retired_panel_kinds::without_retired(data);
    serde_json::from_str(data.as_ref())
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

        assert_eq!(decoded.to_json().expect("json"), config.to_json().expect("json"));
    }

    #[test]
    fn a_named_remote_browser_host_survives_a_store_round_trip() {
        let mut config = Config::default();
        config.browser_host.ssh_target = Some("browser-vm".to_string());
        config.browser_host.profile = Some("mail".to_string());

        let stored = rows(&config);
        let decoded = decode(&stored).expect("decode");

        assert_eq!(decoded.browser_host, config.browser_host);
        assert!(
            stored["browser_host"].contains("browser-vm"),
            "the entry is one row of the store"
        );
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
