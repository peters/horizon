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
    let header = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1400.0, 118.0));
    let labels = spoken(|ui| {
        let indicators = strip::Indicators {
            running: 1,
            terminals: 2,
            desktop: Some(false),
            sharing: strip::Sharing::Open(3),
            companions: 0,
        };
        let spend = strip::Spend {
            line: "$0.320/h · $1.02 run".into(),
            explanation: "Estimated from the worker's rate.".into(),
        };
        strip::show(ui, header, &failed, &indicators, &spend, false, None);
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
    assert!(!sentence.contains(".."), "{sentence}");
    for said in [
        "$0.320/h · $1.02 run. Estimated from the worker's rate",
        "1 of 2 terminals running",
        "Sharing the local network · 3 open",
    ] {
        assert!(labels.iter().any(|label| label == said), "{said} in {labels:?}");
    }
    assert!(
        !labels.iter().any(|label| label.starts_with("Desktop tunnel")),
        "an unconnected desktop is not an indicator: {labels:?}"
    );
    for step in ["Validate: done", "Push image: failed", "Ready: pending"] {
        assert!(labels.iter().any(|label| label == step), "{step} in {labels:?}");
    }
}
