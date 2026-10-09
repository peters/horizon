use super::*;
use crate::cloud_runtime::{Cancellation, github::tests::github};
use std::sync::{Arc, Mutex};

fn chain(access: &str, refresh: &str, access_in: Duration) -> Chain {
    Chain {
        access_token: Secret::new(access.into()),
        access_expires_at: SystemTime::now() + access_in,
        refresh_token: Secret::new(refresh.into()),
        refresh_expires_at: SystemTime::now() + Duration::from_hours(4368),
    }
}

fn mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o600
    }
}

#[test]
fn only_ghcr_images_need_the_publishing_login() {
    assert!(publishes_to_ghcr("ghcr.io/peters/horizon-development:tag"));
    assert!(publishes_to_ghcr("GHCR.IO/acme/worker@sha256:abc"));
    for image in [
        "docker.io/acme/worker:1",
        "acme/worker:1",
        "ghcr.io",
        "registry.example/ghcr.io/x",
    ] {
        assert!(!publishes_to_ghcr(image), "{image}");
    }
}

#[test]
fn the_login_keeps_other_settings_and_replaces_a_ghcr_helper() {
    let docker = tempfile::tempdir().unwrap();
    write_private(
        docker.path(),
        "config.json",
        br#"{"auths":{"registry.example":{"auth":"eDp5"}},"credHelpers":{"ghcr.io":"desktop","gcr.io":"gcloud"},"psFormat":"x"}"#,
    )
    .unwrap();
    write_auth(docker.path(), "octo-cat", &Secret::new("gho_synthetic".into())).unwrap();
    let path = docker.path().join("config.json");
    assert_eq!(mode(&path), 0o600);
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let expected = base64::engine::general_purpose::STANDARD.encode("octo-cat:gho_synthetic");
    assert_eq!(config["auths"]["ghcr.io"]["auth"], expected.as_str());
    assert_eq!(config["auths"]["registry.example"]["auth"], "eDp5");
    assert_eq!(config["credHelpers"], serde_json::json!({"gcr.io": "gcloud"}));
    assert_eq!(config["psFormat"], "x");
    write_private(docker.path(), "config.json", br#"{"credsStore":"desktop"}"#).unwrap();
    assert!(write_auth(docker.path(), "octo-cat", &Secret::new("gho_synthetic".into())).is_err());
}

#[test]
fn a_fresh_stored_chain_is_used_after_github_names_its_account() {
    let docker = tempfile::tempdir().unwrap();
    save(
        docker.path(),
        "octo-cat",
        &chain("gho_fresh", "ghr_fresh", Duration::from_hours(4)),
    )
    .unwrap();
    assert_eq!(mode(&docker.path().join(STORE)), 0o600);
    let (client, task) = github(vec![serde_json::json!({"id": 1, "login": "octo-cat"})]);
    let (login, token) = current(docker.path(), &client).unwrap().unwrap();
    task.join().unwrap();
    assert_eq!((login.as_str(), token.expose()), ("octo-cat", "gho_fresh"));
}

#[test]
fn a_chain_near_its_expiry_is_renewed_and_stored_before_use() {
    let docker = tempfile::tempdir().unwrap();
    save(
        docker.path(),
        "octo-cat",
        &chain("gho_old", "ghr_old", Duration::from_secs(60)),
    )
    .unwrap();
    let (client, task) = github(vec![
        serde_json::json!({"access_token": "gho_new", "expires_in": 28800, "refresh_token": "ghr_new",
                           "refresh_token_expires_in": 15_724_800, "token_type": "bearer",
                           "scope": "write:packages"}),
        serde_json::json!({"id": 1, "login": "octo-cat"}),
    ]);
    let (_, token) = current(docker.path(), &client).unwrap().unwrap();
    task.join().unwrap();
    assert_eq!(token.expose(), "gho_new");
    let stored = load(docker.path()).unwrap();
    assert_eq!(stored.refresh_token.expose(), "ghr_new", "the rotated chain is stored");
}

#[test]
fn a_chain_github_no_longer_renews_is_forgotten_so_the_card_asks_again() {
    let docker = tempfile::tempdir().unwrap();
    save(
        docker.path(),
        "octo-cat",
        &chain("gho_old", "ghr_revoked", Duration::from_secs(60)),
    )
    .unwrap();
    let (client, task) = github(vec![serde_json::json!({"error": "bad_refresh_token"})]);
    assert!(current(docker.path(), &client).unwrap().is_none());
    task.join().unwrap();
    assert!(!docker.path().join(STORE).exists());
}

#[test]
fn a_skipped_publishing_sign_in_ends_with_a_clear_reason() {
    let docker = tempfile::tempdir().unwrap();
    let (client, task) = github(vec![serde_json::json!({
        "device_code": "synthetic-device", "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 1})]);
    let cancel = Cancellation::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    let emit = move |event: Event| seen.lock().unwrap().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    super::super::skip("publish-cloud");
    // The skip comes after the code shows, as from the card.
    let skipper = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        super::super::skip("publish-cloud");
    });
    let error = sign_in(docker.path(), &client, "publish-cloud", &runner).unwrap_err();
    skipper.join().unwrap();
    task.join().unwrap();
    assert!(error.to_string().contains("not allowed to publish"), "{error}");
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GitHub(Prompt::Publish { user_code, .. }) if user_code == "WDJB-MJHT"))
    );
    assert!(matches!(
        events.last(),
        Some(Event::GitHub(Prompt::Published { allowed: false }))
    ));
}
