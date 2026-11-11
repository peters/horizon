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

    /// A start that owns this home and names no settings file.
    fn load_from_home(&self) -> crate::error::Result<Config> {
        Config::load_from_home(&self.config_path())
    }

    /// The YAML settings file that a release up to v0.2.7 kept in this home.
    fn write_old_settings(&self, text: &str) -> PathBuf {
        let path = self.root.path().join("config.yaml");
        std::fs::write(&path, text).expect("write the YAML settings file");
        path
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

/// A document that reads as neither format is refused with the format of this
/// build in the message, and nothing is written in its place.
#[test]
fn a_settings_file_that_reads_as_neither_format_is_refused_with_it() {
    let home = Home::new();
    let broken = "{ this is not a settings document\n";
    home.write_copy(broken);

    let error = home.load().expect_err("a document of no format is not a settings file");

    assert!(
        error.to_string().contains("JSON"),
        "the message names the format: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        broken,
        "the refused file keeps every byte"
    );
    assert!(!home.store_path().exists(), "a refused file writes no store");
}

/// The format of a settings document is its content. A YAML mapping that a run
/// names is imported, which is the rule of `--config legacy.yaml`.
#[test]
fn a_yaml_document_at_the_settings_path_is_imported_by_its_content() {
    let home = Home::new();
    let old = "version: 11\nwindow:\n  width: 1010\n";
    home.write_copy(old);

    let loaded = home.load().expect("a YAML document is imported");

    assert_eq!(loaded.window.width, 1010.0);
    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION);
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the file"),
        old,
        "the imported document keeps every byte"
    );
    assert!(home.store_path().is_file(), "the import writes the store");
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

/// The two presets that the last release shipped for the panels that this
/// release removed, and the Markdown preset that stays.
const COPY_WITH_REMOVED_PRESETS: &str = r#"{
  "version": 11,
  "presets": [
    {"name": "Git Changes", "alias": "gc", "kind": "git_changes"},
    {"name": "Markdown", "alias": "md", "kind": "editor"},
    {"name": "Usage", "alias": "u", "kind": "usage"}
  ]
}"#;

fn preset_names(config: &Config) -> Vec<&str> {
    config.presets.iter().map(|preset| preset.name.as_str()).collect()
}

#[test]
fn a_copy_with_a_preset_of_a_removed_panel_kind_loses_it_and_keeps_the_rest() {
    let home = Home::new();
    home.write_copy(COPY_WITH_REMOVED_PRESETS);

    let loaded = home
        .load()
        .expect("a preset of a removed kind must not refuse the settings");

    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION, "the migration ran");
    assert_eq!(preset_names(&loaded), vec!["Markdown"], "the other presets stay");
    assert_eq!(
        std::fs::read_to_string(home.config_path()).expect("read the copy"),
        COPY_WITH_REMOVED_PRESETS,
        "the imported file keeps every byte"
    );

    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(
        stored.config.version, CURRENT_CONFIG_VERSION,
        "the store holds the migrated version"
    );
    assert_eq!(
        preset_names(&stored.config),
        vec!["Markdown"],
        "the store keeps no preset that opens no panel"
    );
}

#[test]
fn a_store_with_a_preset_of_a_removed_panel_kind_loads_and_is_written_again() {
    let home = Home::new();
    home.write_copy(r#"{"version": 11, "window": {"width": 1010}}"#);
    home.load().expect("first load, which writes the store");
    let store = ConfigStore::resolve(&home.config_path());
    // The presets row as the last release wrote it, with the two dead presets.
    let connection = rusqlite::Connection::open(store.path()).expect("open the store");
    let presets = r#"[{"name":"Git Changes","alias":"gc","kind":"git_changes"},{"name":"Markdown","alias":"md","kind":"editor"},{"name":"Usage","alias":"u","kind":"usage"}]"#;
    connection
        .execute("UPDATE settings SET data = ?1 WHERE section = 'presets'", [presets])
        .expect("the presets of the last release");
    connection
        .execute("UPDATE settings SET data = '11' WHERE section = 'version'", [])
        .expect("the version of the last release");
    drop(connection);

    let loaded = home
        .load()
        .expect("a store with a preset of a removed kind still loads");

    assert_eq!(loaded.window.width, 1010.0, "the other settings stay");
    assert_eq!(preset_names(&loaded), vec!["Markdown"]);
    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION);

    let stored = ConfigStore::resolve(&home.config_path())
        .load()
        .expect("store")
        .expect("the store exists");
    assert_eq!(
        preset_names(&stored.config),
        vec!["Markdown"],
        "the start wrote the store again"
    );
    assert_eq!(stored.config.version, CURRENT_CONFIG_VERSION);
}

/// The YAML settings file that release v0.2.7 wrote when a person ran it with
/// no settings at all, cut down to the sections of this test. The full files of
/// that release and of `main` are the fixtures of `tests/upgrade/`.
const YAML_OF_THE_RELEASE: &str = "\
version: 8
window:
  width: 1234.0
  height: 777.0
  x: null
  y: null
appearance:
  theme: dark
overlays:
  attention_feed_height: 321.0
  attention_feed_width: 210.0
  minimap_height: 200.0
  minimap_width: 400.0
features:
  attention_feed: false
  speech:
    enabled: true
    model: /home/user/models/whisper-small.bin
    hotkey: F10
presets:
- name: Shell
  alias: sh
  kind: shell
  command: /bin/zsh
  args: []
  resume: fresh
- name: Git Changes
  alias: gc
  kind: git_changes
  command: null
  args: []
  resume: fresh
- name: Markdown
  alias: md
  kind: editor
  command: null
  args: []
  resume: fresh
- name: Usage
  alias: u
  kind: usage
  command: null
  args: []
  resume: fresh
workspaces: []
";

#[test]
fn the_yaml_settings_file_of_an_older_horizon_reaches_the_store() {
    let home = Home::new();
    let yaml = home.write_old_settings(YAML_OF_THE_RELEASE);

    let loaded = home.load_from_home().expect("the import of the YAML file");

    assert_eq!(
        loaded.appearance.theme,
        crate::AppearanceTheme::Dark,
        "a value that this build still has arrives unchanged"
    );
    assert_eq!(loaded.window.width, 1234.0);
    assert_eq!(loaded.window.height, 777.0);
    assert_eq!(loaded.overlays.minimap_width, 400.0);
    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION, "the migrations ran");
    assert_eq!(
        preset_names(&loaded),
        vec!["Shell", "Markdown", "Grok", "Browser"],
        "the presets of the removed panels are gone, the others stay, and the migrations add the \
         two presets of the versions between"
    );
    assert_eq!(
        std::fs::read_to_string(&yaml).expect("read the YAML file"),
        YAML_OF_THE_RELEASE,
        "the file stays exactly where the person left it, with every byte"
    );
    assert!(
        !home.config_path().exists(),
        "no settings file of this build is written beside the store"
    );
    assert!(home.store_path().is_file(), "the import writes the store");
}

#[test]
fn a_start_that_finds_the_store_never_reads_the_yaml_file() {
    let home = Home::new();
    let yaml = home.write_old_settings(YAML_OF_THE_RELEASE);
    home.load_from_home().expect("the import of the YAML file");
    std::fs::remove_file(&yaml).expect("remove the YAML file");

    let second = home.load_from_home().expect("the second start reads the store");

    assert_eq!(
        second.appearance.theme,
        crate::AppearanceTheme::Dark,
        "the settings stay after the file is gone, so the store carries them"
    );
    assert_eq!(home.names().len(), 1, "the home holds the store and nothing else");
}

#[test]
fn a_yaml_file_beside_a_store_is_not_read() {
    let home = Home::new();
    home.write_copy(FIRST);
    let first = home.load_from_home().expect("a settings file reaches the store");
    assert_eq!(first.appearance.theme, crate::AppearanceTheme::Light);
    home.write_old_settings("version: 8\nappearance:\n  theme: dark\n");

    let second = home.load_from_home().expect("the start of a home with a store");

    assert_eq!(
        second.appearance.theme,
        crate::AppearanceTheme::Light,
        "the store is the settings, and the YAML file of an older Horizon is not read"
    );
    assert_eq!(home.names().len(), 3, "the files of the home stay as they are");
}

#[test]
fn a_file_that_an_import_moved_aside_is_never_read() {
    let home = Home::new();
    let moved = home.root.path().join("config.yaml.imported-1792777304");
    std::fs::write(&moved, YAML_OF_THE_RELEASE).expect("write the moved file");

    let loaded = home.load_from_home().expect("a home with no settings at all");

    assert_eq!(
        loaded.appearance.theme,
        crate::AppearanceTheme::Auto,
        "a file that an earlier import moved aside is not read"
    );
    assert!(!home.store_path().exists(), "the defaults write no store");
}

#[test]
fn a_yaml_file_that_cannot_be_read_stops_the_start_and_writes_no_store() {
    let home = Home::new();
    let broken = "version: 8\nappearance:\n  theme: dark\n   theme: light\n";
    let yaml = home.write_old_settings(broken);

    let error = home.load_from_home().expect_err("a broken YAML file stops the start");

    let message = error.to_string();
    assert!(
        message.contains(&yaml.display().to_string()),
        "the sentence names the file: {message}"
    );
    assert!(message.contains("line"), "the sentence names the line: {message}");
    assert!(!home.store_path().exists(), "the defaults never replace the file");
    assert_eq!(
        std::fs::read_to_string(&yaml).expect("read the YAML file"),
        broken,
        "a file that cannot be read stays exactly as it is"
    );
}

#[test]
fn a_yaml_file_of_a_newer_configuration_version_stops_the_start() {
    let home = Home::new();
    let text = format!("version: {}\nappearance:\n  theme: dark\n", CURRENT_CONFIG_VERSION + 1);
    let yaml = home.write_old_settings(&text);

    let error = home
        .load_from_home()
        .expect_err("a version above this build stops the start");

    let message = error.to_string();
    assert!(message.contains(&yaml.display().to_string()), "{message}");
    assert!(
        message.contains(&format!("newer Horizon, with version {}", CURRENT_CONFIG_VERSION + 1)),
        "{message}"
    );
    assert!(
        !home.store_path().exists(),
        "no store is written from a version above this build"
    );
}

#[test]
fn a_named_yaml_settings_file_is_imported_one_time() {
    let home = Home::new();
    let path = home.root.path().join("legacy.yaml");
    std::fs::write(&path, YAML_OF_THE_RELEASE).expect("write the named YAML file");

    let loaded = Config::load(Some(&path)).expect("a named YAML file is imported");

    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Dark);
    assert!(
        home.root.path().join("legacy.settings.sqlite").is_file(),
        "the store sits beside the named file"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read the named file"),
        YAML_OF_THE_RELEASE,
        "the named file keeps every byte"
    );
}

/// The store that `main` wrote, built here from its own statements:
///
/// - `crates/horizon-core/src/config_store/schema.rs` of `origin/main`:
///   `CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;`
///   `CREATE TABLE IF NOT EXISTS settings (section TEXT PRIMARY KEY, format TEXT NOT NULL, data
///   TEXT NOT NULL, updated_at INTEGER NOT NULL) STRICT;`
/// - `config_store/mod.rs` of `origin/main`: the meta rows `schema_version` (`1`),
///   `mirror_sha256` and `saved_at`.
/// - `config_store/sections.rs` of `origin/main`: one row for each of its sections,
///   `format = "json"`, in the order `version`, `window`, `appearance`, `shortcuts`,
///   `overlays`, `features`, `browser`, `remote_hosts`, `browser_host`, `presets`,
///   `workspaces`. It has no `server` and no `secrets` row, because those sections
///   came after it.
///
/// A row of a section that this build does not know is added as well, as a newer
/// build would write it.
fn write_main_store(path: &std::path::Path, sections: &[(&str, &str)], mirror: &str) {
    let connection = rusqlite::Connection::open(path).expect("create the store of main");
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             ) STRICT;
             CREATE TABLE IF NOT EXISTS settings (
                 section TEXT PRIMARY KEY,
                 format TEXT NOT NULL,
                 data TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             ) STRICT;",
        )
        .expect("the tables of main");
    for (key, value) in [
        ("schema_version", "1"),
        ("mirror_sha256", mirror),
        ("saved_at", "1792777304000"),
    ] {
        connection
            .execute("INSERT INTO meta (key, value) VALUES (?1, ?2)", [key, value])
            .expect("a meta row of main");
    }
    for (section, data) in sections {
        connection
            .execute(
                "INSERT INTO settings (section, format, data, updated_at) VALUES (?1, 'json', ?2, 1792777304000)",
                [section, data],
            )
            .expect("a section of main");
    }
}

/// The sections of the last `main` build, with a value that is not the default
/// and the sections of the features that this build removed.
fn sections_of_main() -> Vec<(&'static str, &'static str)> {
    vec![
        ("version", "11"),
        ("window", r#"{"width":1350.0,"height":860.0,"x":71.0,"y":39.0}"#),
        ("appearance", r#"{"theme":"dark"}"#),
        (
            "shortcuts",
            r#"{"command_palette":"Alt+K","new_terminal":"Ctrl+Shift+N","focus_active_workspace":"Ctrl+Shift+W","fit_active_workspace":"Ctrl+Shift+9","open_remote_hosts":"Ctrl+Shift+H","toggle_sessions":"Ctrl+Shift+J","toggle_sidebar":"Ctrl+Shift+B","toggle_hud":"Ctrl+Shift+U","toggle_minimap":"Ctrl+Shift+M","align_workspaces_horizontally":"Ctrl+Shift+A","toggle_settings":"Ctrl+Shift+Comma","zoom_reset":"Ctrl+0","zoom_in":"Ctrl+Plus","zoom_out":"Ctrl+Minus","fullscreen_panel":"F11","exit_fullscreen_panel":"Escape","fullscreen_window":"Ctrl+Shift+F11","save_editor":"Ctrl+Shift+S","search":"Ctrl+Shift+F"}"#,
        ),
        ("overlays", r#"{"minimap_height":240.0,"minimap_width":360.0}"#),
        (
            "features",
            r#"{"organize_workspaces_on_session_load":true,"sidebar_accordion":true,"speech":{"enabled":true,"model":"/home/user/models/whisper-small.bin","desktop_injection":true}}"#,
        ),
        (
            "browser",
            r#"{"backend":"chromium","headless":false,"quality":70,"every_nth_frame":2}"#,
        ),
        (
            "remote_hosts",
            r#"{"default_workspace":"Remote Lab","vnc_port":5901,"vnc_ports":{"lab":5902}}"#,
        ),
        (
            "browser_host",
            r#"{"ssh_target":"browser-vm.example.net","profile":"mail"}"#,
        ),
        (
            "presets",
            r#"[{"name":"Git Changes","alias":"gc","kind":"git_changes","command":null,"args":[],"resume":"fresh"},{"name":"Markdown","alias":"md","kind":"editor","command":null,"args":[],"resume":"fresh"},{"name":"Usage","alias":"u","kind":"usage","command":null,"args":[],"resume":"fresh"}]"#,
        ),
        ("workspaces", "[]"),
        ("a_section_of_a_newer_build", r#"{"keep":"me"}"#),
    ]
}

#[test]
fn the_store_that_main_wrote_opens_with_its_values() {
    let home = Home::new();
    write_main_store(&home.store_path(), &sections_of_main(), "e5555063");

    let loaded = home
        .load_from_home()
        .expect("the store that main wrote opens in this build");

    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION, "the migration ran");
    assert_eq!(loaded.window.width, 1350.0);
    assert_eq!(loaded.window.height, 860.0);
    assert_eq!(loaded.window.x, Some(71.0));
    assert_eq!(loaded.window.y, Some(39.0));
    assert_eq!(loaded.appearance.theme, crate::AppearanceTheme::Dark);
    assert_eq!(loaded.shortcuts.command_palette, "Alt+K");
    assert_eq!(loaded.overlays.minimap_height, 240.0);
    assert_eq!(loaded.overlays.minimap_width, 360.0);
    assert!(loaded.features.organize_workspaces_on_session_load);
    assert!(loaded.features.sidebar_accordion);
    assert_eq!(loaded.browser.quality, 70, "the browser section of main arrives");
    assert!(!loaded.browser.headless);
    assert_eq!(loaded.remote_hosts.default_workspace, "Remote Lab");
    assert_eq!(loaded.remote_hosts.vnc_ports.get("lab"), Some(&5902));
    assert_eq!(
        loaded.browser_host.ssh_target.as_deref(),
        Some("browser-vm.example.net")
    );
    assert_eq!(loaded.browser_host.profile.as_deref(), Some("mail"));
    assert_eq!(
        preset_names(&loaded),
        vec!["Markdown"],
        "the presets of the removed panels are gone and the others stay"
    );
    assert!(
        !loaded.to_json().expect("the settings as JSON").contains("speech"),
        "the speech section of a removed feature is ignored"
    );
}

#[test]
fn a_save_after_the_store_of_main_keeps_every_row_that_this_build_does_not_read() {
    let home = Home::new();
    write_main_store(&home.store_path(), &sections_of_main(), "e5555063");
    let mut loaded = home.load_from_home().expect("the store of main");

    loaded.appearance.theme = crate::AppearanceTheme::Light;
    Config::save(&home.config_path(), &loaded).expect("write the store");

    let connection = rusqlite::Connection::open(home.store_path()).expect("open the store");
    let section = |name: &str| -> Option<String> {
        connection
            .query_row("SELECT data FROM settings WHERE section = ?1", [name], |row| row.get(0))
            .ok()
    };
    assert_eq!(
        section("a_section_of_a_newer_build").as_deref(),
        Some(r#"{"keep":"me"}"#),
        "a row of a section that this build does not know stays as it is"
    );
    assert_eq!(
        section("appearance").as_deref(),
        Some(r#"{"theme":"light"}"#),
        "the save wrote the section that changed"
    );
    let mirror: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key = 'mirror_sha256'", [], |row| {
            row.get(0)
        })
        .ok();
    assert_eq!(
        mirror.as_deref(),
        Some("e5555063"),
        "the meta row of the last export of main stays, so an older Horizon still finds its store"
    );
}
