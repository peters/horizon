//! Connect GitHub: the manifest flow's pages and the app's secret.
use super::*;

#[test]
fn the_start_page_posts_an_escaped_manifest_with_the_state() {
    let page = connect::start_page(4711, "s1", "Horizon \"<quoted>\"");
    assert!(page.contains("action=\"https://github.com/settings/apps/new?state=s1\""));
    assert!(page.contains("http://127.0.0.1:4711/created"));
    assert!(page.contains("&quot;callback_urls&quot;:[&quot;http://127.0.0.1/callback&quot;]"));
    assert!(!page.contains("\"<quoted>\""), "the name is escaped");
    assert!(page.contains("&quot;public&quot;:true"));
    assert!(page.contains("never stores its private key"), "the description");
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
