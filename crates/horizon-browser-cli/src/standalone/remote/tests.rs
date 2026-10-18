use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use horizon_browser::remote::{CredentialReference, RemoteBrowserConfig};

use super::config;
use super::holds::Holds;

const PROVIDERS: &str = "
providers:
  grid:
    adapter: webdriver
    endpoint: https://grid.example.net/wd/hub
    authentication: { kind: basic, username_ref: user, password_ref: key }
targets:
  desktop:
    provider: grid
    browser_name: chrome
    platform_name: linux
    device: { kind: any }
";

fn remote() -> RemoteBrowserConfig {
    serde_yaml::from_str(PROVIDERS).expect("remote configuration")
}

fn private(dir: &Path, name: &str, value: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, value).expect("write credential");
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).expect("private");
    path
}

fn files(user: PathBuf, key: PathBuf) -> BTreeMap<String, BTreeMap<CredentialReference, PathBuf>> {
    let mut references = BTreeMap::new();
    references.insert(CredentialReference::from("user"), user);
    references.insert(CredentialReference::from("key"), key);
    BTreeMap::from([("grid".to_owned(), references)])
}

#[test]
fn credential_files_form_the_shared_header_without_their_final_line_break() {
    let dir = tempfile::tempdir().expect("dir");
    let config = config::for_tests(
        remote(),
        files(
            private(dir.path(), "user", b"alice\n"),
            private(dir.path(), "key", b"s3cret-key\r\n"),
        ),
    )
    .expect("host file");
    let header = config.authorization("grid").expect("header").expect("basic");
    assert_eq!(header.as_str(), "Basic YWxpY2U6czNjcmV0LWtleQ==");
    let catalog = horizon_browser::provider_catalog::CatalogCache::default();
    let request = config.request("desktop", &catalog).expect("request");
    assert_eq!(request.provider, "grid");
    assert_eq!(
        request.quota_key,
        horizon_browser_control::remote_slots::quota_key(&config.remote.providers["grid"])
    );
    let unknown = config.request("phone", &catalog).unwrap_err().to_string();
    assert!(unknown.starts_with("target_unknown: "), "{unknown}");
}

#[test]
fn a_credential_problem_names_the_reference_and_never_the_value() {
    let dir = tempfile::tempdir().expect("dir");
    let user = private(dir.path(), "user", b"al:ice");
    let key = private(dir.path(), "key", b"s3cret-key");
    let config = config::for_tests(remote(), files(user.clone(), key.clone())).expect("host file");
    let refused = config.authorization("grid").unwrap_err().to_string();
    assert!(
        refused.starts_with("credentials_not_ready: provider `grid` credential `user`"),
        "{refused}"
    );
    assert!(!refused.contains("al:ice") && !refused.contains("s3cret"), "{refused}");
    std::fs::write(&user, b"alice").expect("rewrite");
    #[cfg(unix)]
    {
        std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o640)).expect("shared");
        let shared = config.authorization("grid").unwrap_err().to_string();
        assert!(shared.contains("must be readable only by its owner"), "{shared}");
        assert!(!shared.contains("s3cret"), "{shared}");
    }
    std::fs::remove_file(&key).expect("take back");
    let removed = config.authorization("grid").unwrap_err().to_string();
    assert!(
        removed.contains("cannot be opened"),
        "a removed file applies to the next use: {removed}"
    );
    assert!(!removed.contains(dir.path().to_str().expect("path")), "{removed}");
}

#[test]
fn the_host_file_names_exactly_the_files_the_authentication_needs() {
    let key = PathBuf::from("/run/key");
    let user = PathBuf::from("/run/user");
    let mut missing = files(user.clone(), key.clone());
    missing
        .get_mut("grid")
        .expect("grid")
        .remove(&CredentialReference::from("key"));
    let mut extra = files(user.clone(), key.clone());
    extra
        .get_mut("grid")
        .expect("grid")
        .insert(CredentialReference::from("spare"), PathBuf::from("/run/spare"));
    let mut stranger = files(user.clone(), key.clone());
    stranger.insert("other".into(), BTreeMap::new());
    let mut bound = remote();
    bound
        .providers
        .get_mut("grid")
        .expect("grid")
        .credential_bindings
        .insert(
            CredentialReference::from("user"),
            horizon_browser::remote::CredentialBinding {
                store: horizon_browser::remote::CredentialStoreKind::Environment,
                slot: Some("GRID_USER".into()),
            },
        );
    for (case, remote, files) in [
        ("missing", remote(), missing),
        ("extra", remote(), extra),
        ("unknown provider", remote(), stranger),
        ("relative", remote(), self::files(PathBuf::from("user"), key.clone())),
        ("bindings", bound, self::files(user.clone(), key.clone())),
        ("no provider", RemoteBrowserConfig::default(), BTreeMap::new()),
    ] {
        assert!(config::for_tests(remote, files).is_err(), "{case}");
    }
}

#[cfg(unix)]
#[test]
fn a_host_file_that_others_can_change_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("host.yaml");
    let text = format!(
        "browser:\n  remote:\n{}credential_files:\n  grid:\n    user: /run/user\n    key: /run/key\n",
        PROVIDERS
            .lines()
            .filter(|line| !line.is_empty())
            .fold(String::new(), |mut text, line| {
                text.push_str("    ");
                text.push_str(line);
                text.push('\n');
                text
            })
    );
    std::fs::write(&path, text).expect("host file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
    let loaded = config::HostConfig::load(&path).expect("an owner-writable host file loads");
    assert_eq!(loaded.remote.targets.len(), 1);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).expect("mode");
    let refused = config::HostConfig::load(&path).unwrap_err().to_string();
    assert!(
        refused.contains("must not be writable by group or other users"),
        "{refused}"
    );
}

#[test]
fn a_new_host_takes_over_the_holds_of_a_stopped_host_and_leaves_a_running_one() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("holds");
    let config = config::for_tests(
        remote(),
        files(
            private(dir.path(), "user", b"alice"),
            private(dir.path(), "key", b"key"),
        ),
    )
    .expect("host file");
    let running = Holds::open(&root, "host-running", "horizon:standalone-1", &config).expect("running host");
    // A stopped host left one hold whose journal cannot be read.
    let stopped = root.join("host-stopped");
    std::fs::create_dir_all(&stopped).expect("stopped folder");
    std::fs::write(stopped.join("owner.lock"), b"").expect("lock file");
    std::fs::write(
        stopped.join("browser-lost.hold"),
        br#"{"provider":"grid","reference":"lost-reference"}"#,
    )
    .expect("hold");
    std::fs::write(stopped.join("browser-lost.journal"), b"not a journal").expect("journal");
    // A hold whose journal never got saved had no driver, so a new host removes it.
    std::fs::write(
        stopped.join("browser-never.hold"),
        br#"{"provider":"grid","reference":"never"}"#,
    )
    .expect("hold without journal");
    let adopted = Holds::open(&root, "host-new", "horizon:standalone-2", &config).expect("new host");
    assert!(!stopped.exists(), "the stopped host's folder is taken over");
    assert!(folder(&root, "host-new").join("browser-lost.hold").exists());
    assert!(!folder(&root, "host-new").join("browser-never.hold").exists());
    assert!(
        folder(&root, "host-running").exists(),
        "a running host keeps its folder"
    );
    assert!(
        adopted.any_unreleased(),
        "an unreadable hold stays listed until it is resolved"
    );
    drop(adopted);
    drop(running);
    // A child that another test forks holds inherited lock descriptors until
    // it execs, so a freed lock can read as held for a moment.
    let mut attempts = 0;
    let next = loop {
        let next = Holds::open(&root, "host-next", "horizon:standalone-3", &config).expect("next host");
        if next.any_unreleased() || attempts == 100 {
            break next;
        }
        drop(next);
        attempts += 1;
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(next.any_unreleased(), "the hold moves on to the next host");
    assert!(!folder(&root, "host-running").exists() && !folder(&root, "host-new").exists());
    assert!(folder(&root, "host-next").join("browser-lost.hold").exists());
}

fn folder(root: &Path, host: &str) -> PathBuf {
    root.join(horizon_browser_control::paths::safe_local_id(host))
}
