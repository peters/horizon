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
    keep_repositories(
        root.path(),
        &settings(42),
        "octo-cat",
        &["acme/private".into()],
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        kept_repositories(root.path(), &settings(42)),
        Some(("octo-cat".to_owned(), vec!["acme/private".to_owned()]))
    );
    let saved = disconnect(root.path(), &settings(42), || {
        std::fs::write(root.path().join("settings.json"), "{}")?;
        Ok("saved")
    })
    .unwrap();
    assert_eq!(saved, "saved");
    assert!(!path(root.path(), &settings(42)).exists(), "forgotten under the lock");
    assert!(
        !listed(root.path(), &settings(42)).exists(),
        "the listed names go with the sign-in"
    );
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

#[test]
fn a_web_sign_in_renews_with_the_secret_whatever_the_setting_says_now() {
    use std::io::Write as _;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    let credentials = root.path().join("credentials");
    stored::private_directory(&credentials).unwrap();
    stored::write_private(&credentials, "github-app-42", b"synthetic-secret").unwrap();
    // Signed in while the app was Automatic; the setting is Ask now.
    let ask = Settings {
        client_secret_file: credentials.join("github-app-42"),
        mode: Mode::Ask,
        ..settings(42)
    };
    let chain = horizon_cloud::github::Chain {
        access_token: Secret::new("ghu_old".into()),
        access_expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
        refresh_token: Secret::new("ghr_old".into()),
        refresh_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(400),
    };
    stored::save(&path(root.path(), &ask), "octo-cat", &chain, true).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::loopback(listener.local_addr().unwrap()).unwrap();
    let github = std::thread::spawn(move || {
        let answers = [
            serde_json::json!({"access_token": "ghu_new", "expires_in": 28800, "refresh_token": "ghr_new",
                               "refresh_token_expires_in": 15_724_800, "token_type": "bearer", "scope": ""}),
            serde_json::json!({"id": 1, "login": "octo-cat"}),
        ];
        let mut requests = Vec::new();
        for answer in answers {
            let (mut stream, _) = listener.accept().unwrap();
            requests.push(super::super::tests::read_request(&mut stream));
            let text = answer.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
            .unwrap();
        }
        requests
    });
    let (login, token) = renewed(root.path(), &ask, &client).unwrap().unwrap();
    let requests = github.join().unwrap();
    assert_eq!((login.as_str(), token.expose()), ("octo-cat", "ghu_new"));
    assert!(requests[0].contains("grant_type=refresh_token"));
    assert!(
        requests[0].contains("client_secret=synthetic-secret"),
        "the renewal sends the secret"
    );
    let stored = stored::load(&path(root.path(), &ask)).unwrap();
    assert_eq!(stored.refresh_token.expose(), "ghr_new", "the rotated chain is stored");
    assert!(stored.web, "and still renews with the secret");
}

#[test]
fn a_disconnected_app_asks_github_for_no_sign_in() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), "{}").unwrap();
    let ended = sign_in(root.path(), &settings(42), &Cancellation::default(), &|_| {
        panic!("no code or page is shown")
    })
    .unwrap();
    assert_eq!(ended.err().as_deref(), Some(DISCONNECTED));
}

/// Stores a chain for `login`, as a sign-in of this computer would.
fn sign_in_as(root: &Path, login: &str) {
    let chain = horizon_cloud::github::Chain {
        access_token: Secret::new("ghu_synthetic".into()),
        access_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(4),
        refresh_token: Secret::new("ghr_synthetic".into()),
        refresh_expires_at: std::time::SystemTime::now() + std::time::Duration::from_hours(400),
    };
    stored::save(&path(root, &settings(42)), login, &chain, false).unwrap();
}

fn keep(root: &Path, login: &str, names: &[&str], cancel: &Cancellation) -> bool {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    keep_repositories(root, &settings(42), login, &names, cancel).unwrap()
}

#[test]
fn a_list_is_kept_for_the_signed_in_account_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    keep(root.path(), "octo-cat", &["acme/web"], &Cancellation::default());
    assert!(
        !listed(root.path(), &settings(42)).exists(),
        "no sign-in, so nothing is kept"
    );
    sign_in_as(root.path(), "octo-cat");
    assert!(
        !keep(root.path(), "someone-else", &["acme/other"], &Cancellation::default()),
        "a list of an account that is not signed in is not shown either"
    );
    assert!(
        !listed(root.path(), &settings(42)).exists(),
        "a list for an account that is not signed in is not kept"
    );
    keep(root.path(), "octo-cat", &["acme/web"], &Cancellation::default());
    assert_eq!(
        kept_repositories(root.path(), &settings(42)),
        Some(("octo-cat".to_owned(), vec!["acme/web".to_owned()]))
    );
    assert_eq!(
        kept_repositories(root.path(), &settings(43)),
        None,
        "another app keeps its own"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(listed(root.path(), &settings(42)))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    sign_in_as(root.path(), "other-account");
    assert_eq!(
        kept_repositories(root.path(), &settings(42)),
        None,
        "another account never sees the first one's names"
    );
}

#[test]
fn a_replaced_or_outdated_request_never_drops_a_newer_list() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    sign_in_as(root.path(), "octo-cat");
    keep(root.path(), "octo-cat", &["acme/new"], &Cancellation::default());
    let replaced = Cancellation::default();
    replaced.cancel();
    keep(root.path(), "octo-cat", &["acme/old"], &replaced);
    assert_eq!(
        kept_repositories(root.path(), &settings(42)),
        Some(("octo-cat".to_owned(), vec!["acme/new".to_owned()])),
        "a replaced request does not overwrite the newer list"
    );
    std::fs::remove_file(path(root.path(), &settings(42))).unwrap();
    forget_repositories(root.path(), &settings(42), &replaced).unwrap();
    assert!(
        listed(root.path(), &settings(42)).exists(),
        "a replaced request forgets nothing"
    );
    sign_in_as(root.path(), "octo-cat");
    forget_repositories(root.path(), &settings(42), &Cancellation::default()).unwrap();
    assert!(
        listed(root.path(), &settings(42)).exists(),
        "a sign-in that another window finished keeps its list"
    );
    std::fs::remove_file(path(root.path(), &settings(42))).unwrap();
    forget_repositories(root.path(), &settings(42), &Cancellation::default()).unwrap();
    assert!(
        !listed(root.path(), &settings(42)).exists(),
        "signed out, so the list goes"
    );
    assert_eq!(kept_repositories(root.path(), &settings(42)), None);
}

#[test]
fn a_list_too_large_to_read_back_is_not_kept() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), r#"{"github":{"app_id":42}}"#).unwrap();
    sign_in_as(root.path(), "octo-cat");
    keep(root.path(), "octo-cat", &["acme/small"], &Cancellation::default());
    let long = format!("acme/{}", "x".repeat(100));
    let many: Vec<&str> = std::iter::repeat_n(long.as_str(), 12_000).collect();
    assert!(
        keep(root.path(), "octo-cat", &many, &Cancellation::default()),
        "still this account's list"
    );
    assert_eq!(
        kept_repositories(root.path(), &settings(42)),
        None,
        "the older list is not shown instead"
    );
}
