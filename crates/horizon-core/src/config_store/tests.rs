use std::path::{Path, PathBuf};

use super::{ConfigStore, META_SAVED_AT, META_SCHEMA_VERSION, SETTINGS_SCHEMA_VERSION, set_meta};
use crate::Config;

fn store_in(root: &Path) -> ConfigStore {
    ConfigStore::resolve(&root.join("config.json"))
}

#[test]
fn the_store_sits_next_to_its_configuration() {
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/config.json")).path(),
        Path::new("/tmp/a/settings.sqlite")
    );
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/horizon.json")).path(),
        Path::new("/tmp/a/settings.sqlite")
    );
    assert_eq!(
        ConfigStore::resolve(Path::new("/tmp/a/smoke.json")).path(),
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
fn every_section_round_trips() {
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

    store.save(&config).expect("save");
    let stored = store.load().expect("load").expect("the store exists");

    assert_eq!(stored.config.to_json().expect("json"), config.to_json().expect("json"));
}

#[test]
fn every_field_of_the_settings_window_round_trips() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    let mut config = Config::default();
    // The settings that a person changes in the window, one value each.
    config.window.height = 900.0;
    config.appearance.theme = crate::AppearanceTheme::Light;
    config.features.sidebar_accordion = true;
    config.features.notifications = crate::notify::NotificationSetting::NeedsYou;
    config.overlays.minimap_width = 400.0;
    config.browser_host.ssh_target = Some("browser-vm".to_string());
    config.browser_host.profile = Some("work".to_string());
    config.server.ssh_target = Some("server-vm".to_string());
    config.server.command = Some("/opt/horizon --server-link".to_string());
    config.secrets.names = vec!["HETZNER_TOKEN".to_string(), "CLOUD_KEY".to_string()];
    config.remote_hosts.default_workspace = "Ops".to_string();
    config.remote_hosts.vnc_port = 5997;
    config.remote_hosts.vnc_ports.insert("smoke-node".to_string(), 5901);
    config.browser.command = Some("/opt/chromium".to_string());
    config.browser.firefox_command = Some("/opt/firefox".to_string());
    config.browser.geckodriver_command = Some("/opt/geckodriver".to_string());
    config.browser.safaridriver_command = Some("/opt/safaridriver".to_string());
    config.browser.quality = 42;
    config.browser.every_nth_frame = 3;
    config.browser.video.quality = 77;
    config.browser.video.fps = 7;
    config.browser.video.compression_level = 6;
    config.browser.video.max_width = Some(1280);
    config.browser.video.max_file_bytes = 64 * 1024 * 1024;
    config.browser.webauthn_command = vec![
        "/opt/my authenticator".to_string(),
        "--dir".to_string(),
        "/var/tmp/auth".to_string(),
    ];

    store.save(&config).expect("save");
    let stored = store.load().expect("load").expect("the store exists").config;

    assert_eq!(
        stored.features.notifications,
        crate::notify::NotificationSetting::NeedsYou,
        "the value of a desktop notification"
    );
    assert_eq!(stored.remote_hosts, config.remote_hosts, "every remote host entry");
    assert_eq!(stored.secrets, config.secrets, "the names of the named secrets");
    assert_eq!(stored.browser.quality, 42, "the picture quality");
    assert_eq!(stored.browser.every_nth_frame, 3, "the skipped frames");
    assert_eq!(stored.browser.video, config.browser.video, "every recording option");
    assert_eq!(
        stored.browser.webauthn_command,
        vec![
            "/opt/my authenticator".to_string(),
            "--dir".to_string(),
            "/var/tmp/auth".to_string()
        ],
        "an argument with a space stays one argument"
    );
    assert_eq!(stored.browser.command.as_deref(), Some("/opt/chromium"));
    assert_eq!(stored.browser.firefox_command.as_deref(), Some("/opt/firefox"));
    assert_eq!(stored.browser.geckodriver_command.as_deref(), Some("/opt/geckodriver"));
    assert_eq!(
        stored.browser.safaridriver_command.as_deref(),
        Some("/opt/safaridriver")
    );
}

#[test]
fn a_section_that_this_build_does_not_know_survives_a_save() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    let config = Config::default();
    store.save(&config).expect("save");
    let connection = rusqlite::Connection::open(store.path()).expect("open");
    connection
        .execute(
            "INSERT INTO settings (section, format, data, updated_at) VALUES ('later', 'json', '{}', 1)",
            [],
        )
        .expect("a section of a newer build");
    drop(connection);

    store.save(&config).expect("second save");

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
    store.save(&Config::default()).expect("save");

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
    store.save(&Config::default()).expect("save");
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
    store.save(&Config::default()).expect("save");
    let connection = rusqlite::Connection::open(store.path()).expect("open");
    connection
        .execute("UPDATE settings SET format = 'toml' WHERE section = 'window'", [])
        .expect("another format");
    drop(connection);

    let error = store.load().expect_err("a format this build cannot read is refused");

    assert!(error.to_string().contains("format toml"), "{error}");
}

#[test]
fn a_saved_file_is_written_where_the_store_says() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = ConfigStore::at(root.path().join("elsewhere").join("mine.sqlite"));

    store.save(&Config::default()).expect("save");

    assert!(PathBuf::from(store.path()).is_file());
    let stored = store.load().expect("load").expect("exists");
    assert_eq!(stored.config.window.width, Config::default().window.width);
    assert!(
        super::meta(&rusqlite::Connection::open(store.path()).expect("open"), META_SAVED_AT)
            .expect("meta")
            .is_some(),
        "the store answers after its folder was created"
    );
}

/// A store that an older build wrote still holds a `mirror_sha256` row. This
/// build writes no exported copy, so it reads the settings and leaves the row
/// where it is: nothing of an older Horizon is deleted behind its back.
#[test]
fn a_store_that_still_holds_the_export_hash_is_read_and_the_row_stays() {
    let root = tempfile::tempdir().expect("temp dir");
    let store = store_in(root.path());
    let mut config = Config::default();
    config.window.height = 905.0;
    store.save(&config).expect("save");
    {
        let connection = rusqlite::Connection::open(store.path()).expect("open");
        set_meta(&connection, "mirror_sha256", "abc123").expect("the row of an older build");
    }

    let stored = store.load().expect("a store of an older build is not an error");
    assert_eq!(stored.expect("settings").config.window.height, 905.0);

    store.save(&config).expect("a save of this build");
    assert_eq!(
        super::meta(
            &rusqlite::Connection::open(store.path()).expect("open"),
            "mirror_sha256"
        )
        .expect("meta")
        .as_deref(),
        Some("abc123"),
        "the row of an older build stays where it is"
    );
}
