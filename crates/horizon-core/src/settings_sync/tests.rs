use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};

use super::{
    BODY_FORMAT, SECTIONS, Scope, SectionBody, apply_to_store, arrive, backup_path, body_of, changed_sections, covers,
    decode_body, encode_body, is_shared, merged_config, merged_for_push, read_store, shared_sections, unknown_fields,
    with_section, write_backup,
};
use crate::config::Config;
use crate::config_store::ConfigStore;

fn store(name: &str) -> (tempfile::TempDir, ConfigStore) {
    let directory = tempfile::tempdir().expect("a directory");
    let store = ConfigStore::at(directory.path().join(name));
    (directory, store)
}

fn saved(config: &Config) -> (tempfile::TempDir, ConfigStore) {
    let (directory, store) = store("settings.sqlite");
    store.save(config).expect("the store of this computer");
    (directory, store)
}

fn body_of_section(config: &Config, name: &str) -> Value {
    body_of(config, name)
        .expect("the section of this build")
        .expect("the section of this configuration")
}

/// A body of one section of `config`, as another computer would send it.
fn envelope(config: &Config, name: &str) -> String {
    encode_body(config, name)
        .expect("the body of the section")
        .expect("a shared section")
}

fn arriving(name: &str, text: &str) -> BTreeMap<String, Value> {
    let body = decode_body(text).expect("one section body");
    let mut map = BTreeMap::new();
    map.insert(name.to_owned(), arrive(name, &body).expect("a section of this build"));
    map
}

#[test]
fn every_section_of_the_store_has_one_decision() {
    let config = Config::default();
    let document = serde_json::to_value(&config).expect("the settings of this build");
    let mut fields: Vec<&str> = document
        .as_object()
        .expect("one object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    let mut table: Vec<&str> = SECTIONS.iter().map(|section| section.name).collect();
    table.sort_unstable();
    assert_eq!(
        fields, table,
        "the table names every section of one configuration, and only those"
    );

    // The store writes one row for each section of the table, and this build reads
    // every row of the table back.
    let mut rows = BTreeMap::new();
    for section in SECTIONS {
        let value = body_of(&config, section.name)
            .expect("a section of this build")
            .unwrap_or_else(|| panic!("{} is one section that this build encodes", section.name));
        rows.insert(section.name.to_owned(), serde_json::to_string(&value).expect("json"));
    }
    crate::config_store::decode_rows(&rows).expect("every section of the table reads back");
}

#[test]
fn every_local_section_says_why_it_stays() {
    let local: Vec<(&str, &str)> = SECTIONS
        .iter()
        .filter_map(|section| match section.scope {
            Scope::Local(reason) => Some((section.name, reason)),
            Scope::Shared => None,
        })
        .collect();
    assert_eq!(
        local.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        vec!["version", "window", "server"],
        "the three sections that stay with one computer"
    );
    for (name, reason) in local {
        assert!(reason.len() > 20, "the reason of {name} is one sentence: {reason}");
    }
}

#[test]
fn a_shared_section_travels_in_one_envelope_and_a_local_one_does_not() {
    let config = Config {
        appearance: crate::config::AppearanceConfig {
            theme: crate::config::AppearanceTheme::Light,
        },
        ..Config::default()
    };
    let text = envelope(&config, "appearance");
    let body = decode_body(&text).expect("one body");
    assert_eq!(body.format, BODY_FORMAT);
    assert_eq!(body.config_version, config.version);
    assert_eq!(body.section, json!({ "theme": "light" }));
    assert_eq!(
        arrive("appearance", &body).expect("a section of this build"),
        json!({ "theme": "light" })
    );

    for local in ["window", "server", "version"] {
        assert_eq!(encode_body(&config, local).expect("the section"), None, "{local} stays");
    }
    assert!(
        is_shared("a_section_of_a_newer_horizon"),
        "an unknown section is shared"
    );
    assert!(
        !super::is_section("a_section_of_a_newer_horizon"),
        "an unknown section is no field of this build"
    );
    assert_eq!(
        shared_sections().count(),
        SECTIONS.len() - 3,
        "one shared section per section that does not stay"
    );
    assert_eq!(
        body_of(&config, "nothing_of_this_build").expect("no fault"),
        None,
        "a name that this build does not have is no section"
    );
}

#[test]
fn an_unknown_section_is_shared_and_survives_the_save_of_an_older_build() {
    let (directory, store) = saved(&Config::default());
    // A newer Horizon wrote a section that this build does not know.
    let mut rows = BTreeMap::new();
    rows.insert("of_a_newer_horizon".to_owned(), json!({ "value": 7 }).to_string());
    store.write_sections(&rows).expect("the row of the newer section");

    let arriving = arriving("appearance", &envelope(&Config::default(), "appearance"));
    apply_to_store(&store, &arriving).expect("the section of the server");
    Config::save(store.path(), &Config::default()).expect("a save of this build");

    let rows = store.section_rows().expect("the rows of the store");
    assert_eq!(
        rows.get("of_a_newer_horizon").map(String::as_str),
        Some("{\"value\":7}"),
        "a section of a newer Horizon stays on this computer"
    );
    drop(directory);
}

#[test]
fn a_section_of_an_older_configuration_version_is_migrated_before_it_is_applied() {
    // Version 1 held `Ctrl+K` for the command palette and version 2 moved it to
    // `Ctrl+Shift+K`, so a body of version 1 arrives with the binding of version 2.
    let body = json!({
        "format": BODY_FORMAT,
        "config_version": 1,
        "section": {
            "command_palette": "Ctrl+K",
            "new_terminal": "Ctrl+N",
            "open_remote_hosts": "Ctrl+Shift+R",
            "toggle_sidebar": "Ctrl+B",
            "toggle_hud": "Ctrl+Shift+H",
            "toggle_settings": "Ctrl+,",
            "zoom_reset": "Ctrl+0",
            "zoom_in": "Ctrl+Plus",
            "zoom_out": "Ctrl+Minus",
            "fullscreen_window": "Ctrl+F11",
            "save_editor": "Ctrl+S"
        }
    });
    let body: SectionBody = serde_json::from_value(body).expect("one body");
    let section = arrive("shortcuts", &body).expect("a section of an older version");
    assert_eq!(section["command_palette"], json!("Ctrl+Shift+K"));
    assert_eq!(section["save_editor"], json!("Ctrl+Shift+S"));
}

#[test]
fn a_section_of_a_newer_configuration_version_stays_on_the_server() {
    let body = SectionBody {
        format: BODY_FORMAT,
        config_version: crate::config_migration::CURRENT_CONFIG_VERSION + 1,
        section: json!({ "theme": "light" }),
    };
    let fault = arrive("appearance", &body).expect_err("a newer section is refused");
    assert!(
        matches!(
            fault,
            super::SectionFault::NewerHorizon {
                found,
                expected
            } if found == crate::config_migration::CURRENT_CONFIG_VERSION + 1
                && expected == crate::config_migration::CURRENT_CONFIG_VERSION
        ),
        "{fault}"
    );
    assert!(
        fault.to_string().contains("newer Horizon"),
        "the person reads that this computer runs an older Horizon: {fault}"
    );

    // The store of this computer keeps its own row.
    let (_directory, store) = saved(&Config::default());
    let arriving = arriving("appearance", &envelope(&Config::default(), "appearance"));
    apply_to_store(&store, &arriving).expect("the section of this build");
    assert_eq!(
        read_store(&store)
            .expect("the settings of this computer")
            .appearance
            .theme,
        crate::config::AppearanceTheme::Auto
    );
}

#[test]
fn a_section_that_fails_the_rule_is_not_applied_and_not_written() {
    let (_directory, store) = saved(&Config::default());
    // Two shortcuts of one key are refused by the rule of the settings.
    let body = json!({
        "format": BODY_FORMAT,
        "config_version": crate::config_migration::CURRENT_CONFIG_VERSION,
        "section": {
            "command_palette": "Ctrl+Shift+K",
            "new_terminal": "Ctrl+Shift+K"
        }
    });
    let text = serde_json::to_string(&body).expect("json");
    let arriving = arriving("shortcuts", &text);
    let fault = apply_to_store(&store, &arriving).expect_err("the rule refuses the section");
    assert!(matches!(fault, super::SectionFault::Refused(_)), "{fault}");

    let stored = store.section_rows().expect("the rows of the store");
    let shortcuts: Value = serde_json::from_str(stored.get("shortcuts").expect("the row")).expect("json");
    assert_eq!(
        shortcuts["command_palette"],
        json!(Config::default().shortcuts.command_palette),
        "the row of this computer is exactly what it was"
    );
}

#[test]
fn an_unknown_field_of_a_known_section_stays_in_the_store_and_in_the_next_push() {
    let (_directory, store) = saved(&Config::default());
    let body = json!({
        "format": BODY_FORMAT,
        "config_version": crate::config_migration::CURRENT_CONFIG_VERSION,
        "section": { "theme": "light", "of_a_newer_horizon": 7 }
    });
    let text = serde_json::to_string(&body).expect("json");
    let arriving = arriving("appearance", &text);
    apply_to_store(&store, &arriving).expect("the section of the server");

    let stored = store.section_rows().expect("the rows of the store");
    let stored_appearance: Value = serde_json::from_str(stored.get("appearance").expect("the row")).expect("json");
    assert_eq!(
        stored_appearance,
        json!({ "theme": "light", "of_a_newer_horizon": 7 }),
        "the store keeps the field that this build does not know"
    );

    // A local change of the same section goes back with the field of the newer
    // Horizon, so an older computer never drops it from the server.
    let mut local = Config::default();
    local.appearance.theme = crate::config::AppearanceTheme::Dark;
    let local_value = body_of_section(&local, "appearance");
    // The mirror of the link holds the body of the server, so the next push of
    // this computer merges that body and not the typed value of the store.
    let server_value = stored_appearance.clone();
    let pushed = merged_for_push(Some(&server_value), &local_value);
    assert_eq!(pushed, json!({ "theme": "dark", "of_a_newer_horizon": 7 }));
    assert_eq!(
        unknown_fields(&server_value, &local_value),
        Some(vec!["of_a_newer_horizon".to_owned()])
    );
    assert!(
        !covers(&server_value, &local_value),
        "the value of this computer differs"
    );
    assert!(covers(&server_value, &server_value), "the same value is no difference");
}

#[test]
fn a_server_body_with_the_same_values_covers_this_computer() {
    let config = Config::default();
    let local = body_of_section(&config, "appearance");
    assert!(covers(&local, &local));
    let with_unknown = merged_for_push(Some(&json!({ "theme": "auto", "later": true })), &local);
    assert_eq!(with_unknown, json!({ "theme": "auto", "later": true }));
    assert!(
        !covers(&with_unknown, &local),
        "a field of a newer Horizon is a difference"
    );
    assert_eq!(
        merged_for_push(Some(&json!([1, 2])), &local),
        local,
        "a section that is not an object is sent as this build reads it"
    );
}

#[test]
fn a_local_section_never_becomes_a_change() {
    let before = Config::default();
    let mut after = Config::default();
    after.window.width = 900.0;
    after.server.ssh_target = Some("server-vm".to_owned());
    after.version = 1;
    assert!(
        changed_sections(&before, &after).is_empty(),
        "no local section is a change"
    );

    after.appearance.theme = crate::config::AppearanceTheme::Light;
    after.shortcuts.search = "Ctrl+Shift+G".to_owned();
    assert_eq!(changed_sections(&before, &after), vec!["appearance", "shortcuts"]);
}

#[test]
fn a_section_of_a_computer_that_names_a_folder_this_one_does_not_have_still_applies() {
    let (directory, store) = saved(&Config::default());
    let missing = directory.path().join("a-folder-of-another-computer");
    let mut other = Config::default();
    other.workspaces.push(crate::config::WorkspaceConfig {
        name: "Moved".to_owned(),
        color: None,
        cwd: Some(missing.display().to_string()),
        position: None,
        terminals: Vec::new(),
    });
    other.presets.push(crate::config::PresetConfig {
        name: "Moved shell".to_owned(),
        alias: None,
        kind: crate::panel::PanelKind::Shell,
        command: None,
        args: Vec::new(),
        resume: crate::panel::PanelResume::Fresh,
        ssh_connection: None,
    });

    for name in ["workspaces", "presets"] {
        let arriving = arriving(name, &envelope(&other, name));
        apply_to_store(&store, &arriving).expect("a name of another computer applies");
    }
    let config = read_store(&store).expect("the settings of this computer");
    assert_eq!(config.workspaces.len(), 1);
    assert!(!missing.exists(), "the folder is not here");
    assert!(
        config.workspaces[0]
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd == missing.display().to_string()),
        "the section keeps the name that the other computer wrote"
    );
}

#[test]
fn a_speech_section_of_another_computer_keeps_the_rule_of_this_one() {
    let (_directory, store) = saved(&Config::default());
    // The recorder line of another computer arrives. A program that this computer
    // does not have fails its take with the sentence of today (`... is not installed
    // on this computer`, and the take test of that sentence is in the speech
    // module); an empty line turns speech off with its sentence.
    let mut other = Config::default();
    other.speech.recorder = "/nonexistent/pw-record {file}".to_owned();
    other.speech.transcriber.kind = crate::config::TranscriberKind::Service;
    let speech = arriving("speech", &envelope(&other, "speech"));
    apply_to_store(&store, &speech).expect("the speech section applies");
    let config = read_store(&store).expect("the settings of this computer");
    assert_eq!(config.speech.recorder, "/nonexistent/pw-record {file}");
    assert!(
        config.speech.is_on(),
        "the rule of the section is the same on every computer"
    );
    assert_eq!(config.speech.off_reason(), None);

    let mut empty = Config::default();
    empty.speech.recorder = String::new();
    let empty_section = arriving("speech", &envelope(&empty, "speech"));
    apply_to_store(&store, &empty_section).expect("the empty recorder applies");
    let config = read_store(&store).expect("the settings of this computer");
    assert!(!config.speech.is_on());
    assert_eq!(config.speech.off_reason(), Some("The recorder line is empty."));
}

#[test]
fn a_credential_reference_without_a_value_here_keeps_the_rule_of_today() {
    let (_directory, store) = saved(&Config::default());
    let mut other = Config::default();
    other.browser.remote = serde_json::from_value(json!({
        "providers": {
            "grid": {
                "endpoint": "https://grid.example.net/wd/hub",
                "authentication": { "kind": "bearer", "token_ref": "grid-token" }
            }
        }
    }))
    .expect("a remote browser section");
    let arriving = arriving("browser", &envelope(&other, "browser"));
    apply_to_store(&store, &arriving).expect("the browser section applies");

    let config = read_store(&store).expect("the settings of this computer");
    let presence = config.browser.remote.binding_presence();
    assert_eq!(
        presence["grid"].values().filter(|present| !**present).count(),
        1,
        "the reference of another computer is a readiness state here, not a refused section"
    );
}

#[test]
fn the_settings_of_before_the_first_link_are_kept_beside_the_store() {
    let (directory, store) = saved(&Config::default());
    let path = backup_path(&store, 1_700_000_000_000);
    assert_eq!(path.parent(), store.path().parent());
    assert_eq!(
        path.file_name().and_then(std::ffi::OsStr::to_str),
        Some("settings.before-server.1700000000000.json")
    );

    let mut sections = BTreeMap::new();
    sections.insert("appearance".to_owned(), json!({ "theme": "dark" }).to_string());
    write_backup(&path, &sections).expect("the backup");
    assert_eq!(
        std::fs::metadata(&path).expect("the file").permissions().mode() & 0o777,
        0o600,
        "the settings of a person are private"
    );
    assert!(directory.path().exists());
}

#[test]
fn one_section_named_by_the_wrong_build_is_one_fault_and_not_a_panic() {
    assert!(decode_body("{\"format\":99,\"config_version\":12,\"section\":{}}").is_err());
    assert!(decode_body("not json").is_err());
    let (_directory, store) = saved(&Config::default());
    let config = Config::default();
    let merged = merged_config(&store.section_rows().expect("the rows"), &BTreeMap::new());
    assert!(merged.is_ok(), "the rows of this computer read");
    assert!(
        with_section(&config, "of_a_newer_horizon", &json!({ "a": 1 })).is_err(),
        "a name that this build does not know is no section of it"
    );
    assert!(
        with_section(&config, "appearance", &json!("not an object")).is_err(),
        "a section of the wrong shape is a fault and not a panic"
    );
}
