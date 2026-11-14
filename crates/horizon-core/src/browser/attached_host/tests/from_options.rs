//! The page of a private panel of a browser host, from the options of the create
//! to the commands that the browser of that machine receives.
//!
//! Every test here starts where a create starts: the options that the rule of
//! `crate::browser::new_browser_panel` filled for a panel of the browser host of
//! the settings. The stand-in `ssh` carries the endpoint of a mock `DevTools`
//! browser, and the test reads the browser-context commands that the session of
//! that panel sent.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::PageIsolation;
use serde_json::Value;

use super::cdp::MockCdpBrowser;
use super::*;
use crate::{BrowserHostConfig, PanelKind, PanelOptions, browser_panel_of_options};

/// The bound of a wait for a command of the session of a panel.
const COMMAND_DEADLINE: Duration = Duration::from_secs(20);

/// One mock browser that the probe of the stand-in `ssh` starts on the loopback
/// port of the forward that it was asked about.
type Slot = Arc<Mutex<Option<MockCdpBrowser>>>;

/// The options of a panel that an agent creates on the browser host of the
/// settings: the `profile` field of the create, as the rule reads it. A create
/// that writes no field is a page of the profile of that host.
fn host_panel_options(profile: Option<&str>) -> PanelOptions {
    let host = BrowserHostConfig {
        ssh_target: Some("browser-vm".to_string()),
        profile: Some(horizon_browser::DEFAULT_PROFILE.to_string()),
        ..BrowserHostConfig::default()
    };
    let mut opts = PanelOptions {
        kind: PanelKind::Browser,
        command: Some("https://example.test/".to_string()),
        local_id: Some("panel-of-a-host".to_string()),
        follows_browser_host: true,
        // The two fields that a create writes, as `browser_create_options`
        // writes them, and then the rule of the app, as the app runs it.
        browser_request: Some(crate::browser::BrowserPanelRequest {
            profile: profile.map(str::to_string),
            backend: None,
        }),
        ..PanelOptions::default()
    };
    crate::browser::place_browser_panel(&mut opts, &host, crate::browser::BackendKind::ChromiumCdp)
        .expect("the rule of a page of a browser host");
    opts
}

/// The `ssh` of a test, whose probe starts a mock browser on the loopback port
/// of the forward that it carries.
fn ssh_with_mock(program: &str, slot: &Slot, answer: Option<Answer>) -> Ssh {
    let slot = Arc::clone(slot);
    test_ssh(program, move |port: u16| {
        let mut started = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if started.is_some() {
            return true;
        }
        *started = Some(match &answer {
            Some((answer, replace)) => MockCdpBrowser::answering(port, Arc::clone(answer), Some(*replace)),
            None => MockCdpBrowser::start(port),
        });
        true
    })
}

/// One answer that a mock browser sends instead of the ordinary answer of the
/// method `replace`.
type Answer = (Arc<dyn Fn(&Value) -> Value + Send + Sync>, &'static str);

/// Wait until the session of `panel` sent a command of `method`, and drain the
/// panel while the connect and the session start.
fn wait_for_command(panel: &mut BrowserPanelState, browser: &MockCdpBrowser, method: &str) -> bool {
    let deadline = Instant::now() + COMMAND_DEADLINE;
    while Instant::now() < deadline {
        panel.drain_events();
        if browser.methods().iter().any(|seen| seen == method) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// The mock browser of `slot`, once the connect of the panel has started it. The
/// panel drains while the connect runs, because the outcome of a connect reaches
/// the session of the panel on a frame of that panel.
fn mock_after_connect(panel: &mut BrowserPanelState, slot: &Slot) -> MockCdpBrowser {
    let deadline = Instant::now() + COMMAND_DEADLINE;
    loop {
        panel.drain_events();
        if let Some(browser) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            return browser.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the connect of the panel started no mock browser"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The stand-in `ssh` of a browser host: it prints the endpoint file of the
/// browser of that machine, on the path that the connect reads.
fn stand_in_ssh(directory: &Path, log: &Path) -> String {
    stand_in(
        directory,
        log,
        &format!("printf '%s\\n' {}", script_text("41000\n/devtools/browser/mock")),
    )
}

#[test]
fn a_private_panel_of_a_browser_host_is_a_page_of_a_context_of_its_own() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in_ssh(directory.path(), &log);
    let slot: Slot = Arc::default();
    let mut opts = host_panel_options(Some(horizon_browser::PRIVATE_PROFILE));

    let ssh = ssh_with_mock(&program, &slot, None);
    let mut panel = browser_panel_of_options(&mut opts, ssh).expect("the page of a private panel of that host");
    let browser = mock_after_connect(&mut panel, &slot);

    assert!(
        wait_for_command(&mut panel, &browser, "Target.createTarget"),
        "the session of a private panel makes its page: {:?}",
        browser.methods()
    );
    wait_until(&mut panel, |panel| panel.page_isolation_report().is_some());
    assert_eq!(
        panel.page_isolation_report(),
        Some(PageIsolation::IsolatedContext),
        "the session proves the context that makes this page private"
    );
    assert!(
        panel.is_private_page(),
        "the panel says Private because its page is in a context of its own"
    );
    assert_eq!(panel.reported_profile(), horizon_browser::PRIVATE_PROFILE);

    assert_eq!(
        panel.user_navigation_count(),
        0,
        "the create of a host page counts no navigation of the person: S3 probe"
    );
    let commands = browser.commands();
    let methods = browser.methods();
    let context = methods
        .iter()
        .position(|method| method == "Target.createBrowserContext")
        .expect("the context of the page");
    let created = methods
        .iter()
        .position(|method| method == "Target.createTarget")
        .expect("the page of the context");
    assert!(context < created, "the context comes before its page: {methods:?}");
    assert_eq!(
        commands[created]["params"]["browserContextId"], "ctx-private",
        "the page of a private panel carries the context of its own: {}",
        commands[created]
    );
    assert!(
        !methods.iter().any(|method| method == "Target.getTargets"),
        "a private page adopts no page of the profile of that machine: {methods:?}"
    );

    panel.stop();
    let deadline = Instant::now() + COMMAND_DEADLINE;
    while Instant::now() < deadline
        && !browser
            .methods()
            .iter()
            .any(|method| method == "Target.disposeBrowserContext")
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        browser
            .methods()
            .iter()
            .any(|method| method == "Target.disposeBrowserContext"),
        "the close of the panel disposes the context of its page: {:?}",
        browser.methods()
    );
    let commands = browser.commands();
    let disposed = commands
        .iter()
        .find(|command| command["method"] == "Target.disposeBrowserContext")
        .expect("the disposed context");
    assert_eq!(disposed["params"]["browserContextId"], "ctx-private");
}

#[test]
fn a_shared_panel_of_a_browser_host_sends_no_browser_context_command() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in_ssh(directory.path(), &log);
    let slot: Slot = Arc::default();
    let mut opts = host_panel_options(None);

    let ssh = ssh_with_mock(&program, &slot, None);
    let mut panel = browser_panel_of_options(&mut opts, ssh).expect("the page of a shared panel of that host");
    let browser = mock_after_connect(&mut panel, &slot);

    assert!(
        wait_for_command(&mut panel, &browser, "Target.createTarget"),
        "the session of a shared panel makes its page: {:?}",
        browser.methods()
    );
    wait_until(&mut panel, |panel| panel.page_isolation_report().is_some());
    assert_eq!(
        panel.page_isolation_report(),
        Some(PageIsolation::Shared),
        "a page of the profile of that machine keeps its sign-ins"
    );
    assert!(!panel.is_private_page(), "a shared page is no private page");
    assert_eq!(panel.reported_profile(), horizon_browser::DEFAULT_PROFILE);

    let methods = browser.methods();
    for forbidden in ["Target.createBrowserContext", "Target.disposeBrowserContext"] {
        assert!(
            !methods.iter().any(|method| method == forbidden),
            "a shared page sends no {forbidden}: {methods:?}"
        );
    }
    let created = browser
        .commands()
        .into_iter()
        .find(|command| command["method"] == "Target.createTarget")
        .expect("the page of a shared panel");
    assert!(
        created["params"].get("browserContextId").is_none(),
        "the page of a shared panel carries no context: {created}"
    );
    panel.stop();
}

#[test]
fn a_private_panel_of_a_browser_that_refuses_a_context_says_so_and_opens_no_page() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in_ssh(directory.path(), &log);
    let slot: Slot = Arc::default();
    let mut opts = host_panel_options(Some(horizon_browser::PRIVATE_PROFILE));
    let answer: Answer = (
        Arc::new(|_command: &Value| serde_json::json!({})),
        "Target.createBrowserContext",
    );

    let ssh = ssh_with_mock(&program, &slot, Some(answer));
    let mut panel = browser_panel_of_options(&mut opts, ssh).expect("the panel of a private page");
    let browser = mock_after_connect(&mut panel, &slot);

    assert!(
        wait_for_command(&mut panel, &browser, "Target.createBrowserContext"),
        "the session asks for the context of its private page: {:?}",
        browser.methods()
    );
    wait_until(&mut panel, |panel| {
        matches!(
            panel.status,
            crate::browser::BrowserStatus::Error { .. } | crate::browser::BrowserStatus::Stopped { .. }
        )
    });
    assert_eq!(
        panel.page_isolation(),
        PageIsolation::Shared,
        "the session proves nothing, so nothing calls the page private"
    );
    assert!(
        !panel.is_private_page(),
        "a page that nothing proved isolated is never called private"
    );
    assert_eq!(
        panel.reported_profile(),
        super::super::DEFAULT_HOST_PROFILE,
        "the panel reports the profile of that machine instead of a private page"
    );
    let methods = browser.methods();
    assert!(
        !methods.iter().any(|method| method == "Target.createTarget"),
        "no page of the shared profile is opened in its place: {methods:?}"
    );
    panel.stop();
}
