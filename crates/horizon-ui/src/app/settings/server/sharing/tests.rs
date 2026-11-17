use egui::{Event, PointerButton, Pos2, RawInput, Rect};
use horizon_core::{Config, SHARED_TERMINALS_NOTICE, ServerConfig};

use super::super::{ServerTabState, ServerView};
use super::{SEE, TYPE, render};
use crate::test_egui::DiscardTextures;

/// One frame of the section with `events`: the texts that it drew with their
/// places, and whether it changed the settings.
fn frame(ctx: &egui::Context, server: &mut ServerConfig, events: Vec<Event>) -> (Vec<(String, Rect)>, bool) {
    drawn(ctx, events, |ui| render(ui, server))
}

/// One frame of `content` in a window that holds a whole tab: the texts that
/// it drew with their places, and what it answered.
fn drawn(
    ctx: &egui::Context,
    events: Vec<Event>,
    mut content: impl FnMut(&mut egui::Ui) -> bool,
) -> (Vec<(String, Rect)>, bool) {
    let mut changed = false;
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(520.0, 2400.0))),
        events,
        ..RawInput::default()
    };
    let output = ctx
        .run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| changed = content(ui));
        })
        .discard_textures();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut texts);
    }
    (texts, changed)
}

fn collect(shape: &egui::Shape, texts: &mut Vec<(String, Rect)>) {
    match shape {
        egui::Shape::Text(text) => texts.push((
            text.galley.text().to_owned(),
            Rect::from_min_size(text.pos, text.galley.size()),
        )),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, texts)),
        _ => {}
    }
}

/// Presses the text `label` of the section, and answers whether a frame of the
/// press changed the settings.
fn press(ctx: &egui::Context, server: &mut ServerConfig, label: &str) -> bool {
    let (texts, _) = frame(ctx, server, Vec::new());
    let at = texts
        .iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("the section draws {label}: {texts:?}"))
        .1
        .center();
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut changed = false;
    for events in [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
        changed |= frame(ctx, server, events).1;
    }
    changed
}

#[test]
fn the_section_says_what_sharing_means_and_its_switches_change_the_settings() {
    let ctx = egui::Context::default();
    let mut server = ServerConfig::default();
    let (texts, changed) = frame(&ctx, &mut server, Vec::new());
    assert!(!changed, "a frame that only draws changes nothing");
    for words in [
        "Shared terminals",
        SHARED_TERMINALS_NOTICE,
        "Name of this computer",
        SEE,
        TYPE,
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == words),
            "the section draws {words}: {texts:?}"
        );
    }
    assert!(server.share_terminals && server.remote_typing, "both switches start on");

    assert!(
        press(&ctx, &mut server, TYPE),
        "a press on the second switch is a change"
    );
    assert!(server.share_terminals && !server.remote_typing);
    assert!(press(&ctx, &mut server, SEE), "a press on the first switch is a change");
    assert!(!server.share_terminals);

    // Nobody types in a terminal that no other computer sees, so the second
    // switch takes no press while the first one is off.
    assert!(!press(&ctx, &mut server, TYPE));
    assert!(!server.remote_typing, "the switch keeps its value");
}

/// The sentence about sharing is in the tab once: in the join of a computer
/// that has no link yet, and beside the switches of a computer that has one.
#[test]
fn the_tab_says_what_sharing_means_where_a_computer_links_and_where_it_is_linked() {
    for linked in [false, true] {
        let view = ServerView {
            line: "Server: in step".to_owned(),
            device: "window-b".to_owned(),
            linked,
            ..ServerView::default()
        };
        let (mut config, mut state) = (Config::default(), ServerTabState::default());
        let ctx = egui::Context::default();
        let (texts, _) = drawn(&ctx, Vec::new(), |ui| {
            super::super::render(ui, &mut config, &mut state, &view)
        });
        let count = |words: &str| texts.iter().filter(|(text, _)| text == words).count();
        assert_eq!(count(SHARED_TERMINALS_NOTICE), 1, "linked {linked}: {texts:?}");
        assert_eq!(count("Join with a link"), 1, "the join is always there: {texts:?}");
        assert_eq!(
            count(SEE),
            usize::from(linked),
            "the switches belong to a linked computer"
        );
    }
}
