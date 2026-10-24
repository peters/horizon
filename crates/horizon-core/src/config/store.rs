//! The policy of the settings store: when a copy is imported, and what a person
//! gets when the store or the copy is missing or unreadable.

use std::path::PathBuf;

use crate::config_store::ConfigStore;
use crate::{Config, config_migration::CURRENT_CONFIG_VERSION};

const FIRST: &str = "\
version: 11
window:
  width: 1010
  height: 700
appearance:
  theme: light
";

const SECOND: &str = "\
version: 11
window:
  width: 1010
  height: 880
appearance:
  theme: dark
";

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
        self.root.path().join("config.yaml")
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

fn backups(home: &Home) -> Vec<String> {
    home.names()
        .into_iter()
        .filter(|name| name.contains(".imported-"))
        .collect()
}

#[test]
fn a_configuration_file_is_imported_one_time_and_kept_aside() {
    let home = Home::new();
    home.write_copy(FIRST);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.window.width, 1010.0, "the values of the file arrive");
    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Light);
    assert!(home.store_path().is_file(), "the store is written next to the copy");
    assert!(
        !home.config_path().exists(),
        "the copy stops being a settings file once the store holds its values"
    );
    let kept = backups(&home);
    assert_eq!(kept.len(), 1, "the words of the person are kept exactly once");
    assert_eq!(
        std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept copy"),
        FIRST,
        "every byte of the copy survives the import"
    );

    let again = home.load().expect("second load");

    assert_eq!(again.window.width, 1010.0);
    assert_eq!(home.names().len(), 2, "the second start writes no third file");
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
    assert_eq!(backups(&home).len(), 1, "no second copy is kept");
}

#[test]
fn an_import_keeps_everything_that_this_build_does_not_read() {
    let home = Home::new();
    let text = "\
version: 11 # the theme of this machine
appearance:
  theme: light
remote_hosts:
  default_workspace: Ops
  future_key: true
a_section_of_a_newer_build:
  keep: me
";
    home.write_copy(text);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Light);
    let kept = backups(&home);
    assert_eq!(kept.len(), 1);
    assert_eq!(
        std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept copy"),
        text,
        "an import loses no comment, no unknown key and no unknown section"
    );
    assert_eq!(
        Config::load(Some(&home.config_path()))
            .expect("reload")
            .remote_hosts
            .default_workspace,
        "Ops",
        "a second start reads the store"
    );
    assert_eq!(backups(&home).len(), 1, "the second start keeps nothing new");
}

#[test]
fn a_store_without_a_copy_keeps_working_and_writes_no_file() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    let kept = backups(&home);
    std::fs::remove_file(home.root.path().join(&kept[0])).expect("remove the kept copy");

    let loaded = home.load().expect("load without any copy");

    assert_eq!(loaded.window.width, 1010.0, "the store still answers");
    assert_eq!(home.names().len(), 1, "a start with no copy writes nothing");
    assert!(home.store_path().is_file(), "the store is the only file");
}

#[test]
fn a_damaged_store_is_repaired_from_the_kept_copy() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    let kept = backups(&home);
    let kept_text = std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept copy");
    std::fs::write(home.store_path(), b"not a database").expect("damage the store");

    let loaded = home.load().expect("a damaged store reads its kept copy");

    assert_eq!(loaded.window.width, 1010.0, "the kept copy carries the settings");
    assert_eq!(
        std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept copy"),
        kept_text,
        "the repair changes no byte of the kept copy"
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
    let kept = backups(&home);
    std::fs::remove_file(home.root.path().join(&kept[0])).expect("remove the kept copy");
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
    let kept = backups(&home);
    std::fs::remove_file(home.root.path().join(&kept[0])).expect("remove the kept copy");
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
        home.names().iter().all(|name| name != "config.yaml"),
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
        "a save writes no YAML file; a person exports one on request"
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

#[test]
fn a_saved_text_reaches_the_store_and_writes_no_file() {
    let home = Home::new();
    let text = "\
version: 11 # keep this comment
remote_hosts:
  default_workspace: Ops
workspaces: []
";

    Config::save_text(&home.config_path(), text).expect("save");

    assert!(
        !home.config_path().exists(),
        "the text of the window is stored, not written"
    );
    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(
        stored.config.remote_hosts.default_workspace, "Ops",
        "the store holds the section that the text parses to"
    );
}

#[test]
fn a_saved_text_that_does_not_parse_writes_nothing() {
    let home = Home::new();

    let error = Config::save_text(&home.config_path(), "window: [not a map").expect_err("refuse");

    assert!(error.to_string().contains("Config error"), "{error}");
    assert!(home.names().is_empty(), "a refused text writes no store");
}

#[test]
fn an_export_writes_a_copy_that_leaves_the_settings_alone() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 700.0;
    Config::save(&home.config_path(), &config).expect("save");
    let export = home.root.path().join("exported.yaml");

    Config::export(&home.config_path(), &export).expect("export");

    let text = std::fs::read_to_string(&export).expect("read the export");
    assert!(text.contains("height: 700"), "the export holds the settings");
    assert_eq!(
        Config::load(Some(&home.config_path())).expect("reload").window.height,
        700.0,
        "the next start reads the store"
    );
    assert_eq!(
        std::fs::read_to_string(&export).expect("read the export again"),
        text,
        "a start changes no byte of an export"
    );
}

#[test]
fn an_exported_copy_is_never_taken_as_a_setting_again() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 700.0;
    Config::save(&home.config_path(), &config).expect("save");
    Config::export(&home.config_path(), &home.config_path()).expect("export next to the store");
    let exported = std::fs::read_to_string(home.config_path()).expect("read the export");

    let loaded = home.load().expect("load");

    assert_eq!(loaded.window.height, 700.0, "the store still wins");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the export again"),
        exported,
        "the export of Horizon is not imported, and it is not moved aside"
    );
    assert_eq!(home.names().len(), 2, "an export adds no backup");
}

#[test]
fn an_edited_export_does_not_change_the_settings() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 700.0;
    Config::save(&home.config_path(), &config).expect("save");
    Config::export(&home.config_path(), &home.config_path()).expect("export next to the store");
    let exported = std::fs::read_to_string(home.config_path()).expect("read the export");
    let edited = edited_to(&exported, 900.0);
    std::fs::write(home.config_path(), &edited).expect("edit the export");

    let loaded = home.load().expect("load after the edit");

    assert_eq!(
        loaded.window.height, 700.0,
        "a setting changes in the Settings window, not in an export"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the edited export"),
        edited,
        "the edited file stays where the person left it"
    );
    assert_eq!(backups(&home).len(), 0, "nothing is kept aside");
}

/// `text` with one height, as a person edits a copy in an editor.
fn edited_to(text: &str, height: f32) -> String {
    let mut config = Config::from_yaml(text).expect("parse the copy");
    config.window.height = height;
    config.to_yaml().expect("yaml")
}

#[test]
fn an_export_never_reads_a_copy_before_the_first_start() {
    let home = Home::new();
    home.write_copy(FIRST);

    let error = Config::export(&home.config_path(), &home.config_path()).expect_err("no store, no export");

    assert!(error.to_string().contains("no settings to export"), "{error}");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the copy"),
        FIRST,
        "the hand-written copy keeps its bytes, comments and unknown keys"
    );
    assert!(
        !ConfigStore::resolve(&home.config_path()).path().exists(),
        "an export writes no store"
    );
}

#[test]
fn an_export_with_no_settings_is_an_error() {
    let home = Home::new();
    let export = home.root.path().join("exported.yaml");

    let error = Config::export(&home.config_path(), &export).expect_err("nothing to export");

    assert!(error.to_string().contains("no settings to export"), "{error}");
    assert!(!export.exists(), "a refused export writes no file");
}

#[test]
fn a_legacy_file_reaches_the_store_at_the_current_version() {
    let home = Home::new();
    home.write_copy("shortcuts:\n  command_palette: Alt+K\n");

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
    let kept = backups(&home);
    assert_eq!(kept.len(), 1, "the text before the migration is kept");
    assert_eq!(
        std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept text"),
        "shortcuts:\n  command_palette: Alt+K\n",
        "the kept text is exactly the text that the person wrote"
    );
    assert!(!home.config_path().exists(), "a migration never writes the copy again");
}

#[test]
fn the_store_of_a_smoke_configuration_is_separate() {
    let home = Home::new();
    let smoke = home.root.path().join("smoke.yml");
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
    assert!(!smoke.exists(), "the smoke copy stops being a settings file too");
}
