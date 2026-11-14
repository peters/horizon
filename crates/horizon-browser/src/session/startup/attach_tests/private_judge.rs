//! One real Chrome of the macOS test lane, which judges the private page of a
//! browser host against a browser and not against a mock.
//!
//! A mock browser proves the commands that a session sends; only a browser proves
//! that a private page reads nothing of the profile. This test starts that
//! browser itself, in a throwaway profile of the test, and attaches to it the way
//! the lane of a browser host attaches to the browser of that machine.
//!
//! The module is for macOS, because it starts the Chrome that the test lane
//! installs at its own path.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::attach_tests::*;

/// The Chrome of a Mac test lane, when this machine has it.
#[cfg(target_os = "macos")]
const MACOS_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Two small pages of one loopback HTTP server, so a cookie and a stored value
/// of a page are read by a page and not by a script of the test.
#[cfg(target_os = "macos")]
struct PageServer {
    port: u16,
}

#[cfg(target_os = "macos")]
impl PageServer {
    /// Serve `/page-a.html` and `/page-b.html`, each one small paragraph.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the page server");
        let port = listener.local_addr().expect("page server address").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut socket = stream;
                let mut reader = std::io::BufReader::new(socket.try_clone().expect("a page request"));
                let mut request = String::new();
                let _ = std::io::BufRead::read_line(&mut reader, &mut request);
                let body = "<!doctype html><title>judge</title><p>a page of the judge</p>";
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut socket, answer.as_bytes());
                let _ = std::io::Write::flush(&mut socket);
            }
        });
        Self { port }
    }

    fn url(&self, page: &str) -> String {
        format!("http://127.0.0.1:{}/{page}", self.port)
    }
}

/// One session of this test on the page `url`, attached to the browser of
/// `endpoint` the way the lane of a browser host attaches to it.
#[cfg(target_os = "macos")]
fn judge_session(endpoint: &DevtoolsEndpoint, url: &str, private: bool) -> crate::BrowserSession {
    let frame_slot = Arc::new(FrameSlot::new());
    let mut config = attached_config(endpoint.clone(), &frame_slot);
    config.initial_url = Some(url.to_string());
    config.browser.isolated_page = private;
    config.browser.profile = Some(if private {
        crate::PRIVATE_PROFILE.to_string()
    } else {
        crate::DEFAULT_PROFILE.to_string()
    });
    let session = crate::start_session(config).expect("start a session of the judge");
    assert!(session.send(BrowserCommand::Navigate(url.to_string())), "navigate");
    let committed = wait_for_url(&session, Duration::from_secs(30), url);
    assert!(committed.is_some(), "the page {url} commits: {committed:?}");
    session
}

/// What the page of `target` holds, read by one link of this test: the cookies
/// of that page's browser context and the value that it stored.
#[cfg(target_os = "macos")]
fn read_page(link: &mut crate::cdp::CdpLink, target: &str, expression: &str) -> String {
    let attached = link.call_and_drain(
        Duration::from_secs(10),
        "Target.attachToTarget",
        &json!({ "targetId": target, "flatten": true }),
        None,
    );
    let session_id = attached
        .result
        .expect("attach to the page of the judge")
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("the page session")
        .to_string();
    let answer = link.call_and_drain(
        Duration::from_secs(10),
        "Runtime.evaluate",
        &json!({ "expression": expression, "returnByValue": true }),
        Some(&session_id),
    );
    answer
        .result
        .expect("evaluate in the page of the judge")
        .pointer("/result/value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The page target of the address `url`, as the browser of the judge lists it.
#[cfg(target_os = "macos")]
fn page_target(link: &mut crate::cdp::CdpLink, url: &str) -> Option<Value> {
    let targets = link
        .call_and_drain(Duration::from_secs(10), "Target.getTargets", &json!({}), None)
        .result
        .ok()?;
    targets
        .get("targetInfos")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target["type"] == "page" && target["url"].as_str() == Some(url))
                .cloned()
        })
}

/// The browser contexts of the judge's browser, without the default one.
#[cfg(target_os = "macos")]
fn browser_contexts(link: &mut crate::cdp::CdpLink) -> Vec<String> {
    let answer = link.call_and_drain(Duration::from_secs(10), "Target.getBrowserContexts", &json!({}), None);
    answer
        .result
        .expect("the browser contexts of the judge")
        .get("browserContextIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One real Chrome of this machine judges the private page of a browser host: a
/// shared page and a second shared page share a cookie and a stored value, the
/// private page reads neither, what the private page sets is read by no shared
/// page, and the browser context of the private page is new while the page is
/// open and gone after its close.
///
/// It runs on macOS only, because it starts the Chrome of the test lane at the
/// path the lane installs it, and it starts that browser itself with the few
/// switches of a throwaway profile: no profile of a person and no Keychain.
#[test]
#[cfg(target_os = "macos")]
#[ignore = "starts a real Chrome of the macOS test lane"]
fn a_real_chrome_keeps_a_private_page_apart_from_the_shared_profile() {
    let binary = std::path::Path::new(MACOS_CHROME);
    if !binary.is_file() {
        println!("skipped: this machine has no Chrome at {}", binary.display());
        return;
    }
    let profile = std::env::temp_dir().join(format!("horizon-private-judge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).expect("create the temporary profile of the judge");
    let child = std::process::Command::new(binary)
        .args([
            "--headless=new",
            "--use-mock-keychain",
            "--no-first-run",
            &format!("--user-data-dir={}", profile.display()),
            "--remote-debugging-port=0",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(child) = child else {
        println!("skipped: could not start {}", binary.display());
        return;
    };
    let mut browser = BrowserGuard(child);
    let Some(endpoint) = wait_for_endpoint(&profile.join("DevToolsActivePort")) else {
        browser.kill();
        let _ = std::fs::remove_dir_all(&profile);
        panic!("{} wrote no DevToolsActivePort", binary.display());
    };
    let pages = PageServer::start();
    let mut link = crate::cdp::CdpLink::connect(endpoint.ws_url()).expect("a link to the judge's browser");
    let before = browser_contexts(&mut link);

    // One shared page stores a cookie and a value that another page of the
    // profile reads.
    let first = judge_session(&endpoint, &pages.url("page-a.html"), false);
    let first_url = pages.url("page-a.html");
    let first_target = page_target(&mut link, &first_url).expect("the page of the profile");
    let first_target = first_target["targetId"].as_str().expect("a page target").to_string();
    assert_eq!(
        read_page(
            &mut link,
            &first_target,
            "document.cookie = 'probe=one; path=/'; localStorage.setItem('probe', 'one'); document.cookie"
        )
        .contains("probe=one"),
        true,
        "the shared page stores its cookie"
    );

    let second = judge_session(&endpoint, &pages.url("page-b.html"), false);
    let second_url = pages.url("page-b.html");
    let second_target = page_target(&mut link, &second_url).expect("the second page of the profile");
    let second_target = second_target["targetId"].as_str().expect("a page target").to_string();
    assert!(
        read_page(&mut link, &second_target, "document.cookie").contains("probe=one"),
        "a second page of the profile reads the cookie of the first"
    );
    assert_eq!(
        read_page(&mut link, &second_target, "localStorage.getItem('probe') || 'none'"),
        "one",
        "a second page of the profile reads the value of the first"
    );

    // The private page of the same browser reads neither, and what it stores is
    // read by no shared page.
    let private = judge_session(&endpoint, &pages.url("page-a.html?private=1"), true);
    let private_url = pages.url("page-a.html?private=1");
    let private_target = page_target(&mut link, &private_url).expect("the private page");
    assert!(
        private_target["browserContextId"].as_str().is_some(),
        "the private page is a page of a browser context of its own: {private_target}"
    );
    let private_target = private_target["targetId"].as_str().expect("a page target").to_string();
    assert_eq!(
        read_page(&mut link, &private_target, "document.cookie"),
        "",
        "the private page reads none of the cookies of the profile"
    );
    assert_eq!(
        read_page(&mut link, &private_target, "localStorage.getItem('probe') || 'none'"),
        "none",
        "the private page reads none of the stored values of the profile"
    );
    let _ = read_page(
        &mut link,
        &private_target,
        "document.cookie = 'secret=1; path=/'; localStorage.setItem('secret', '1'); document.cookie",
    );
    let during = browser_contexts(&mut link);
    assert_eq!(
        during.len(),
        before.len() + 1,
        "the browser holds one context more while the private page is open: {before:?} then {during:?}"
    );

    let private_signal = private.shutdown_signal();
    let _ = private_signal.wait(Duration::from_secs(20));
    let after = browser_contexts(&mut link);
    assert_eq!(
        after, before,
        "the close of the private page disposes its context: {before:?} then {after:?}"
    );

    assert_eq!(
        read_page(&mut link, &second_target, "document.cookie"),
        "probe=one",
        "the cookie of the private page reached no shared page"
    );
    assert_eq!(
        read_page(&mut link, &second_target, "localStorage.getItem('secret') || 'none'"),
        "none",
        "the value of the private page reached no shared page"
    );

    let first_signal = first.shutdown_signal();
    let _ = first_signal.wait(Duration::from_secs(20));
    let second_signal = second.shutdown_signal();
    let _ = second_signal.wait(Duration::from_secs(20));
    assert!(
        browser.0.try_wait().expect("state of the judge's browser").is_none(),
        "the sessions never stopped the browser of the judge"
    );
    browser.kill();
    let _ = std::fs::remove_dir_all(&profile);
}
