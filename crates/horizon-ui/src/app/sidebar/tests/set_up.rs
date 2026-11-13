//! The Set up control of the machine section.

use super::super::machines::{NO_MACHINE, NO_SERVER, SET_UP};
use crate::test_egui::DiscardTextures;
use egui::epaint::Shape;

/// The machine section and its Set up control are there before the first
/// machine: a person who starts with an empty window has to be able to get one.
#[test]
fn the_machine_section_says_where_a_machine_comes_from_while_the_list_is_empty() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let mut drawn: Vec<String> = Vec::new();
    // A panel shows from its second frame on.
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        drawn = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
    }

    assert!(drawn.iter().any(|text| text == "Machines"), "{drawn:?}");
    assert!(
        drawn.iter().any(|text| text == SET_UP),
        "the control is here: {drawn:?}"
    );
    assert!(drawn.iter().any(|text| text == NO_SERVER), "{drawn:?}");
    assert!(drawn.iter().any(|text| text == NO_MACHINE), "{drawn:?}");
    assert!(app.board.panels.is_empty(), "drawing the section starts nothing");
}

/// The Set up control of the machine section is a press that starts Set up.
#[test]
fn the_set_up_control_of_the_machine_section_presses_set_up() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.setup.integrations = false;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let frame = |app: &mut crate::app::HorizonApp, events: Vec<egui::Event>| {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        input.events = events;
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    let mut bounds = None;
    for _ in 0..2 {
        let output = frame(&mut app, Vec::new());
        let nodes = output.platform_output.accesskit_update.expect("an accessibility tree");
        bounds = nodes.nodes.iter().find_map(|(_, node)| {
            (node.label().or_else(|| node.value()) == Some(SET_UP))
                .then(|| node.bounds())
                .flatten()
        });
    }
    let bounds = bounds.expect("the control of the machine section has a position");
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
    let _ = frame(&mut app, vec![egui::Event::PointerMoved(position)]);
    let _ = frame(&mut app, vec![button(true)]);
    let _ = frame(&mut app, vec![button(false)]);

    assert!(
        app.setup.notice_panel.is_some(),
        "the press started Set up, which draws its notice without integrations"
    );
}
