use super::*;
use crate::cloud_runtime::{Cancellation, github::tests::github};
use horizon_cloud::github::Chain;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

fn chain(access: &str, refresh: &str, access_in: Duration) -> Chain {
    Chain {
        access_token: Secret::new(access.into()),
        access_expires_at: SystemTime::now() + access_in,
        refresh_token: Secret::new(refresh.into()),
        refresh_expires_at: SystemTime::now() + Duration::from_hours(4368),
    }
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
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
    stored::save(
        &docker.path().join(STORE),
        "octo-cat",
        &chain("gho_fresh", "ghr_fresh", Duration::from_hours(4)),
        false,
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
    stored::save(
        &docker.path().join(STORE),
        "octo-cat",
        &chain("gho_old", "ghr_old", Duration::from_secs(60)),
        false,
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
    let stored = stored::load(&docker.path().join(STORE)).unwrap();
    assert_eq!(stored.refresh_token.expose(), "ghr_new", "the rotated chain is stored");
}

#[test]
fn a_chain_github_no_longer_renews_is_forgotten_so_the_card_asks_again() {
    let docker = tempfile::tempdir().unwrap();
    stored::save(
        &docker.path().join(STORE),
        "octo-cat",
        &chain("gho_old", "ghr_revoked", Duration::from_secs(60)),
        false,
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

#[test]
fn a_chain_is_kept_when_github_fails_without_revoking_it() {
    let docker = tempfile::tempdir().unwrap();
    stored::save(
        &docker.path().join(STORE),
        "octo-cat",
        &chain("gho_old", "ghr_old", Duration::from_secs(60)),
        false,
    )
    .unwrap();
    let (client, task) = github(vec![serde_json::json!({"error": "unexpected_error"})]);
    assert!(current(docker.path(), &client).is_err());
    task.join().unwrap();
    assert_eq!(
        stored::load(&docker.path().join(STORE)).unwrap().refresh_token.expose(),
        "ghr_old",
        "the chain stays for a retry"
    );
}

#[test]
fn a_stored_chain_reads_back_as_written() {
    let docker = tempfile::tempdir().unwrap();
    let path = docker.path().join(STORE);
    stored::save(
        &path,
        "octo-cat",
        &chain("gho_a\"b", "ghr_c", Duration::from_hours(4)),
        true,
    )
    .unwrap();
    let back = stored::load(&path).expect("the written chain loads");
    assert_eq!(back.login, "octo-cat");
    assert_eq!(back.access_token.expose(), "gho_a\"b");
    assert_eq!(back.refresh_token.expose(), "ghr_c");
    assert!(back.web);
}

#[test]
fn cloud_settings_name_the_publishing_account_without_asking_github() {
    let docker = tempfile::tempdir().unwrap();
    let path = docker.path().join(STORE);
    assert_eq!(publisher(docker.path()), None, "without a chain the card asks first");
    let mut ended = chain("gho_old", "ghr_old", Duration::from_hours(4));
    ended.refresh_expires_at = SystemTime::now() - Duration::from_mins(1);
    stored::save(&path, "octo-cat", &ended, false).unwrap();
    assert_eq!(publisher(docker.path()), None, "an ended chain asks again");
    stored::save(
        &path,
        "octo-cat",
        &chain("gho_fresh", "ghr_fresh", Duration::from_hours(4)),
        false,
    )
    .unwrap();
    assert_eq!(publisher(docker.path()).as_deref(), Some("octo-cat"));
}

#[test]
fn a_build_without_a_stored_login_pulls_anonymously() {
    let home = tempfile::tempdir().unwrap();
    let (directory, signed_in) = build_config_with(home.path(), None).unwrap();
    assert!(!signed_in);
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(
        config,
        serde_json::json!({ "auths": { "horizon.invalid": {} } }),
        "no helper, no login, and an entry so Docker picks no helper itself"
    );
    assert_eq!(mode(&directory.path().join("config.json")) & 0o077, 0, "private");
    let path = directory.path().to_path_buf();
    drop(directory);
    assert!(!path.exists(), "the configuration goes with the build");
}

#[cfg(unix)]
#[test]
fn a_build_keeps_the_machines_docker_context_and_plugins_but_none_of_its_credentials() {
    let home = tempfile::tempdir().unwrap();
    let machine = tempfile::tempdir().unwrap();
    std::fs::write(
        machine.path().join("config.json"),
        r#"{"auths":{"ghcr.io":{"auth":"c3ludGhldGlj"}},"credsStore":"desktop","currentContext":"rootless","cliPluginsExtraDirs":["/opt/plugins"]}"#,
    )
    .unwrap();
    std::fs::create_dir(machine.path().join("contexts")).unwrap();
    let (directory, _) = build_config_with(home.path(), Some(machine.path())).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(config["currentContext"], "rootless");
    assert_eq!(config["cliPluginsExtraDirs"], serde_json::json!(["/opt/plugins"]));
    assert_eq!(
        config["auths"],
        serde_json::json!({ "horizon.invalid": {} }),
        "the machine's logins stay out"
    );
    assert!(config.get("credsStore").is_none(), "and its credential helpers");
    assert!(
        directory.path().join("contexts").is_dir(),
        "the context it names resolves"
    );
}

#[test]
fn the_app_page_is_the_page_of_horizons_own_oauth_app() {
    assert_eq!(
        APP_PAGE,
        format!("https://github.com/settings/connections/applications/{CLIENT_ID}")
    );
}

#[test]
fn a_build_log_names_the_ghcr_packages_it_pulled() {
    let log = "#3 [internal] load metadata for ghcr.io/peters/horizon-worker-base@sha256:9f23\n\
               #4 [internal] load metadata for ghcr.io/FinterMobilityAS/sdk/runtime:1.0@sha256:bb51\n\
               ERROR: failed to resolve ghcr.io/fintermobilityas/sdk/runtime:1.0: denied\n\
               FROM mcr.microsoft.com/dotnet/sdk:11.0\n";
    assert_eq!(
        ghcr_packages(log),
        [
            ("peters".to_owned(), "horizon-worker-base".to_owned()),
            ("fintermobilityas".to_owned(), "sdk/runtime".to_owned()),
        ]
    );
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        restricting_organization(home.path(), log, &|| false),
        None,
        "nobody to ask without a stored login"
    );
    let flood = (0..20)
        .map(|n| format!("ghcr.io/owner/image-{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        ghcr_packages(&flood).len(),
        8,
        "a log that names many images asks about a few"
    );
    assert_eq!(
        ghcr_packages("the build pulls ghcr.io/acme/base."),
        [("acme".to_owned(), "base".to_owned())],
        "a full stop after the image is no part of its name"
    );
    assert_eq!(
        ghcr_packages(
            "ghcr.io/acme/base:1: unexpected status from HEAD request to https://ghcr.io/v2/acme/base/manifests/1: 403 Forbidden"
        ),
        [("acme".to_owned(), "base".to_owned())],
        "the registry API address is no image"
    );
}

#[test]
fn a_stopped_proof_stops_waiting_for_the_login_another_cloud_holds() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(STORE), "{}").unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join(LOCK))
        .unwrap();
    held.lock().unwrap();
    let asked = std::cell::Cell::new(0);
    let started = std::time::Instant::now();
    let found = restricting_organization(home.path(), "ERROR: ghcr.io/acme/base:1: denied", &|| {
        asked.set(asked.get() + 1);
        asked.get() > 1
    });
    assert_eq!(found, None);
    assert!(asked.get() >= 2, "the wait asked whether the proof stopped");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "and ended at once"
    );
}
