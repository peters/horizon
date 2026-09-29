//! The companion rows a source cloud's card shows while agents' operations run.
use super::tests::groups;
use super::*;
use crate::test_egui::DiscardTextures as _;

#[test]
fn a_companion_starting_on_its_card_cannot_be_unchecked_until_that_finishes() {
    use horizon_core::cloud_runtime::companions::{Catalog, Companion, Row, Snapshot, Status};
    let companion = Companion {
        alias: "app".into(),
        repository: "example/app".into(),
        profile: "dev".into(),
        target_cloud_id: Some("target".into()),
        selected: true,
        status: Status::Stopped,
        access: None,
    };
    let snapshot = Snapshot {
        catalog: Catalog {
            version: 1,
            source_cloud_id: "source".into(),
            observed_at: 0,
            companions: vec![companion.clone()],
        },
        rows: vec![Row {
            companion,
            candidates: Vec::new(),
            error: None,
        }],
        publication_error: None,
        notice: None,
    };
    for starting in [true, false] {
        let mut state = State::default();
        state.sync(Some("session"), &groups());
        let entry = state.entries.get_mut("source").unwrap();
        entry.snapshot = Some(snapshot.clone());
        let running: std::collections::BTreeSet<String> = if starting {
            ["target".to_owned()].into()
        } else {
            std::collections::BTreeSet::new()
        };
        let ctx = egui::Context::default();
        let frame = |entry: &mut Entry, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ui| view::render(ui, entry, &running),
            )
        };
        // Find the checkbox by its label, then click it.
        let output = frame(entry, Vec::new());
        let label = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "app" => Some(text.visual_bounding_rect().center()),
                _ => None,
            })
            .expect("the companion's checkbox label is shown");
        let _ = output.discard_textures();
        for pressed in [true, false] {
            let events = vec![
                egui::Event::PointerMoved(label),
                egui::Event::PointerButton {
                    pos: label,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let _ = frame(entry, events).discard_textures();
        }
        assert_eq!(entry.clearing.contains("app"), !starting, "starting: {starting}");
    }
}
