use super::*;
use egui::{Event, PointerButton, RawInput};
use horizon_core::WorkspaceLayout;

fn ready_status() -> status::Status {
    let mut ready = status::of(
        &super::super::super::Runtime::default(),
        status::Occupancy::default(),
        std::time::SystemTime::now(),
    );
    ready.tone = status::Tone::Ready;
    ready
}

fn idle_indicators() -> strip::Indicators {
    strip::Indicators {
        running: 0,
        terminals: 0,
        desktop: None,
        sharing: strip::Sharing::Off,
        companions: 0,
    }
}

fn frame(
    ctx: &egui::Context,
    width: f32,
    status: &status::Status,
    indicators: &strip::Indicators,
    events: Vec<Event>,
) -> (Option<strip::StripAction>, Vec<(String, egui::Rect)>) {
    let header = egui::Rect::from_min_size(Pos2::ZERO, egui::Vec2::new(width, 118.0));
    let mut action = None;
    let output = ctx
        .run_ui(
            RawInput {
                events,
                ..RawInput::default()
            },
            |ui| {
                let spend = strip::Spend {
                    line: String::new(),
                    explanation: String::new(),
                };
                let layout = strip::LayoutControls {
                    selected: None,
                    color: egui::Color32::LIGHT_BLUE,
                };
                action = strip::show(ui, header, status, indicators, &spend, false, Some(layout)).action;
            },
        )
        .discard_textures();
    let texts = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.visual_bounding_rect())),
            _ => None,
        })
        .collect();
    (action, texts)
}

fn has(texts: &[(String, egui::Rect)], label: &str) -> bool {
    texts.iter().any(|(text, _)| text == label)
}

#[test]
fn a_ready_cloud_offers_its_layouts_on_the_header_and_applies_a_click() {
    let ctx = egui::Context::default();
    let ready = ready_status();
    let (_, texts) = frame(&ctx, 1100.0, &ready, &idle_indicators(), vec![]);
    for label in ["Default", "Rows", "Cols", "Grid"] {
        assert!(has(&texts, label), "{label} is on the header: {texts:?}");
    }
    let grid = texts.iter().find(|(text, _)| text == "Grid").unwrap().1.center();
    let button = |pressed| Event::PointerButton {
        pos: grid,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = frame(
        &ctx,
        1100.0,
        &ready,
        &idle_indicators(),
        vec![Event::PointerMoved(grid)],
    );
    let _ = frame(&ctx, 1100.0, &ready, &idle_indicators(), vec![button(true)]);
    let (action, _) = frame(&ctx, 1100.0, &ready, &idle_indicators(), vec![button(false)]);
    assert_eq!(action, Some(strip::StripAction::Layout(Some(WorkspaceLayout::Grid))));
}

#[test]
fn layouts_are_offered_while_the_cloud_takes_or_holds_panels_but_not_beside_a_failure() {
    let group = horizon_core::cloud_panel::CloudGroup::new(101, "test".into(), "ws".into(), ".".into(), [0.0, 0.0]);
    let none = status::Occupancy::default();
    let some = status::Occupancy { panels: 1, ..none };
    let failed = status::of(
        &super::super::super::Runtime {
            stage: Some(Stage::Push),
            error: Some("Push failed".into()),
            ..Default::default()
        },
        none,
        std::time::SystemTime::now(),
    );
    let mut stopped = ready_status();
    stopped.tone = status::Tone::Idle;
    assert!(
        view::layout_controls(&group, &ready_status(), none).is_some(),
        "a ready cloud"
    );
    assert!(
        view::layout_controls(&group, &stopped, some).is_some(),
        "a cloud holding a panel"
    );
    assert!(
        view::layout_controls(&group, &stopped, none).is_none(),
        "nothing to arrange yet"
    );
    assert!(
        view::layout_controls(&group, &failed, some).is_none(),
        "a failure keeps its room"
    );
}

#[test]
fn manage_offers_the_layouts_the_header_has_no_room_for() {
    let mut group = horizon_core::cloud_panel::CloudGroup::new(101, "test".into(), "ws".into(), ".".into(), [0.0, 0.0]);
    let none = status::Occupancy::default();
    let some = status::Occupancy { panels: 1, ..none };
    let failed = status::of(
        &super::super::super::Runtime {
            stage: Some(Stage::Push),
            error: Some("Push failed".into()),
            ..Default::default()
        },
        none,
        std::time::SystemTime::now(),
    );
    group.size[0] = 1200.0;
    assert!(
        view::manage_layout_controls(&group, &ready_status(), none).is_none(),
        "a wide header shows them"
    );
    assert!(
        view::manage_layout_controls(&group, &failed, some).is_some(),
        "a failed cloud with panels keeps them in Manage"
    );
    assert!(
        view::manage_layout_controls(&group, &failed, none).is_none(),
        "nothing to arrange"
    );
    group.size[0] = 560.0;
    assert!(
        view::manage_layout_controls(&group, &ready_status(), none).is_some(),
        "a narrow header leaves them to Manage"
    );
}

#[test]
fn a_narrow_header_keeps_its_sentence_and_leaves_layouts_to_manage() {
    let ctx = egui::Context::default();
    let (_, texts) = frame(&ctx, 548.0, &ready_status(), &idle_indicators(), vec![]);
    assert!(!has(&texts, "Rows"), "{texts:?}");
}

#[test]
fn header_indicators_name_what_is_in_use_and_hide_what_is_not() {
    let ctx = egui::Context::default();
    let ready = ready_status();
    let quiet = strip::Indicators {
        desktop: Some(true),
        ..idle_indicators()
    };
    let (_, texts) = frame(&ctx, 1400.0, &ready, &quiet, vec![]);
    assert!(has(&texts, "Desktop"), "{texts:?}");
    assert!(
        !texts
            .iter()
            .any(|(text, _)| text.starts_with("Network") || text.starts_with("Companions"))
    );
    let busy = strip::Indicators {
        sharing: strip::Sharing::Open(2),
        companions: 1,
        ..quiet
    };
    let (_, texts) = frame(&ctx, 1400.0, &ready, &busy, vec![]);
    assert!(has(&texts, "Network 2") && has(&texts, "Companions 1"), "{texts:?}");
}
