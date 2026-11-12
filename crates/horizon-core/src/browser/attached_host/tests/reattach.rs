//! The page report of a panel that lost its link: the same page, or a new one.

use super::*;

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
        },
        &mut output,
    );

    assert!(!panel.page_resumed);
    assert_eq!(
        panel.navigation_error, None,
        "a first page lost nothing, so the panel says nothing"
    );
}
