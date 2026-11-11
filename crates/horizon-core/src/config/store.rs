//! The policy of the settings store: when a settings file is imported, and what
//! a person gets when the store or the file is missing or unreadable.

use std::path::PathBuf;

use crate::config_store::ConfigStore;
use crate::{Config, config_migration::CURRENT_CONFIG_VERSION};

const FIRST: &str = r#"{
  "version": 11,
  "window": {"width": 1010, "height": 700},
  "appearance": {"theme": "light"}
}"#;

const SECOND: &str = r#"{
  "version": 11,
  "window": {"width": 1010, "height": 880},
  "appearance": {"theme": "dark"}
}"#;

struct Home {
    root: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("temporary home"),
        }
    }

    fn config_path(&self) -> PathBuf {
        self.root.path().join("config.json")
    }

    fn store_path(&self) -> PathBuf {
        self.root.path().join("settings.sqlite")
    }

    fn write_copy(&self, text: &str) {
        std::fs::write(self.config_path(), text).expect("write the copy");
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.path())
            .expect("read the home")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn load(&self) -> crate::error::Result<Config> {
        Config::load(Some(&self.config_path()))
    }
}

#[test]
fn a_settings_file_is_imported_one_time() {
    let home = Home::new();
    home.write_copy(FIRST);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.window.width, 1010.0, "the values of the file arrive");
    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Light);
    assert!(home.store_path().is_file(), "the store is written next to the file");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        FIRST,
        "the imported file stays exactly as it is"
    );

    let again = home.load().expect("second load");

    assert_eq!(again.window.width, 1010.0);
    assert_eq!(home.names().len(), 2, "the second start writes no third file");
}

/// A file of an older Horizon was YAML. This build reads JSON only, refuses such
/// a file with the format in the message, and writes nothing in its place.
#[test]
fn a_settings_file_of_an_older_horizon_is_refused_with_its_format() {
    let home = Home::new();
    let old = "version: 11\nwindow:\n  width: 1010\n";
    home.write_copy(old);

    let error = home.load().expect_err("a YAML file is not a settings file");

    assert!(
        error.to_string().contains("JSON"),
        "the message names the format: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        old,
        "the refused file keeps every byte"
    );
    assert!(!home.store_path().exists(), "a refused file writes no store");
}

#[test]
fn a_copy_beside_a_store_is_not_read() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    home.write_copy(SECOND);

    let loaded = home.load().expect("load with a copy beside the store");

    assert_eq!(
        loaded.window.width, 1010.0,
        "the store is the settings of this computer"
    );
    assert_eq!(loaded.window.height, 700.0, "the copy is not imported");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the copy"),
        SECOND,
        "the copy stays exactly as the person left it"
    );
    assert_eq!(home.names().len(), 2, "no third file is written");
}

#[test]
fn an_import_keeps_everything_that_this_build_does_not_read() {
    let home = Home::new();
    let text = r#"{
  "version": 11,
  "appearance": {"theme": "light"},
  "remote_hosts": {"default_workspace": "Ops", "future_key": true},
  "a_section_of_a_newer_build": {"keep": "me"}
}"#;
    home.write_copy(text);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Light);
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        text,
        "an import loses no unknown key and no unknown section"
    );
    assert_eq!(
        Config::load(Some(&home.config_path()))
            .expect("reload")
            .remote_hosts
            .default_workspace,
        "Ops",
        "a second start reads the store"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file again"),
        text,
        "the second start changes no byte of the file"
    );
}

#[test]
fn a_store_without_a_copy_keeps_working_and_writes_no_file() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    std::fs::remove_file(home.config_path()).expect("remove the copy");

    let loaded = home.load().expect("load without any copy");

    assert_eq!(loaded.window.width, 1010.0, "the store still answers");
    assert_eq!(home.names().len(), 1, "a start with no copy writes nothing");
    assert!(home.store_path().is_file(), "the store is the only file");
}

#[test]
fn a_damaged_store_is_repaired_from_the_copy() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    std::fs::write(home.store_path(), b"not a database").expect("damage the store");

    let loaded = home.load().expect("a damaged store reads its copy");

    assert_eq!(loaded.window.width, 1010.0, "the copy carries the settings");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the copy"),
        FIRST,
        "the repair changes no byte of the copy"
    );
    assert_eq!(
        ConfigStore::resolve(&home.config_path())
            .load()
            .expect("store")
            .expect("the store exists again")
            .config
            .window
            .width,
        1010.0,
        "the store holds the settings again"
    );
    assert!(
        home.names().iter().any(|name| name.contains("damaged-")),
        "the damaged file stays next to the store for a person to examine"
    );
}

#[test]
fn a_damaged_store_without_a_copy_is_an_error_and_not_a_default() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    std::fs::remove_file(home.config_path()).expect("remove the copy");
    std::fs::write(home.store_path(), b"not a database").expect("damage the store");

    let error = home.load().expect_err("nothing carries the settings");

    assert!(
        error.to_string().contains("settings store") && error.to_string().contains("damaged"),
        "the error names the store and the reason: {error}"
    );
    assert!(
        !home.names().iter().any(|name| name.contains("damaged-")),
        "a damaged store with no copy is never moved, so a person can examine it"
    );
}

#[test]
fn a_store_of_another_version_with_no_copy_stops_the_start() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    std::fs::remove_file(home.config_path()).expect("remove the copy");
    let connection = rusqlite::Connection::open(home.store_path()).expect("store");
    connection
        .execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '99') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .expect("write a newer schema version");
    drop(connection);
    let store_before = std::fs::read(home.store_path()).expect("read the store");

    let error = home.load().expect_err("a store of another version is refused");

    assert!(
        error.to_string().contains("schema 99"),
        "the error names both versions: {error}"
    );
    assert_eq!(
        std::fs::read(home.store_path()).expect("reread the store"),
        store_before,
        "a store of another version is never rewritten"
    );
    assert!(
        home.names().iter().all(|name| name != "config.json"),
        "no copy is written"
    );
}

#[test]
fn a_store_of_another_version_reads_a_copy_without_writing_anything() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    let connection = rusqlite::Connection::open(home.store_path()).expect("store");
    connection
        .execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '99') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .expect("write a newer schema version");
    drop(connection);
    let store_before = std::fs::read(home.store_path()).expect("read the store");
    home.write_copy(FIRST);

    let loaded = home.load().expect("the copy carries the settings of the person");

    assert_eq!(
        loaded.window.width, 1010.0,
        "the start uses the copy, not the default settings"
    );
    assert_eq!(
        std::fs::read(home.store_path()).expect("reread the store"),
        store_before,
        "a store of another version is never rewritten"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("reread the copy"),
        FIRST,
        "the start that refuses the store writes no copy either"
    );

    let error = Config::save(&home.config_path(), &loaded).expect_err("a save is refused");

    assert!(
        error.to_string().contains("schema 99"),
        "the error names both versions: {error}"
    );
    assert_eq!(
        std::fs::read(home.store_path()).expect("reread the store"),
        store_before,
        "a refused save leaves the store alone"
    );
}

#[test]
fn a_named_configuration_that_exists_nowhere_is_an_error() {
    let home = Home::new();

    let error = home
        .load()
        .expect_err("an isolated instance must not use the user's settings");

    assert!(
        error.to_string().contains("no settings store or configuration file"),
        "{error}"
    );
    assert!(home.names().is_empty(), "a refused load writes nothing");
}

#[test]
fn a_save_writes_the_store_and_no_file() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 640.0;

    Config::save(&home.config_path(), &config).expect("save");

    assert!(
        !home.config_path().exists(),
        "a save writes no settings file of its own"
    );
    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(stored.config.window.height, 640.0, "the store holds the save");
    assert_eq!(
        Config::load(Some(&home.config_path())).expect("reload").window.height,
        640.0,
        "a reload reads the save"
    );
    assert_eq!(home.names(), vec!["settings.sqlite"], "one file holds the settings");
}

/// A settings file that lies beside the store is not a settings source, and a
/// start leaves it exactly as the person left it: no backup, no rewrite.
#[test]
fn a_settings_file_beside_the_store_does_not_change_the_settings() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 700.0;
    Config::save(&home.config_path(), &config).expect("save");
    let mut edited = config.clone();
    edited.window.height = 900.0;
    let document = edited.to_json().expect("serialize");
    std::fs::write(home.config_path(), &document).expect("edit the file");

    let loaded = home.load().expect("load after the edit");

    assert_eq!(
        loaded.window.height, 700.0,
        "a setting changes in the Settings window, not in a file beside the store"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file again"),
        document,
        "the file stays where the person left it"
    );
    assert_eq!(home.names().len(), 2, "a start adds no third file");
}

#[test]
fn a_legacy_file_reaches_the_store_at_the_current_version() {
    let home = Home::new();
    let text = r#"{"shortcuts": {"command_palette": "Alt+K"}}"#;
    home.write_copy(text);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION);
    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(
        stored.config.version, CURRENT_CONFIG_VERSION,
        "the store holds the migrated version, not the file's"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        text,
        "the migrated file is exactly the text that the person wrote"
    );
}

#[test]
fn the_store_of_a_smoke_configuration_is_separate() {
    let home = Home::new();
    let smoke = home.root.path().join("smoke.json");
    std::fs::write(&smoke, FIRST).expect("write the smoke configuration");

    Config::load(Some(&smoke)).expect("load the smoke configuration");

    assert!(
        home.root.path().join("smoke.settings.sqlite").is_file(),
        "another configuration in one folder keeps its own store"
    );
    assert!(
        !home.store_path().exists(),
        "the smoke run does not write the store of the usual configuration"
    );
    assert_eq!(
        std::fs::read_to_string(&smoke).expect("read the smoke configuration"),
        FIRST,
        "the smoke settings file stays exactly as it is"
    );
}

#[test]
fn a_server_section_of_a_copy_reaches_the_store_and_a_bad_one_is_refused() {
    let home = Home::new();
    home.write_copy(
        r#"{
  "version": 11,
  "server": {
    "ssh_target": "server-vm",
    "command": "/Users/x/my_tools/horizon --server-link"
  }
}"#,
    );

    let loaded = home.load().expect("load");
    assert_eq!(loaded.server.ssh_target(), Some("server-vm"));
    assert_eq!(loaded.server.command(), "/Users/x/my_tools/horizon --server-link");
    assert!(loaded.server.names_server());
    let stored = Config::read(&home.config_path())
        .expect("read the store")
        .expect("the imported settings");
    assert_eq!(stored.server, loaded.server, "the section travels through the store");

    let other = Home::new();
    other.write_copy(
        r#"{
  "version": 11,
  "server": {"ssh_target": "-oProxyCommand=evil"}
}"#,
    );
    let error = other.load().expect_err("a target that ssh reads as an option");
    assert!(
        matches!(error, crate::error::Error::Config(ref message) if message.contains("server.ssh_target")),
        "{error}"
    );
    assert!(
        !other.store_path().exists(),
        "a refused setting writes no store, so the next start reads the copy again"
    );
}
