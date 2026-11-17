//! The link of a card: its label, the host under it, and the press that asks
//! Horizon to open the page.

use horizon_browser_control::block::{BlockDocument, Component};

use super::{CardFrames, Owner};
use crate::block_widget::link::OPENS_IN_BROWSER_PANEL;
use crate::theme;

use super::{card_nodes, card_nodes_in, disabled_state};

/// A card of one link: `label` over `url`.
fn link(label: &str, url: &str) -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Link {
            label: label.to_owned(),
            url: url.to_owned(),
        }],
    }
}

/// Every text of a card that draws `document` for `owner`, in both themes.
fn texts_in_both_themes(document: &BlockDocument, owner: Owner) -> Vec<String> {
    let mut texts = Vec::new();
    for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
        texts.extend(
            card_nodes_in(resolved, document, owner)
                .into_iter()
                .map(|(text, _)| text),
        );
    }
    texts
}

#[test]
fn a_link_draws_its_label_and_the_host_of_its_address() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    for owner in Owner::ALL {
        let texts = texts_in_both_themes(&document, owner);
        for text in ["Open session", "cloud.example.test"] {
            assert!(
                texts.iter().any(|drawn| drawn == text),
                "{text} must be drawn with {owner:?}: {texts:?}"
            );
        }
    }
}

#[test]
fn an_odd_address_shows_the_host_that_the_browser_resolves() {
    for (url, host) in [
        // A Cyrillic `a` spells the same as the Latin one on screen.
        ("https://\u{430}pple.example/session", "xn--pple-43d.example"),
        ("https://m\u{fc}nchen.example/", "xn--mnchen-3ya.example"),
        ("https://cloud.example.test:8443/", "cloud.example.test:8443"),
        (
            &format!("https://cloud.example.test/{}", "p".repeat(1500)),
            "cloud.example.test",
        ),
        ("https://[::1]:8443/session", "[::1]:8443"),
    ] {
        let texts = texts_in_both_themes(&link("Open session", url), Owner::Listening);
        assert!(
            texts.iter().any(|drawn| drawn == host),
            "{url} must show its own host {host}: {texts:?}"
        );
    }
}

#[test]
fn a_link_stays_live_with_the_agent_gone() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    let nodes = card_nodes(&document, Owner::Gone);

    assert_eq!(
        disabled_state(&nodes, "Open session"),
        Some(false),
        "a link needs no agent: {nodes:?}"
    );
    // The host of the address is a text of the card, not a control.
    assert!(
        nodes.iter().any(|(text, _)| text == "cloud.example.test"),
        "the card still says where the link goes: {nodes:?}"
    );
}

#[test]
fn a_press_on_a_link_asks_for_its_address_and_records_no_click() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    for owner in Owner::ALL {
        let frames = CardFrames::new();
        let mut card = horizon_core::CardRuntime::default();

        let opened = frames.press_link(&document, &mut card, owner, "Open session");

        assert_eq!(
            opened.as_deref(),
            Some("https://cloud.example.test/sessions/7"),
            "the press asks for the page of the link, with {owner:?}"
        );
        let now = 1_000_000;
        assert_eq!(card.pending_clicks(now), 0, "a link records no click: {owner:?}");
        assert_eq!(card.take_click(now), None, "a link records no click: {owner:?}");
    }
}

#[test]
fn the_host_of_a_link_is_read_once_for_one_document() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    let mut card = horizon_core::CardRuntime::default();
    assert_eq!(
        card.link_host(4, 0, &document),
        Some("cloud.example.test"),
        "the host of the address"
    );

    // The card keeps the host it read for the revision, so a frame that draws
    // the same document reads no URL again.
    let changed = link("Open session", "https://other.example.test/sessions/7");
    assert_eq!(
        card.link_host(4, 0, &changed),
        Some("cloud.example.test"),
        "a frame of the same revision draws the host it read"
    );
    assert_eq!(
        card.link_host(5, 0, &changed),
        Some("other.example.test"),
        "a new document reads its links again"
    );
}

#[test]
fn a_press_under_a_host_dialog_records_nothing() {
    // The card that a dialog covers takes no press, so the link asks for no page
    // either: `interactive` false is what a dialog does to every card.
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    let frames = CardFrames::new();
    let mut card = horizon_core::CardRuntime::default();
    let opened = frames.press_link_when_covered(&document, &mut card, "Open session");

    assert_eq!(opened, None, "{OPENS_IN_BROWSER_PANEL} is a hint, not a press");
}

#[test]
fn a_link_that_opened_nothing_draws_the_reason_under_it_in_the_danger_tone() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    let mut card = horizon_core::CardRuntime::default();
    card.fail_link(
        3,
        "https://cloud.example.test/sessions/7",
        "no Chrome/Chromium binary found",
    );

    let texts = super::texts_of_card_with(&document, &mut card, Owner::Listening);

    assert!(
        texts
            .iter()
            .any(|text| text == "Could not open: no Chrome/Chromium binary found"),
        "the card says why the page did not open: {texts:?}"
    );
}

#[test]
fn a_link_that_opened_its_page_draws_no_line_of_a_refused_press() {
    let document = link("Open session", "https://cloud.example.test/sessions/7");
    let mut card = horizon_core::CardRuntime::default();
    card.fail_link(
        3,
        "https://cloud.example.test/sessions/7",
        "no Chrome/Chromium binary found",
    );
    card.clear_link_failure("https://cloud.example.test/sessions/7");

    let texts = super::texts_of_card_with(&document, &mut card, Owner::Listening);

    assert!(
        !texts.iter().any(|text| text.starts_with("Could not open")),
        "a press that opened its page leaves no line: {texts:?}"
    );
}
