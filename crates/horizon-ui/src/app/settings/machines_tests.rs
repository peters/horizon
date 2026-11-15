//! The row of the Settings window that shows the agents of each machine.

use egui::Context;
use horizon_core::Config;

use crate::test_egui::DiscardTextures;

/// The words of the check box that the test presses.
const LABEL: &str = "Show the agents of each machine";

/// One render of the General tab, with `events`, and the accessibility tree of it.
fn render(ctx: &Context, config: &mut Config, events: Vec<egui::Event>) -> egui::FullOutput {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 3000.0));
    let mut input = egui::RawInput {
        screen_rect: Some(rect),
        events,
        ..egui::RawInput::default()
    };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(rect);
    ctx.run_ui(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            super::general::render(ui, config, super::general::GeneralView::default());
        });
    })
    .discard_textures()
}

#[test]
fn the_setting_that_shows_the_agents_of_each_machine_is_a_check_box_that_writes_it() {
    let ctx = Context::default();
    ctx.enable_accesskit();
    let mut config = Config::default();
    assert!(config.machines.watch_agents, "the watch of the machines is on");
    let mut center = None;
    for _ in 0..2 {
        let output = render(&ctx, &mut config, Vec::new());
        let nodes = output
            .platform_output
            .accesskit_update
            .expect("the accessibility tree of the settings");
        center = nodes.nodes.iter().find_map(|(_, node)| {
            let text = node.label().or_else(|| node.value());
            text.is_some_and(|text| text.starts_with(LABEL))
                .then(|| node.bounds())
                .flatten()
        });
    }
    let bounds = center.expect("the check box of the machines has a position");
    #[allow(clippy::cast_possible_truncation)]
    let position = egui::Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    );
    let button = |pressed: bool| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let _ = render(&ctx, &mut config, vec![egui::Event::PointerMoved(position)]);
    let _ = render(&ctx, &mut config, vec![button(true)]);
    let _ = render(&ctx, &mut config, vec![button(false)]);

    assert!(
        !config.machines.watch_agents,
        "the press turns the watch of the machines off in the settings of this window"
    );
}
