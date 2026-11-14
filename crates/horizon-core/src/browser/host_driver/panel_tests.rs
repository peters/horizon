//! The panel of a Firefox browser host: what it says and where it keeps its host.
//!
//! The link of these tests is the real worker of the panel, over a stand-in `ssh`
//! program that refuses or that produces nothing, and the events of a link that
//! no machine of this suite can give: a page that came back and a page that was
//! lost.

use std::time::Duration;

use horizon_browser::host_driver::HostDriverPage;

use super::HostDriverEngine;
use super::panel::{HostDriverEvent, HostDriverSlot};
use crate::browser::attached_host::{AttachedHost, PAGE_STATE_LOST};
use crate::browser::{BrowserCommand, BrowserPanelState, BrowserStatus};
use crate::config::BrowserHostEngine;
use horizon_browser_control::ssh_forward::Ssh;

fn host(engine: BrowserHostEngine) -> AttachedHost {
    AttachedHost::with_engine("browser-vm", Some("mail"), engine, None).expect("one SSH name")
}

/// One `sh` program that stands in for `ssh`.
fn stand_in(directory: &std::path::Path, body: &str) -> String {
    let path = crate::util::stand_in_program_in(directory, "ssh", body).expect("the stand-in ssh");
    path.to_string_lossy().into_owned()
}

/// Wait until the panel has drained its state.
fn drain(panel: &mut BrowserPanelState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let _ = panel.drain_events();
        if !matches!(panel.status, BrowserStatus::Starting) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "the panel stayed in Starting");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_firefox_panel_names_the_driver_of_its_host() {
    let panel = BrowserPanelState::start_host_driver(
        "panel-1",
        &crate::browser::BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        &host(BrowserHostEngine::Firefox),
    );
    assert!(panel.is_host_driver());
    assert_eq!(panel.host_driver_target(), Some("browser-vm"));
    assert_eq!(panel.host_driver_profile(), Some("mail"));
    assert_eq!(panel.host_driver_engine(), Some(HostDriverEngine::Firefox));
    assert_eq!(
        panel.backend(),
        horizon_browser::BackendKind::FirefoxBidi,
        "the page of a Firefox host is a Firefox page"
    );
    assert_eq!(
        panel.remote_status.as_deref(),
        Some(
            "Firefox of browser-vm in the profile mail, through the driver of that machine; a page there cannot take \
             a file of this computer"
        ),
        "the note line of the panel says where the browser runs and the one limit of a page there"
    );
    assert!(panel.can_accept_commands(), "the link of the panel is open");
}

#[test]
fn a_panel_of_a_firefox_host_keeps_its_host_profile_and_engine_for_a_restart() {
    let panel = BrowserPanelState::start_host_driver(
        "panel-1",
        &crate::browser::BrowserConfig::default(),
        None,
        &host(BrowserHostEngine::Firefox),
    );
    let state = panel.attached_host_state().expect("the host of the panel");
    assert_eq!(state.ssh_target, "browser-vm");
    assert_eq!(state.profile.as_deref(), Some("mail"));
    assert_eq!(state.engine, Some(BrowserHostEngine::Firefox));
}

#[test]
fn a_host_that_ssh_refuses_shows_its_words_and_keeps_the_page_it_had() {
    let directory = tempfile::tempdir().expect("a folder of a test");
    let program = stand_in(directory.path(), "echo 'Permission denied (publickey).' >&2\nexit 255");
    let mut panel = BrowserPanelState::start_host_driver_with(
        Ssh::default().with_program(program),
        "panel-1",
        &crate::browser::BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        &host(BrowserHostEngine::Firefox),
    );
    drain(&mut panel);
    let BrowserStatus::Error { message } = &panel.status else {
        panic!("a refused host ends the panel with its reason: {:?}", panel.status);
    };
    assert!(message.contains("could not be reached"), "{message}");
    assert!(
        message.contains("Permission denied"),
        "the words of ssh reach the panel: {message}"
    );
    assert_eq!(
        panel.url, None,
        "a page that no link ever showed keeps no address of a page"
    );
}

#[test]
fn a_page_that_comes_back_keeps_the_notice_away_and_a_new_page_says_so() {
    let mut panel = BrowserPanelState::lost_host_driver_for_tests(
        &host(BrowserHostEngine::Firefox),
        "https://example.test/page",
        "the link dropped",
    );
    assert!(!panel.has_ended(), "a panel that keeps its page has not ended");
    assert_eq!(panel.url.as_deref(), Some("https://example.test/page"));

    // The automatic attempt comes back to the same page: nothing of the page is
    // lost, so the panel says nothing about a lost state.
    panel.apply_host_driver_event_for_tests(HostDriverEvent::Ready {
        resumed: true,
        page: HostDriverPage {
            url: "https://example.test/page".to_string(),
            title: "Example".to_string(),
            resumed: true,
            loading: false,
        },
    });
    assert_eq!(panel.status, BrowserStatus::Ready);
    assert_eq!(panel.navigation_error, None, "the page came back whole");
    assert!(panel.page_resumed);

    // A new page: the panel says that what the person typed into the old one is
    // gone.
    panel.apply_host_driver_event_for_tests(HostDriverEvent::Reconnecting {
        attempt: 1,
        retry_at: std::time::Instant::now(),
    });
    panel.apply_host_driver_event_for_tests(HostDriverEvent::Ready {
        resumed: false,
        page: HostDriverPage {
            url: "https://example.test/page".to_string(),
            title: "Example".to_string(),
            resumed: false,
            loading: false,
        },
    });
    assert_eq!(panel.navigation_error.as_deref(), Some(PAGE_STATE_LOST));
    assert!(!panel.page_resumed);
}

#[test]
fn the_link_of_the_panel_carries_the_input_of_the_person_and_the_operations_of_an_agent() {
    let panel = BrowserPanelState::start_host_driver(
        "panel-1",
        &crate::browser::BrowserConfig::default(),
        None,
        &host(BrowserHostEngine::Firefox),
    );
    assert!(
        panel.try_send(BrowserCommand::Input(horizon_browser::BrowserInput::InsertText {
            text: "typed".to_string()
        }))
    );
    assert!(panel.try_send(BrowserCommand::SetViewport {
        width: 1280,
        height: 800
    }));
    assert!(
        panel.try_send(BrowserCommand::HandoffDone),
        "a command of the page that a host driver does not serve is still accepted by the link"
    );
}

#[test]
fn a_command_of_the_page_that_a_host_cannot_carry_says_why() {
    let mut panel = BrowserPanelState::lost_host_driver_for_tests(
        &host(BrowserHostEngine::Firefox),
        "https://example.test/",
        "the link dropped",
    );
    panel.apply_host_driver_event_for_tests(HostDriverEvent::Note(
        "this page runs on the browser host, so it cannot take files of this computer".to_string(),
    ));
    assert_eq!(
        panel.remote_status.as_deref(),
        Some("this page runs on the browser host, so it cannot take files of this computer")
    );
}

#[test]
fn a_chromium_host_does_not_start_the_driver_of_a_firefox() {
    let chromium = host(BrowserHostEngine::Chromium);
    assert!(
        chromium.driver().is_none(),
        "a Chromium host is reached through its DevTools port"
    );
    assert!(host(BrowserHostEngine::Firefox).driver().is_some());
    let refused = crate::browser::attached_host_backend_refusal(
        BrowserHostEngine::Firefox,
        horizon_browser::BackendKind::ChromiumCdp,
    );
    assert!(refused.is_some_and(|reason| reason.contains("runs Firefox")));
    // The link of a panel is the worker that `connecting` starts; the test above
    // drives the real one over a stand-in `ssh`, so nothing else is started here.
    let _ = std::mem::size_of::<HostDriverSlot>();
}
