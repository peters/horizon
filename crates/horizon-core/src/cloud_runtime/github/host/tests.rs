use super::*;

fn settings(app_id: u64) -> Settings {
    Settings {
        app_id,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret_file: PathBuf::from("/synthetic/secret"),
        mode: Mode::Ask,
    }
}

#[test]
fn each_app_has_its_own_sign_in_on_this_computer() {
    let root = Path::new("/synthetic/cloud");
    assert_eq!(
        path(root, &settings(42)),
        Path::new("/synthetic/cloud/credentials/github-host-42.json")
    );
    assert_ne!(path(root, &settings(42)), path(root, &settings(43)));
}

#[test]
fn without_a_sign_in_this_computer_asks_without_calling_github() {
    let root = tempfile::tempdir().unwrap();
    assert!(current(root.path(), &settings(42)).unwrap().is_none());
}

#[test]
fn a_disconnect_forgets_the_sign_in_and_a_later_use_finds_the_app_gone() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    assert!(configured(root.path(), &settings(42)));
    assert!(!configured(root.path(), &settings(43)));
    let chain = horizon_cloud::github::Chain {
        access_token: Secret::new("ghu_synthetic".into()),
        access_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(4),
        refresh_token: Secret::new("ghr_synthetic".into()),
        refresh_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(400),
    };
    stored::save(&path(root.path(), &settings(42)), "octo-cat", &chain, false).unwrap();
    let saved = disconnect(root.path(), &settings(42), || {
        std::fs::write(root.path().join("settings.json"), "{}")?;
        Ok("saved")
    })
    .unwrap();
    assert_eq!(saved, "saved");
    assert!(!path(root.path(), &settings(42)).exists(), "forgotten under the lock");
    // A chain that a renewal in another window would store is dropped, not served.
    stored::save(&path(root.path(), &settings(42)), "octo-cat", &chain, false).unwrap();
    assert!(current(root.path(), &settings(42)).unwrap().is_none());
    assert!(!path(root.path(), &settings(42)).exists());
}

#[test]
fn a_web_sign_in_without_a_readable_secret_says_so_and_keeps_the_chain() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    // Close to its expiry, so it renews first; the secret file does not exist.
    let chain = horizon_cloud::github::Chain {
        access_token: Secret::new("ghu_synthetic".into()),
        access_expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        refresh_token: Secret::new("ghr_synthetic".into()),
        refresh_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(400),
    };
    let web = Settings {
        client_secret_file: root.path().join("credentials/github-app-42"),
        mode: Mode::Automatic,
        ..settings(42)
    };
    stored::save(&path(root.path(), &web), "octo-cat", &chain, true).unwrap();
    let error = current(root.path(), &web).expect_err("the missing secret is this computer's error");
    assert!(
        !error.to_string().contains("GitHub"),
        "not taken for a refusal by GitHub: {error}"
    );
    assert!(
        stored::load(&path(root.path(), &web)).is_some(),
        "the chain stays for when the secret is back"
    );
}
