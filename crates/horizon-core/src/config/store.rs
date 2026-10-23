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
fn a_configuration_file_is_imported_one_time() {
    let home = Home::new();
    home.write_copy(FIRST);

    let loaded = home.load().expect("load");

    assert_eq!(loaded.window.width, 1010.0, "the values of the file arrive");
    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Light);
    assert!(home.store_path().is_file(), "the store is written next to the file");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        FIRST,
        "an import does not change the words of the person"
    );
    assert!(
        backups(&home).is_empty(),
        "an import replaces nothing, so it keeps nothing"
    );

    let again = home.load().expect("second load");

    assert_eq!(again.window.width, 1010.0);
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        FIRST,
        "a later load changes nothing either"
    );
}

#[test]
fn an_edited_copy_is_imported_at_the_next_load() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");

    home.write_copy(SECOND);
    let loaded = home.load().expect("load after the edit");

    assert_eq!(loaded.window.height, 880.0, "the edit wins");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        SECOND,
        "the edited text stays as the person wrote it"
    );
    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(stored.config.window.height, 880.0, "the store holds the edit");
    assert!(
        backups(&home).is_empty(),
        "an edit replaces nothing, so it keeps nothing"
    );
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
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        text,
        "an import loses no comment, no unknown key and no unknown section"
    );
    assert_eq!(
        Config::load(Some(&home.config_path()))
            .expect("reload")
            .remote_hosts
            .default_workspace,
        "Ops",
        "a second start reads the store and still leaves the text alone"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        text,
        "the second start loses nothing either"
    );
}

#[test]
fn a_store_without_its_copy_keeps_working_and_gets_a_new_copy() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    std::fs::remove_file(home.config_path()).expect("remove the copy");

    let loaded = home.load().expect("load without a copy");

    assert_eq!(loaded.window.width, 1010.0, "the store still answers");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        loaded.to_yaml().expect("yaml"),
        "the copy comes back from the store"
    );
}

#[test]
fn a_damaged_store_is_repaired_from_its_copy() {
    let home = Home::new();
    home.write_copy(FIRST);
    home.load().expect("first load");
    let exported = std::fs::read_to_string(home.config_path()).expect("read");
    std::fs::write(home.store_path(), b"not a database").expect("damage the store");

    let loaded = home.load().expect("a damaged store reads its copy");

    assert_eq!(loaded.window.width, 1010.0, "the copy carries the settings");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        exported,
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
        std::fs::read_dir(home.root.path()).expect("home").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .contains("damaged-")),
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
        std::fs::read_dir(home.root.path()).expect("home").all(|entry| !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .contains("damaged-")),
        "a damaged store with no copy is never moved, so a person can examine it"
    );
}

#[test]
fn a_store_of_another_version_is_refused_and_not_replaced_by_its_copy() {
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
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("reread the copy"),
        FIRST,
        "a refused save leaves the copy alone"
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
fn a_save_writes_the_copy_and_the_store_together() {
    let home = Home::new();
    let mut config = Config::default();
    config.window.height = 640.0;

    let yaml = Config::save(&home.config_path(), &config).expect("save");

    assert_eq!(std::fs::read_to_string(home.config_path()).expect("read"), yaml);
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
    assert!(backups(&home).is_empty(), "a save imports nothing");
}

#[test]
fn a_saved_copy_keeps_its_comments_and_the_keys_this_build_does_not_model() {
    let home = Home::new();
    let text = "\
version: 11 # keep this comment
remote_hosts:
  default_workspace: Ops
  future_key: true
workspaces: []
";

    Config::save_copy(&home.config_path(), text).expect("save");

    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        text,
        "the copy is the text of the person"
    );
    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(
        stored.config.remote_hosts.default_workspace, "Ops",
        "the store holds the section that the text parses to"
    );
    assert!(
        !backups(&home).contains(&"config.yaml".to_string()),
        "a save imports nothing"
    );
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
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read"),
        loaded.to_yaml().expect("yaml"),
        "the copy is the migrated text"
    );
    let kept = backups(&home);
    assert_eq!(kept.len(), 1, "the text before the migration is kept");
    assert_eq!(
        std::fs::read_to_string(home.root.path().join(&kept[0])).expect("read the kept text"),
        "shortcuts:\n  command_palette: Alt+K\n",
        "the kept text is exactly the text that the person wrote"
    );
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
}
