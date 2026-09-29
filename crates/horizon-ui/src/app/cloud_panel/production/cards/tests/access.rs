use super::*;
use crate::test_egui::DiscardTextures;

/// Every label the frame hands to assistive technology.
fn spoken(run: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run_ui(egui::RawInput::default(), run).discard_textures();
    output
        .platform_output
        .accesskit_update
        .map(|update| {
            update
                .nodes
                .iter()
                // egui gives static text its words as the value, controls theirs as the label.
                .filter_map(|(_, node)| node.label().or_else(|| node.value()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_header_status_and_every_step_reach_a_screen_reader() {
    let long = format!("error from registry: unauthenticated: {}", "token rejected ".repeat(30));
    let runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        error: Some(long.clone()),
        ..Default::default()
    };
    let failed = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
    let header = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(548.0, 118.0));
    let labels = spoken(|ui| {
        let indicators = strip::Indicators {
            running: 0,
            terminals: 0,
            desktop: None,
            sharing: strip::Sharing::Off,
            companions: 0,
        };
        let spend = strip::Spend {
            line: "No charges yet".into(),
            explanation: String::new(),
        };
        strip::show(ui, header, &failed, &indicators, &spend, false);
        steps::vertical(ui, &runtime, &failed);
    });
    let sentence = labels
        .iter()
        .find(|label| label.starts_with("Push failed"))
        .unwrap_or_else(|| panic!("no status sentence in {labels:?}"));
    assert!(
        sentence.contains(long.trim()),
        "the whole cause, however cut on screen: {sentence}"
    );
    assert!(sentence.contains("registry refused"), "{sentence}");
    for step in ["Validate: done", "Push image: failed", "Ready: pending"] {
        assert!(labels.iter().any(|label| label == step), "{step} in {labels:?}");
    }
}
