//! Connect GitHub: the manifest flow's pages and the app's private files.
use super::*;

/// The private key text that the fake app answers with; it is never parsed as a key here.
const PEM: &str = "-----BEGIN RSA PRIVATE KEY-----\nsynthetic\n-----END RSA PRIVATE KEY-----\n";

fn app() -> horizon_cloud::github::App {
    horizon_cloud::github::App {
        id: 42,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret: Secret::new("synthetic-client-secret".into()),
        pem: Secret::new(PEM.into()),
        html_url: "https://github.com/apps/horizon-example".into(),
    }
}

#[test]
fn the_start_page_posts_an_escaped_manifest_with_the_state() {
    let page = connect::start_page(4711, "s1", "Horizon \"<quoted>\"");
    assert!(page.contains("action=\"https://github.com/settings/apps/new?state=s1\""));
    assert!(page.contains("http://127.0.0.1:4711/created"));
    assert!(page.contains("&quot;callback_urls&quot;:[&quot;http://127.0.0.1/callback&quot;]"));
    assert!(!page.contains("\"<quoted>\""), "the name is escaped");
    assert!(page.contains("&quot;public&quot;:true"));
    assert!(page.contains("App key that stays on your computer"), "the description");
}

#[test]
fn a_created_app_keeps_its_key_beside_its_secret_and_its_settings_name_it() {
    let root = tempfile::tempdir().unwrap();
    let settings = connect::app_files(&app(), root.path()).unwrap();
    let key = settings
        .private_key_file
        .clone()
        .expect("the settings name the key file");
    assert_eq!(key, root.path().join("credentials/github-app-42.pem"));
    assert_eq!(std::fs::read_to_string(&key).unwrap(), PEM);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only this user reads the app's key");
    }
    assert!(settings.validate().is_ok());
    assert_eq!(
        std::fs::read_to_string(&settings.client_secret_file).unwrap(),
        "synthetic-client-secret"
    );
}

#[test]
fn a_second_conversion_keeps_the_key_file_that_is_already_there() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("credentials");
    std::fs::create_dir_all(&directory).unwrap();
    let key = directory.join("github-app-42.pem");
    std::fs::write(&key, "the key of the app that is already saved\n").unwrap();
    let settings = connect::app_files(&app(), root.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(&key).unwrap(),
        "the key of the app that is already saved\n",
        "a key that is already there is kept, never replaced"
    );
    assert_eq!(settings.private_key_file.as_deref(), Some(key.as_path()));
}

#[test]
fn a_key_write_that_fails_leaves_neither_the_secret_nor_a_partial_key() {
    let root = tempfile::tempdir().unwrap();
    // The secret is written, then the key write makes its entry and fails, as a full disk or
    // an I/O error does after `create_new`.
    let outcome = connect::app_files_with(&app(), root.path(), |path, value| {
        std::fs::create_dir_all(path.parent().unwrap())?;
        if path.extension().is_some_and(|extension| extension == "pem") {
            std::fs::write(path, "-----BEGIN RSA PRIVATE KEY-----\npartial")?;
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "synthetic write failure",
            )));
        }
        std::fs::write(path, value)?;
        Ok(())
    });
    assert!(
        matches!(&outcome, Err(Error::Io(_))),
        "the caller sees the failure that refused the connect: {outcome:?}"
    );
    let left = names(root.path().join("credentials").as_path());
    assert!(left.is_empty(), "the connect left no file behind: {left:?}");
}

#[test]
fn settings_that_refuse_the_app_leave_no_written_file_and_keep_the_key_that_was_there() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("credentials");
    std::fs::create_dir_all(&directory).unwrap();
    let key = directory.join("github-app-42.pem");
    std::fs::write(&key, "the key of the app that is already saved\n").unwrap();
    // A settings check that refuses the app fails after both files are in place.
    let refused = horizon_cloud::github::App {
        slug: String::new(),
        ..app()
    };
    assert!(connect::app_files(&refused, root.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(&key).unwrap(),
        "the key of the app that is already saved\n",
        "this call did not write that key, so a later failure keeps it"
    );
    assert_eq!(
        names(&directory),
        ["github-app-42.pem"],
        "the client secret that this call wrote goes with the failure"
    );
    let fresh = tempfile::tempdir().unwrap();
    assert!(connect::app_files(&refused, fresh.path()).is_err());
    assert!(
        names(&fresh.path().join("credentials")).is_empty(),
        "a refused app keeps neither file"
    );
}

/// The entry names in `directory`, sorted, so that a test can compare them.
fn names(directory: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_new_app_is_named_for_its_person_when_that_name_is_free() {
    let mut asked = None;
    let name = connect::name_for(Some("Octo-Cat"), |slug| {
        asked = Some(slug.to_owned());
        false
    });
    assert_eq!(name, "Horizon for Octo-Cat");
    assert_eq!(asked.as_deref(), Some("horizon-for-octo-cat"));
    let random = |name: &str| name.len() == 14 && name.starts_with("Horizon ") && !name.contains(" for ");
    let taken = connect::name_for(Some("octo-cat"), |_| true);
    assert!(random(&taken), "a taken name falls back: {taken}");
    assert!(random(&connect::name_for(None, |_| unreachable!())));
    let long = "a".repeat(23);
    assert!(
        random(&connect::name_for(Some(&long), |_| unreachable!())),
        "GitHub allows 34 characters"
    );
    assert_eq!(
        connect::name_for(Some(&long[1..]), |_| false).len(),
        34,
        "a name of 34 characters fits"
    );
    assert!(random(&connect::name_for(Some("../x"), |_| unreachable!())));
}

#[test]
fn the_known_login_comes_from_a_kept_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let docker = root.path().join("docker");
    assert_eq!(connect::known_login(root.path(), &docker), None);
    let credentials = root.path().join("credentials");
    stored::private_directory(&credentials).unwrap();
    stored::save(&credentials.join("github-host-7.json"), "octo-cat", &chain(), false).unwrap();
    assert_eq!(
        connect::known_login(root.path(), &docker).as_deref(),
        Some("octo-cat"),
        "this computer's sign-in for an earlier app"
    );
    stored::private_directory(&docker).unwrap();
    stored::save(&docker.join(publish::STORE), "packages-cat", &chain(), false).unwrap();
    assert_eq!(
        connect::known_login(root.path(), &docker).as_deref(),
        Some("packages-cat"),
        "the permission to publish images comes first"
    );
}

#[test]
fn only_a_redirect_with_the_state_yields_the_manifest_code() {
    assert_eq!(
        connect::redirect_code("code=abc123&state=s1", "s1").map(|code| code.expose().to_owned()),
        Some("abc123".into())
    );
    assert!(connect::redirect_code("code=abc123&state=s2", "s1").is_none());
    assert!(connect::redirect_code("code=a%2Fb&state=s1", "s1").is_none());
    assert!(connect::redirect_code("state=s1", "s1").is_none());
}

#[test]
fn a_secret_stays_when_the_settings_file_already_names_its_app() {
    let root = tempfile::tempdir().unwrap();
    let secret = private(root.path(), "synthetic-secret");
    let app = settings(Mode::Ask, &secret);
    let file = root.path().join("settings.json");
    std::fs::write(
        &file,
        serde_json::json!({"github": {"client_secret_file": secret}}).to_string(),
    )
    .unwrap();
    connect::discard(root.path(), &app);
    assert!(secret.exists(), "the committed settings name this secret");
    std::fs::write(&file, "{").unwrap();
    connect::discard(root.path(), &app);
    assert!(secret.exists(), "unreadable settings keep it");
    std::fs::write(&file, "{}").unwrap();
    connect::discard(root.path(), &app);
    assert!(!secret.exists(), "settings without this app let it go");
    let secret = private(root.path(), "synthetic-secret");
    std::fs::remove_file(&file).unwrap();
    connect::discard(root.path(), &settings(Mode::Ask, &secret));
    assert!(!secret.exists(), "no settings file names nothing");
}

#[test]
fn a_created_app_keeps_its_key_when_the_settings_file_names_it() {
    let root = tempfile::tempdir().unwrap();
    let written = connect::app_files(&app(), root.path()).unwrap();
    let key = written.private_key_file.clone().unwrap();
    std::fs::write(
        root.path().join("settings.json"),
        serde_json::json!({"github": {"client_secret_file": written.client_secret_file}}).to_string(),
    )
    .unwrap();
    connect::discard(root.path(), &written);
    assert!(key.exists(), "the committed settings name this key");
}

#[test]
fn a_key_file_goes_with_its_secret_when_no_settings_name_the_app() {
    let root = tempfile::tempdir().unwrap();
    let written = connect::app_files(&app(), root.path()).unwrap();
    let key = written.private_key_file.clone().unwrap();
    std::fs::write(root.path().join("settings.json"), "{}").unwrap();
    connect::discard(root.path(), &written);
    assert!(!written.client_secret_file.exists());
    assert!(!key.exists(), "the app's key does not outlive a save that failed");
}

#[test]
fn a_cancelled_connect_flow_ends_without_an_app() {
    let root = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let created = connect::start(root.path(), "Horizon synthetic", |_| Ok(()), cancel.clone()).unwrap();
    cancel.cancel();
    let outcome = created
        .recv_timeout(Duration::from_secs(5))
        .expect("the flow ends at once");
    assert!(outcome.is_err());
    assert!(
        std::fs::read_dir(root.path()).unwrap().next().is_none(),
        "no secret is written"
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    let opened = |_: &str| -> std::io::Result<()> { panic!("a cancelled flow opens no page") };
    assert!(connect::start(root.path(), "Horizon synthetic", opened, cancelled).is_err());
}
