//! The page report of a panel that lost its link: the same page, or a new one.

use super::*;

/// A panel of a private page of `host`, whose session proved a context of its own.
fn panel_of_a_private_page(host: &AttachedHost) -> BrowserPanelState {
    let config = crate::browser::BrowserConfig {
        profile: Some(horizon_browser::PRIVATE_PROFILE.to_string()),
        ..crate::browser::BrowserConfig::default()
    };
    let mut panel = BrowserPanelState::start_attached_host_with(
        Ssh::default(),
        "panel-private",
        &config,
        Some("https://example.test/".to_string()),
        host,
    );
    panel.set_page_isolation_for_tests(horizon_browser::PageIsolation::IsolatedContext);
    panel
}

/// A panel that lost its link and is inside its automatic attempt.
fn panel_in_an_automatic_attempt(host: &AttachedHost) -> BrowserPanelState {
    let mut panel = BrowserPanelState::attached_host_for_tests(host, "https://example.test/inbox");
    panel.attached_reached_ready();
    panel.start_automatic_attempt(None);
    panel
}

#[test]
fn a_page_that_came_back_after_a_lost_link_keeps_its_state_and_says_so() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel_in_an_automatic_attempt(&host);

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::PageAttached {
            target_id: "the-old-page".to_string(),
            resumed: true,
            context: None,
        },
        &mut output,
    );

    assert!(panel.page_resumed, "the same page is back: no page state was lost");
    assert_eq!(
        panel.navigation_error, None,
        "the panel does not claim a lost page when the page came back"
    );
    assert!(output.had_output);
}

#[test]
fn a_resume_target_that_is_gone_says_that_the_page_is_new() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel_in_an_automatic_attempt(&host);

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::PageAttached {
            target_id: "a-new-page".to_string(),
            resumed: false,
            context: None,
        },
        &mut output,
    );

    assert!(!panel.page_resumed);
    assert_eq!(
        panel.navigation_error.as_deref(),
        Some(crate::browser::attached_host::PAGE_STATE_LOST),
        "the panel says that the page is new and that what it held is gone"
    );
    assert!(
        panel.attached_page_target() == Some("a-new-page"),
        "the next lost link resumes the page that the person has now"
    );
}

#[test]
fn a_first_page_of_a_browser_host_reports_no_page_state_loss() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = BrowserPanelState::attached_host_for_tests(&host, "https://example.test/");

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::PageAttached {
            target_id: "the-first-page".to_string(),
            resumed: false,
            context: None,
        },
        &mut output,
    );

    assert!(!panel.page_resumed);
    assert_eq!(
        panel.navigation_error, None,
        "a first page lost nothing, so the panel says nothing"
    );
}

/// A private page whose context the browser of the host no longer held is a new
/// private page, and the panel says so even when a person asked for the attempt:
/// the page keeps nothing either way, and what the page before held is gone.
#[test]
fn a_private_page_of_a_lost_context_says_that_it_is_a_new_private_page() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel_of_a_private_page(&host);

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::PageAttached {
            target_id: "a-new-private-page".to_string(),
            resumed: false,
            context: Some("a-new-private-context".to_string()),
        },
        &mut output,
    );

    assert_eq!(
        panel.navigation_error.as_deref(),
        Some(crate::browser::attached_host::PRIVATE_PAGE_LOST),
        "the panel says that the context of its private page went"
    );
    assert!(!panel.page_resumed, "a new private page is no page that came back");
    assert_eq!(
        panel.attached_page_context(),
        Some("a-new-private-context"),
        "the next lost link resumes inside the context of the page that the person has now"
    );
}

/// A page of the profile that a person asked to come back says the ordinary
/// words, and never the words of a private page.
#[test]
fn a_shared_page_that_is_new_says_that_its_page_is_new() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel_in_an_automatic_attempt(&host);

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::PageAttached {
            target_id: "a-new-page".to_string(),
            resumed: false,
            context: None,
        },
        &mut output,
    );

    assert_eq!(
        panel.navigation_error.as_deref(),
        Some(crate::browser::attached_host::PAGE_STATE_LOST)
    );
}
