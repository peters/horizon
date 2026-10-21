use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use horizon_browser::BackendKind;
use horizon_browser_control::manifest::{self, BrowserCreateRequest};

use super::config::{Pref, ProfileConfig, forget_removed_prefs, user_js, valid_name};
use super::host::{MAX_ACTOR_PAGES, MAX_PAGES, profile_dir};
use super::instance::{Claim, Record};
use super::rules::{Close, admit, close_action, idle, idle_pages, page_id};

const HOST_FILE: &str = "\
browser:
  profiles:
    mail:
      backend: chromium
      args: [\"--proxy-server=socks5://proxy.example:1080\", \"--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE proxy.example\"]
    chat:
      backend: firefox
      firefox_prefs:
        network.proxy.type: 1
        network.proxy.socks: proxy.example
        network.proxy.socks_remote_dns: true
";

fn config() -> ProfileConfig {
    ProfileConfig::parse(HOST_FILE.as_bytes()).unwrap_or_else(|error| panic!("valid host file: {error}"))
}

fn request(target: Option<&str>) -> BrowserCreateRequest {
    let mut request = BrowserCreateRequest::for_tests("example/ws/agent");
    request.target = target.map(str::to_owned);
    request
}

#[test]
fn a_host_file_names_each_profile_with_its_engine_args_and_prefs() {
    let config = config();
    let mail = config.profile("mail").unwrap_or_else(|| panic!("mail"));
    assert_eq!(mail.backend, BackendKind::ChromiumCdp);
    assert_eq!(mail.args.len(), 2);
    assert!(mail.firefox_prefs.is_empty());
    let chat = config.profile("chat").unwrap_or_else(|| panic!("chat"));
    assert_eq!(chat.backend, BackendKind::FirefoxBidi);
    assert_eq!(chat.firefox_prefs.get("network.proxy.type"), Some(&Pref::Int(1)));
    assert_eq!(
        chat.firefox_prefs.get("network.proxy.socks_remote_dns"),
        Some(&Pref::Bool(true))
    );
    assert!(config.profile("nope").is_none());
}

#[test]
fn a_host_file_refuses_bad_names_engines_args_and_prefs() {
    let refused = [
        "browser:\n  profiles: {}\n",
        "browser:\n  profiles:\n    Mail: {backend: chromium}\n",
        "browser:\n  profiles:\n    -mail: {backend: chromium}\n",
        "browser:\n  profiles:\n    mail-: {backend: chromium}\n",
        "browser:\n  profiles:\n    a_b: {backend: chromium}\n",
        "browser:\n  profiles:\n    mail: {backend: safari}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, extra: 1}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium}\n  remote: {}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, firefox_prefs: {a.b: 1}}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, args: [\"--user-data-dir=/x\"]}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, args: [\"--remote-debugging-port=9\"]}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, args: [\"--headless=new\"]}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, args: [\"a\\nb\"]}\n",
        "browser:\n  profiles:\n    mail: {backend: chromium, args: [\"\"]}\n",
        "browser:\n  profiles:\n    chat: {backend: firefox, args: [\"-profile\"]}\n",
        "browser:\n  profiles:\n    chat: {backend: firefox, firefox_prefs: {\"a b\": 1}}\n",
        "browser:\n  profiles:\n    chat: {backend: firefox, firefox_prefs: {a: \"x\\ty\"}}\n",
        "browser:\n  profiles:\n    chat: {backend: firefox, firefox_prefs: {a: 3000000000}}\n",
        "credential_files: {}\nbrowser:\n  profiles:\n    mail: {backend: chromium}\n",
    ];
    for text in refused {
        assert!(ProfileConfig::parse(text.as_bytes()).is_err(), "{text}");
    }
    let many = (0..=super::config::MAX_PROFILES).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "    p{n}: {{backend: chromium}}");
        text
    });
    assert!(ProfileConfig::parse(format!("browser:\n  profiles:\n{many}").as_bytes()).is_err());
    assert!(valid_name("a") && valid_name("mail-2") && !valid_name(&"a".repeat(41)));
}

#[cfg(unix)]
#[test]
fn a_host_file_that_others_can_write_or_that_is_too_large_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let path = dir.path().join("host.yaml");
    std::fs::write(&path, HOST_FILE).unwrap_or_else(|error| panic!("write: {error}"));
    for (mode, ok) in [(0o600, true), (0o644, true), (0o664, false), (0o646, false)] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(ProfileConfig::load(&path).is_ok(), ok, "mode {mode:o}");
    }
    let mut large = HOST_FILE.to_owned();
    large.push_str(&"# padding\n".repeat(27_000));
    std::fs::write(&path, large).unwrap_or_else(|error| panic!("write: {error}"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap_or_else(|error| panic!("{error}"));
    let error = ProfileConfig::load(&path)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(error.contains("at most 262144 bytes"), "{error}");
}

#[test]
fn user_js_writes_each_preference_as_a_literal() {
    let config = config();
    let chat = config.profile("chat").unwrap_or_else(|| panic!("chat"));
    let text = user_js(&chat.firefox_prefs);
    assert!(text.contains("user_pref(\"network.proxy.type\", 1);\n"), "{text}");
    assert!(
        text.contains("user_pref(\"network.proxy.socks\", \"proxy.example\");\n"),
        "{text}"
    );
    assert!(
        text.contains("user_pref(\"network.proxy.socks_remote_dns\", true);\n"),
        "{text}"
    );
    let quoted = user_js(&[("a.b".to_owned(), Pref::Text("say \"hi\" \\ now".into()))].into());
    assert!(quoted.contains(r#"user_pref("a.b", "say \"hi\" \\ now");"#), "{quoted}");
}

#[test]
fn a_preference_that_the_host_file_no_longer_names_gets_its_default_again() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let earlier = [
        ("network.proxy.type".to_owned(), Pref::Int(1)),
        ("network.proxy.socks".to_owned(), Pref::Text("proxy.example".into())),
    ]
    .into();
    std::fs::write(dir.path().join("user.js"), user_js(&earlier)).unwrap_or_else(|error| panic!("{error}"));
    let prefs = "// Mozilla User Preferences\nuser_pref(\"browser.startup.page\", 3);\nuser_pref(\"network.proxy.socks\", \"proxy.example\");\nuser_pref(\"network.proxy.type\", 1);\n";
    std::fs::write(dir.path().join("prefs.js"), prefs).unwrap_or_else(|error| panic!("{error}"));
    let now = [("network.proxy.type".to_owned(), Pref::Int(1))].into();
    forget_removed_prefs(dir.path(), &now).unwrap_or_else(|error| panic!("forget: {error}"));
    let kept = std::fs::read_to_string(dir.path().join("prefs.js")).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        kept,
        "// Mozilla User Preferences\nuser_pref(\"browser.startup.page\", 3);\nuser_pref(\"network.proxy.type\", 1);\n"
    );
    std::fs::write(dir.path().join("user.js"), "user_pref(\"browser.startup.page\", 1);\n")
        .unwrap_or_else(|error| panic!("{error}"));
    forget_removed_prefs(dir.path(), &BTreeMap::new()).unwrap_or_else(|error| panic!("forget: {error}"));
    let untouched = std::fs::read_to_string(dir.path().join("prefs.js")).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(untouched, kept, "a user.js that the host did not write changes nothing");
}

#[test]
fn a_create_names_one_profile_of_the_host_file_and_no_remote_target() {
    let config = config();
    let refused =
        |request: &BrowserCreateRequest, running| admit(&config, request, None, running, 0).err().map(|r| r.code);
    assert_eq!(admit(&config, &request(Some("mail")), None, 0, 0), Ok("mail"));
    assert_eq!(refused(&request(None), 0), Some("target_required"));
    assert_eq!(refused(&request(Some("nope")), 0), Some("target_unknown"));
    let device = format!("catalog.browserstack.{}", "a".repeat(64));
    assert_eq!(refused(&request(Some(&device)), 0), Some("target_invalid"));
    assert_eq!(refused(&request(Some("mail")), MAX_PAGES), Some("page_limit_reached"));
    assert_eq!(
        admit(&config, &request(Some("mail")), None, MAX_PAGES - 1, 0),
        Ok("mail")
    );
    let mut engine = request(Some("mail"));
    engine.backend = Some(BackendKind::FirefoxBidi);
    assert_eq!(refused(&engine, 0), Some("unsupported_backend"));
    let mut outsider = request(Some("mail"));
    outsider.actor = "horizon-mcp:42".into();
    assert_eq!(refused(&outsider, 0), Some("browser_start_failed"));
    let mut late = request(Some("mail"));
    late.deadline_at_millis = manifest::now_millis() - 1;
    assert_eq!(refused(&late, 0), Some("browser_start_failed"));
}

#[test]
fn a_duplicate_opens_in_the_profile_of_its_own_source_page_only() {
    let config = config();
    let mut duplicate = request(None);
    duplicate.duplicate_from = Some("profile-chat-0123456789ab".into());
    let actor = duplicate.actor.clone();
    assert_eq!(admit(&config, &duplicate, Some(("chat", &actor)), 1, 1), Ok("chat"));
    let other = admit(&config, &duplicate, Some(("chat", "horizon:example/other/agent")), 1, 0);
    assert_eq!(other.err().map(|refusal| refusal.code), Some("not_found"));
    let gone = admit(&config, &duplicate, None, 0, 0);
    assert_eq!(gone.err().map(|refusal| refusal.code), Some("not_found"));
}

#[test]
fn only_the_actor_of_a_page_closes_it_also_while_it_stops() {
    let (mine, other) = ("horizon:example/ws/agent", "horizon:example/other/agent");
    assert_eq!(close_action(Some(mine), None, mine), Close::Stop);
    assert_eq!(close_action(Some(mine), None, other), Close::NotFound);
    assert_eq!(close_action(None, Some(mine), mine), Close::Wait);
    assert_eq!(close_action(None, Some(mine), other), Close::NotFound);
    assert_eq!(close_action(None, None, mine), Close::NotFound);
}

#[test]
fn one_actor_holds_at_most_its_pages_and_the_last_page_goes_to_a_client_with_none() {
    let config = config();
    let code = |running, mine| {
        admit(&config, &request(Some("mail")), None, running, mine)
            .err()
            .map(|refusal| refusal.code)
    };
    assert_eq!(code(MAX_ACTOR_PAGES - 1, MAX_ACTOR_PAGES - 1), None);
    assert_eq!(code(MAX_ACTOR_PAGES, MAX_ACTOR_PAGES), Some("page_limit_reached"));
    assert_eq!(code(MAX_PAGES - 2, 1), None);
    assert_eq!(code(MAX_PAGES - 1, 1), Some("page_limit_reached"));
    assert_eq!(
        code(MAX_PAGES - 1, 0),
        None,
        "a client with no page takes the last page"
    );
    assert_eq!(code(MAX_PAGES, 0), Some("page_limit_reached"));
    let mut duplicate = request(None);
    duplicate.duplicate_from = Some("profile-chat-0123456789ab".into());
    let actor = duplicate.actor.clone();
    let full = admit(
        &config,
        &duplicate,
        Some(("chat", &actor)),
        MAX_ACTOR_PAGES,
        MAX_ACTOR_PAGES,
    );
    assert_eq!(full.err().map(|refusal| refusal.code), Some("page_limit_reached"));
}

#[test]
fn page_ids_name_their_profile_and_twelve_hex_digits() {
    let first = page_id("mail");
    let hex = first.strip_prefix("profile-mail-").unwrap_or_else(|| panic!("{first}"));
    assert_eq!(hex.len(), 12);
    assert!(hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(first, page_id("mail"));
}

#[test]
fn profiles_live_under_browser_profiles_and_keep_their_engine_folder() {
    let root = std::path::Path::new("/state");
    let mail = profile_dir(root, "mail", BackendKind::ChromiumCdp);
    assert!(mail.starts_with(root.join("browser-profiles/profile-mail")), "{mail:?}");
    assert!(mail.ends_with("chromium"), "{mail:?}");
    let chat = profile_dir(root, "chat", BackendKind::FirefoxBidi);
    assert!(chat.starts_with(root.join("browser-profiles/profile-chat")), "{chat:?}");
    assert!(chat.ends_with("firefox"), "{chat:?}");
}

#[test]
fn a_page_with_no_action_within_the_limit_is_idle() {
    let now = SystemTime::now();
    let limit = Duration::from_hours(1);
    assert!(!idle(now, now, limit));
    assert!(!idle(now - Duration::from_mins(59), now, limit));
    assert!(idle(now - limit, now, limit));
    assert!(!idle(now + Duration::from_secs(5), now, limit));
}

#[test]
fn a_short_idle_limit_closes_only_the_page_whose_journal_is_old() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let opened = SystemTime::now() - Duration::from_secs(5);
    let active = "profile-mail-aaaaaaaaaaaa";
    let audit = manifest::audit_path_for_root(root.path(), active);
    std::fs::create_dir_all(audit.parent().unwrap_or(root.path())).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(&audit, "{}\n").unwrap_or_else(|error| panic!("{error}"));
    let pages = [(active, opened), ("profile-mail-bbbbbbbbbbbb", opened)];
    let idle = idle_pages(
        root.path(),
        pages.into_iter(),
        SystemTime::now(),
        Duration::from_secs(1),
    );
    assert_eq!(idle, vec!["profile-mail-bbbbbbbbbbbb".to_owned()]);
    let later = SystemTime::now() + Duration::from_secs(2);
    let idle = idle_pages(root.path(), pages.into_iter(), later, Duration::from_secs(1));
    assert_eq!(idle.len(), 2, "an old journal is idle too: {idle:?}");
}

#[test]
fn one_host_holds_a_root_and_keeps_its_instance_over_a_restart() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let first = Claim::take(root.path()).unwrap_or_else(|error| panic!("first claim: {error}"));
    let busy = Claim::take(root.path()).err().map(|error| error.kind());
    assert_eq!(busy, Some(std::io::ErrorKind::AddrInUse));
    assert!(
        Record::read(root.path()).is_none(),
        "no record before the host is ready"
    );
    first
        .ready(root.path())
        .unwrap_or_else(|error| panic!("ready: {error}"));
    let record = Record::read(root.path()).unwrap_or_else(|| panic!("record"));
    assert_eq!(record.host_instance, first.host_instance());
    assert_eq!(record.pid, std::process::id());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        use super::instance::RECORD;
        let mode = std::fs::metadata(root.path().join(RECORD))
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or_default();
        assert_eq!(mode, 0o600);
    }
    let instance = first.host_instance().to_owned();
    drop(first);
    let again = Claim::take(root.path()).unwrap_or_else(|error| panic!("second claim: {error}"));
    assert_eq!(again.host_instance(), instance);
}
