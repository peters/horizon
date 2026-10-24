use super::*;

const LEGACY: &str = r"
version: 10
remote:
  providers:
    retired_worker:
      unsupported_legacy_field: discard
browser:
  remote:
    providers:
      mobile_grid:
        endpoint: https://grid.example.net/wd/hub
        authentication:
          kind: basic
          username_ref: grid-user
          password_ref: grid-key
        credential_bindings:
          grid-user:
            store: os_keychain
            slot: remote-browser/mobile-grid/username
          grid-key:
            store: session
        limits:
          max_sessions: 2
    targets:
      phone:
        provider: mobile_grid
        browser_name: safari
        platform_name: iOS
presets:
  - name: Ordinary SSH
    kind: ssh
    ssh_connection:
      host: example-host
      port: 2222
workspaces:
  - name: Local work
    terminals:
      - name: Shell
        kind: shell
      - name: Browser
        kind: browser
";

#[test]
fn v11_removes_only_retired_provider_config_and_is_idempotent() {
    let root = tempfile::tempdir().expect("temporary config directory");
    let path = root.path().join("config.yaml");
    std::fs::write(&path, LEGACY).expect("legacy config");
    let mut expected = Config::from_yaml(LEGACY).expect("preserved config is valid");
    expected.version = CURRENT_CONFIG_VERSION;

    let migrated = Config::load(Some(&path)).expect("migration");
    assert_eq!(migrated.to_yaml().unwrap(), expected.to_yaml().unwrap());
    let saved = migrated.to_yaml().unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&saved).unwrap();
    assert!(value.get("remote").is_none());
    assert!(value["browser"].get("remote").is_some());
    assert_eq!(migrated.browser.remote, expected.browser.remote);
    assert_eq!(migrated.workspaces.len(), 1);
    assert_eq!(migrated.workspaces[0].terminals.len(), 2);

    let reloaded = Config::load(Some(&path)).expect("reload migrated config");
    assert_eq!(reloaded.to_yaml().unwrap(), saved);
    let kept = std::fs::read_dir(root.path())
        .expect("the configuration directory")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("config.yaml.imported-"))
        .expect("the imported copy is kept");
    assert_eq!(
        std::fs::read_to_string(root.path().join(kept)).unwrap(),
        LEGACY,
        "the copy that the run imported keeps every byte that the person wrote"
    );
}

#[test]
fn invalid_preserved_browser_config_does_not_rewrite_legacy_file() {
    let root = tempfile::tempdir().expect("temporary config directory");
    let path = root.path().join("config.yaml");
    let invalid = LEGACY.replace(
        "https://grid.example.net/wd/hub",
        "https://user:secret@grid.example.net/wd/hub",
    );
    std::fs::write(&path, &invalid).expect("legacy config");
    assert!(Config::load(Some(&path)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
}
