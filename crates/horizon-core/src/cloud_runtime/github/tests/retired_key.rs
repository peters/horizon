//! The App private key file that Connect GitHub wrote while Horizon minted installation
//! tokens, and the retirement that removes it when the settings name the app.
use super::*;
use crate::cloud_runtime::settings::Settings as CloudSettings;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The private key file that Connect GitHub wrote for `id`, beside the app's client secret.
fn written_key(root: &Path, id: u64) -> PathBuf {
    let directory = root.join("credentials");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("github-app-{id}.pem"));
    std::fs::write(
        &path,
        "-----BEGIN RSA PRIVATE KEY-----\nsynthetic\n-----END RSA PRIVATE KEY-----\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

/// The `github` section of cloud settings that name `id`, with the private key entry that a
/// settings file from the removed feature carries.
fn app_entry(id: u64, key: Option<&Path>) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "app_id": id,
        "slug": "horizon-example",
        "client_id": "Iv23synthetic",
        "client_secret_file": format!("/synthetic/credentials/github-app-{id}"),
    });
    if let Some(key) = key {
        entry["private_key_file"] = serde_json::json!(key);
    }
    entry
}

/// A cloud root with a settings file, and the path of that file.
fn machine(root: &Path, github: Option<serde_json::Value>) -> PathBuf {
    let mut settings = serde_json::json!({
        "runpod_key_file": root.join("key"),
        "ssh_identity_file": root.join("identity"),
        "docker_config": root.join("docker"),
    });
    if let Some(github) = github {
        settings["github"] = github;
    }
    let path = root.join("settings.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    path
}

/// What the retirement does for the app that the settings at `path` name, without logging.
fn retirement(path: &Path) -> connect::KeyFileRetirement {
    let app = settings(Mode::Ask, &path.parent().unwrap().join("credentials/github-app-42"));
    connect::retire_private_key_file(path, &app)
}

#[test]
fn the_key_file_that_connect_wrote_is_removed_when_the_settings_name_the_app() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    let path = machine(root.path(), Some(app_entry(42, None)));
    let loaded = CloudSettings::load(&path).unwrap();
    assert_eq!(loaded.github.as_ref().map(|github| github.app_id), Some(42));
    assert!(
        !key.exists(),
        "the App's signing key must not stay on this machine once nothing mints with it"
    );
}

#[test]
fn the_key_file_of_a_machine_that_still_names_it_is_removed_on_the_first_read() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    let path = machine(root.path(), Some(app_entry(42, Some(&key))));
    // The entry that names the key is read and dropped, so the file it names is retired.
    assert!(CloudSettings::load(&path).unwrap().github.is_some());
    assert!(!key.exists(), "the file that the dropped entry named goes with it");
}

#[test]
fn the_cloud_settings_window_retires_the_key_file_too() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    machine(root.path(), Some(app_entry(42, None)));
    assert!(crate::cloud_runtime::setup::Draft::load(root.path()).is_ok());
    assert!(
        !key.exists(),
        "Cloud settings reads saved settings through its own path"
    );
}

#[test]
fn a_file_with_another_name_beside_the_secret_stays() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    let directory = key.parent().unwrap();
    let others: Vec<PathBuf> = [
        "github-app-42.pem.bak",
        "github-app-42",
        "github-app-042.pem",
        "private.pem",
    ]
    .into_iter()
    .map(|name| {
        let path = directory.join(name);
        std::fs::write(&path, "synthetic").unwrap();
        path
    })
    .collect();
    let path = machine(root.path(), Some(app_entry(42, None)));
    CloudSettings::load(&path).unwrap();
    assert!(!key.exists());
    for other in others {
        assert!(other.exists(), "{} is not a file this feature wrote", other.display());
    }
    assert!(
        matches!(retirement(&path), connect::KeyFileRetirement::Nothing),
        "a name this feature never wrote is not reported as a key left behind"
    );
}

#[test]
fn a_directory_at_the_key_files_name_stays() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("credentials/github-app-42.pem")).unwrap();
    let path = machine(root.path(), Some(app_entry(42, None)));
    CloudSettings::load(&path).unwrap();
    assert!(root.path().join("credentials/github-app-42.pem").is_dir());
    assert!(matches!(retirement(&path), connect::KeyFileRetirement::Nothing));
}

#[test]
fn another_apps_key_file_stays() {
    let root = tempfile::tempdir().unwrap();
    let other = written_key(root.path(), 43);
    let path = machine(root.path(), Some(app_entry(42, None)));
    CloudSettings::load(&path).unwrap();
    assert!(other.exists(), "the settings name app 42, so app 43 keeps its file");
    assert!(matches!(retirement(&path), connect::KeyFileRetirement::Nothing));
}

#[test]
fn settings_that_name_no_app_remove_no_key_file() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    let loaded = CloudSettings::load(&machine(root.path(), None)).unwrap();
    assert!(loaded.github.is_none());
    assert!(key.exists(), "a machine with no App has nothing to retire");
}

#[cfg(unix)]
#[test]
fn a_link_at_the_key_files_name_is_never_followed_and_never_removed() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("elsewhere.pem");
    std::fs::write(&target, "synthetic").unwrap();
    std::fs::create_dir_all(root.path().join("credentials")).unwrap();
    let link = root.path().join("credentials/github-app-42.pem");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let path = machine(root.path(), Some(app_entry(42, None)));
    CloudSettings::load(&path).unwrap();
    assert!(link.symlink_metadata().is_ok(), "the link stays");
    assert!(target.exists(), "the file the link points at is untouched");
    assert!(
        matches!(retirement(&path), connect::KeyFileRetirement::Nothing),
        "a link is neither followed nor reported"
    );
}

#[cfg(unix)]
#[test]
fn a_disk_that_refuses_the_unlink_still_loads_the_settings_and_reports_the_path() {
    let root = tempfile::tempdir().unwrap();
    let key = written_key(root.path(), 42);
    let path = machine(root.path(), Some(app_entry(42, None)));
    let directory = root.path().join("credentials");
    let mut permissions = std::fs::metadata(&directory).unwrap().permissions();
    permissions.set_mode(0o500);
    std::fs::set_permissions(&directory, permissions.clone()).unwrap();
    assert!(
        CloudSettings::load(&path).is_ok(),
        "a disk that refuses the unlink must not stop Horizon from starting"
    );
    match retirement(&path) {
        connect::KeyFileRetirement::Kept(reported, error) => {
            assert_eq!(reported, key, "the report names the file that must be deleted by hand");
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        other => panic!("the file cannot be removed here, so it must be reported: {other:?}"),
    }
    permissions.set_mode(0o700);
    std::fs::set_permissions(&directory, permissions).unwrap();
    assert!(key.exists(), "the file stays until a person deletes it");
}
