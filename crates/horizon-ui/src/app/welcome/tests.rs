//! The first start page: its rule, its tiles and the press that each starts.

use egui::Context;

use super::{HEADING, LEAD, SHORTCUTS_LEAD, WelcomeTile, welcome_page_shows};
use crate::app::test_support::{run_app_frame, test_app};
use crate::test_egui::DiscardTextures;

/// Every text that a frame of the window painted.
fn frame_texts(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    let mut texts = Vec::new();
    // A panel shows from its second frame on.
    for _ in 0..2 {
        texts.clear();
        let output = ctx.run_ui(crate::app::test_support::raw_input([1200.0, 900.0], None), |ui| {
            app.render_welcome_page(ui.ctx());
        });
        let output: egui::FullOutput = output.discard_textures();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        if !texts.is_empty() {
            break;
        }
    }
    texts
}

fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_texts(shape, texts);
            }
        }
        _ => {}
    }
}

#[test]
fn the_page_shows_only_on_an_empty_board() {
    assert!(welcome_page_shows(0, 0, true));
    assert!(!welcome_page_shows(1, 0, true), "a workspace ends the first start");
    assert!(!welcome_page_shows(1, 1, true));
    assert!(!welcome_page_shows(0, 0, false), "a dialog covers the page");
}

#[test]
fn the_page_names_the_heading_the_line_and_the_four_tiles() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();

    let texts = frame_texts(&ctx, &mut app);

    assert!(texts.iter().any(|text| text == HEADING), "{texts:?}");
    assert!(texts.iter().any(|text| text == LEAD), "{texts:?}");
    assert!(texts.iter().any(|text| text == SHORTCUTS_LEAD), "{texts:?}");
    for tile in WelcomeTile::ALL {
        assert!(
            texts.iter().any(|text| text == tile.title()),
            "{:?} is drawn: {texts:?}",
            tile.title()
        );
        assert!(
            texts.iter().any(|text| text == tile.sentence()),
            "the sentence of {:?} is drawn: {texts:?}",
            tile.title()
        );
        assert!(
            texts.iter().any(|text| text == tile.action()),
            "the press of {:?} is drawn: {texts:?}",
            tile.title()
        );
    }
}

#[test]
fn the_row_of_shortcuts_names_the_keys_of_this_computer() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    let texts = frame_texts(&ctx, &mut app);

    let primary = crate::app::util::primary_shortcut_label();
    for keys in [
        app.shortcuts.new_terminal.display_label(primary),
        app.shortcuts.command_palette.display_label(primary),
        app.shortcuts.open_remote_hosts.display_label(primary),
    ] {
        assert!(
            texts.iter().any(|text| text == &keys),
            "the key {keys} of this computer is drawn: {texts:?}"
        );
    }
    assert!(
        texts.iter().any(|text| text == primary || text.contains(primary)),
        "the primary key of this system is named: {texts:?}"
    );
}

#[test]
fn the_page_goes_away_as_soon_as_the_board_holds_a_panel() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    assert!(!frame_texts(&ctx, &mut app).is_empty(), "the page shows");

    let _ = app.board.create_workspace("Home");
    let texts = frame_texts(&ctx, &mut app);

    assert!(
        texts.is_empty(),
        "a board with a workspace shows no first page: {texts:?}"
    );
}

#[test]
fn the_first_tile_starts_a_panel_of_the_first_preset() {
    let (_temp, mut app) = test_app();
    let mut ctx = Context::default();
    ctx.enable_accesskit();
    let pressed = press_tile(&mut ctx, &mut app, WelcomeTile::Agents);

    assert!(pressed, "the first tile is a control of the page");
    assert!(
        !app.board.panels.is_empty() || app.dir_picker.is_some(),
        "the first tile starts the panel of the person, or asks where to run it"
    );
}

#[test]
fn a_tile_of_a_goal_starts_set_up_with_that_goal() {
    let (_temp, mut app) = test_app();
    app.setup.integrations = false;
    let mut ctx = Context::default();
    ctx.enable_accesskit();
    let pressed = press_tile(&mut ctx, &mut app, WelcomeTile::Machine);

    assert!(pressed, "the tile of a machine is a control of the page");
    assert_eq!(app.setup.goal, horizon_core::SetupGoal::Rent);
    assert!(app.setup.notice_panel.is_some(), "Set up ran for the goal of the tile");
}

/// Presses the tile of `tile` on the page, as a pointer does, and answers
/// whether the page drew a press.
fn press_tile(ctx: &mut Context, app: &mut crate::app::HorizonApp, tile: WelcomeTile) -> bool {
    let frame = |app: &mut crate::app::HorizonApp, events: Vec<egui::Event>| {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        input.events = events;
        ctx.run_ui(input, |ui| app.render_welcome_page(ui.ctx()))
            .discard_textures()
    };
    let mut position = None;
    for _ in 0..2 {
        let output = frame(app, Vec::new());
        if let Some(update) = output.platform_output.accesskit_update {
            position = update.nodes.iter().find_map(|(_, node)| {
                (node.label().or_else(|| node.value()) == Some(tile.spoken().as_str()))
                    .then(|| node.bounds())
                    .flatten()
            });
        }
    }
    let Some(bounds) = position else {
        return false;
    };
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
    let _ = frame(app, vec![egui::Event::PointerMoved(position)]);
    let _ = frame(app, vec![button(true)]);
    let _ = frame(app, vec![button(false)]);
    run_app_frame(ctx, app);
    true
}
