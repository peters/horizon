//! The way to set up a machine and a server: the icon control of the caption of the
//! machine section, and the link of the footer.
//!
//! A person who starts with an empty window has to be able to get a machine and a
//! server, so the control and the link stand there before the first machine. Both
//! take the press of the text button `Set up` of today.

use crate::app::sidebar::caption;
use crate::app::sidebar::footer;
use crate::app::sidebar::machines::SET_UP_HOVER;
use crate::test_egui::DiscardTextures;
use egui::epaint::Shape;

/// The drawn texts of the sidebar of `app`, after the frames that settle it.
fn drawn_texts(ctx: &egui::Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    let mut drawn = Vec::new();
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
    drawn
}

/// The machine section says where a machine comes from in its caption and in the
/// control of that caption: a person with an empty window finds the way there.
#[test]
fn the_machine_section_offers_a_machine_before_the_first_one() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.orchestrator_dock = true;
    app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::following(true);
    let ctx = egui::Context::default();
    let drawn = drawn_texts(&ctx, &mut app);

    assert!(drawn.iter().any(|text| text == caption::MACHINES), "{drawn:?}");
    assert!(
        drawn.iter().any(|text| text == footer::NO_SERVER),
        "the footer says that no server is set: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|text| text == footer::SET_UP),
        "the link of the footer is here: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|text| text == "+ Rent a machine"),
        "the control that rents a machine is here: {drawn:?}"
    );
    assert_eq!(
        drawn.iter().filter(|text| text.as_str() == "Set up").count(),
        1,
        "the only Set up of the sidebar is the link of the footer: {drawn:?}"
    );
    assert!(app.board.panels.is_empty(), "drawing the section starts nothing");
}

/// The centre of the control that a screen reader names `label`.
fn centre_of(ctx: &egui::Context, app: &mut crate::app::HorizonApp, label: &str) -> egui::Pos2 {
    ctx.enable_accesskit();
    let mut bounds = None;
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        let nodes = output.platform_output.accesskit_update.expect("an accessibility tree");
        bounds = nodes.nodes.iter().find_map(|(_, node)| {
            (node.label().or_else(|| node.value()) == Some(label))
                .then(|| node.bounds())
                .flatten()
        });
        if bounds.is_some() {
            break;
        }
    }
    let bounds = bounds.unwrap_or_else(|| panic!("the control {label} has a position"));
    #[allow(clippy::cast_possible_truncation)]
    egui::Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    )
}

/// A press at `pos`, in the frames that egui needs for a click.
fn press(ctx: &egui::Context, app: &mut crate::app::HorizonApp, pos: egui::Pos2) {
    let frame = |app: &mut crate::app::HorizonApp, events: Vec<egui::Event>| {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        input.events = events;
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    let button = |pressed: bool| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let _ = frame(app, vec![egui::Event::PointerMoved(pos)]);
    let _ = frame(app, vec![button(true)]);
    let _ = frame(app, vec![button(false)]);
}

/// The icon control of the caption of the machine section starts Set up.
#[test]
fn the_control_of_the_machine_caption_presses_set_up() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.setup.integrations = false;
    let ctx = egui::Context::default();
    let position = centre_of(&ctx, &mut app, SET_UP_HOVER);
    press(&ctx, &mut app, position);

    assert!(
        app.setup.notice_panel.is_some(),
        "the press started Set up, which draws its notice without integrations"
    );
}

/// The link of the footer starts Set up as well.
#[test]
fn the_link_of_the_footer_presses_set_up() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.setup.integrations = false;
    let ctx = egui::Context::default();
    let position = centre_of(&ctx, &mut app, footer::SET_UP);
    press(&ctx, &mut app, position);

    assert!(
        app.setup.notice_panel.is_some(),
        "the link of the footer started Set up"
    );
}

/// The footer keeps its place at the bottom of the sidebar, under the scroll area
/// of the groups.
#[test]
fn the_footer_stands_at_the_bottom_of_the_sidebar() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let mut line = None;
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        line = output.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.job.text == footer::NO_SERVER => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        });
    }
    let line = line.expect("the line of the footer");
    let viewport = crate::app::test_support::raw_input([1200.0, 900.0], None)
        .screen_rect
        .expect("the screen of the test")
        .height();
    assert!(
        line.top() > viewport - 60.0,
        "the footer stands in its own band at the bottom: {line:?} of {viewport}"
    );
    let _ = crate::app::SIDEBAR_WIDTH;
}

/// The words of the control of the machine section are the words of the design.
#[test]
fn the_control_of_the_machine_caption_says_what_it_does() {
    assert_eq!(SET_UP_HOVER, "Set up a machine");
    assert_eq!(footer::SET_UP, "Set up");
}
