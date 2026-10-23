use std::path::{Path, PathBuf};

use super::{ConfigStore, META_MIRROR_SHA256, META_SCHEMA_VERSION, SETTINGS_SCHEMA_VERSION, set_meta};
use crate::Config;

fn store_in(root: &Path) -> ConfigStore {
    ConfigStore::resolve(&root.join("config.yaml"))
}

#[test]
fn the_store_sits_next_to_its_configuration() {
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/config.yaml")).path(),
        Path::new("/tmp/a/settings.sqlite")
    );
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/horizon.yaml")).path(),
        Path::new("/tmp/a/settings.sqlite")
    );
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/smoke.yml")).path(),
        Path::new("/tmp/a/smoke.settings.sqlite"),
        "a second configuration in one folder keeps its own store"
    );
}

#[test]
fn an_absent_store_has_no_settings() {
    let root = tempfile::tempdir().expect("temp dir");

    assert!(
        store_in(root.path()).load().expect("load").is_none(),
        "a missing file is not an error"
    );
}

#[test]
fn every_section_and_the_export_hash_round_trip() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    let mut config = Config::default();
    config.window.height = 900.0;
    config.presets.push(crate::config::PresetConfig {
        name: "Mine".to_string(),
        alias: Some("m".to_string()),
        kind: crate::panel::PanelKind::Shell,
        command: None,
        args: Vec::new(),
        resume: crate::panel::PanelResume::Fresh,
        ssh_connection: None,
    });

    store.save(&config, Some("abc123")).expect("save");
    let stored = store.load().expect("load").expect("the store exists");

    assert_eq!(stored.config.to_yaml().expect("yaml"), config.to_yaml().expect("yaml"));
    assert_eq!(stored.mirror_sha256.as_deref(), Some("abc123"));

    store.save(&config, None).expect("a save with no copy");
    assert_eq!(
        store.load().expect("load").expect("exists").mirror_sha256,
        None,
        "a store with no exported copy records no hash"
    );
}

#[test]
fn a_section_that_this_build_does_not_know_survives_a_save() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    let config = Config::default();
    store.save(&config, None).expect("save");
    let connection = rusqlite::Connection::open(store.path()).expect("open");
    connection
        .execute(
            "INSERT INTO settings (section, format, data, updated_at) VALUES ('later', 'json', '{}', 1)",
            [],
        )
        .expect("a section of a newer build");
    drop(connection);

    store.save(&config, None).expect("second save");

    let connection = rusqlite::Connection::open(store.path()).expect("open");
    let kept: i64 = connection
        .query_row("SELECT COUNT(*) FROM settings WHERE section = 'later'", [], |row| {
            row.get(0)
        })
        .expect("count");
    assert_eq!(kept, 1, "an unknown section is kept");
}

#[test]
fn the_store_is_one_file_at_rest() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    store.save(&Config::default(), Some("hash")).expect("save");

    let names: Vec<String> = std::fs::read_dir(root.path())
        .expect("read dir")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        names,
        vec!["settings.sqlite".to_string()],
        "no journal or write-ahead file stays"
    );
}

#[test]
fn a_file_that_is_not_a_store_is_refused_with_its_path() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    std::fs::write(store.path(), b"this is not a database").expect("write rubbish");

    let error = store.load().expect_err("a corrupt file is not empty settings");

    assert!(
        error.to_string().contains(&store.path().display().to_string()),
        "the error names the store: {error}"
    );
}

#[test]
fn a_store_of_another_schema_is_refused() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    store.save(&Config::default(), None).expect("save");
    let connection = rusqlite::Connection::open(store.path()).expect("open");
    set_meta(&connection, META_SCHEMA_VERSION, "99").expect("a newer schema");
    drop(connection);

    let error = store.load().expect_err("a newer schema is refused");

    assert!(
        error.to_string().contains("schema 99"),
        "the error names both schemas: {error}"
    );
    assert_eq!(SETTINGS_SCHEMA_VERSION, 1);
}

#[test]
fn a_row_of_another_format_is_refused() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    store.save(&Config::default(), None).expect("save");
    let connection = rusqlite::Connection::open(store.path()).expect("open");
    connection
        .execute("UPDATE settings SET format = 'yaml' WHERE section = 'window'", [])
        .expect("another format");
    drop(connection);

    let error = store.load().expect_err("a format this build cannot read is refused");

    assert!(error.to_string().contains("format yaml"), "{error}");
}

#[test]
fn a_saved_file_is_written_where_the_store_says() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = ConfigStore::at(root.path().join("elsewhere").join("mine.sqlite"));

    store.save(&Config::default(), None).expect("save");

    assert!(PathBuf::from(store.path()).is_file());
    assert_eq!(
        store.load().expect("load").expect("exists").mirror_sha256,
        None,
        "the store answers after its folder was created"
    );
    assert_eq!(
        super::meta(
            &rusqlite::Connection::open(store.path()).expect("open"),
            META_MIRROR_SHA256
        )
        .expect("meta"),
        None
    );
}
