//! The first start page: its rule, its tiles and the press that each starts.

use egui::Context;

use super::{HEADING, LEAD, PAGE_PAD_TOP, SHORTCUTS_LEAD, WelcomeTile, page_top, welcome_page_shows};
use crate::app::test_support::{run_app_frame, test_app};
use crate::test_egui::DiscardTextures;
use crate::typeface::Weight;

/// The shapes of the last frame of the page, with the rectangle each is
/// clipped to.
fn frame_shapes(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<egui::epaint::ClippedShape> {
    let _ = frame_texts(ctx, app);
    let output = ctx.run_ui(crate::app::test_support::raw_input([1200.0, 900.0], None), |ui| {
        app.render_welcome_page(ui.ctx());
    });
    let output: egui::FullOutput = output.discard_textures();
    output.shapes
}

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

fn walk_text<'a>(shape: &'a egui::Shape, text: &str, out: &mut Option<&'a egui::epaint::TextShape>) {
    match shape {
        egui::Shape::Text(shape) if shape.galley.text() == text => *out = Some(shape),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                walk_text(shape, text, out);
            }
        }
        _ => {}
    }
}

/// The text shape that painted `text`, with the rectangle it was clipped to.
fn text_shape<'a>(
    shapes: &'a [egui::epaint::ClippedShape],
    text: &str,
) -> Option<(&'a egui::epaint::TextShape, egui::Rect)> {
    shapes.iter().find_map(|clipped| {
        let mut shape = None;
        walk_text(&clipped.shape, text, &mut shape);
        shape.map(|shape| (shape, clipped.clip_rect))
    })
}

#[test]
fn the_block_of_the_page_is_centred_in_a_canvas_that_holds_it() {
    // The height of the block of the live window, in a canvas that is taller.
    let block = 470.0;
    let centred = page_top(900.0, block);
    assert!(
        (centred - (900.0 - block) * 0.5).abs() < 0.5,
        "the block stands in the middle of the canvas: {centred}"
    );
    assert!(centred > f32::from(PAGE_PAD_TOP), "the middle is below the top pad");

    let top = page_top(block + f32::from(PAGE_PAD_TOP) + 1.0, block);
    assert!(
        (top - f32::from(PAGE_PAD_TOP)).abs() < 0.5,
        "a canvas that does not hold the block keeps it at the top: {top}"
    );
}

#[test]
fn the_line_under_the_heading_is_centred_row_by_row() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    let shapes = frame_shapes(&ctx, &mut app);
    let (text, _) = text_shape(&shapes, LEAD).expect("the line under the heading is drawn");
    assert!(
        text.galley.rows.len() >= 2,
        "the line wraps at this width: {:?}",
        text.galley.rows.len()
    );
    let centre = app.canvas_rect(&ctx).center().x;
    for placed in &text.galley.rows {
        let row_centre = text.pos.x + placed.pos.x + placed.row.size.x * 0.5;
        assert!(
            (row_centre - centre).abs() < 3.0,
            "every row of the line is centred like the mockup: {row_centre} against {centre}"
        );
    }
}

/// The title of a tile starts at the left edge of the tile, where the press of
/// that tile starts. A title with centred rows that is painted at that edge
/// hangs half outside its tile, and the first title goes under the sidebar.
#[test]
fn the_title_of_every_tile_starts_at_the_left_edge_of_its_tile() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    let shapes = frame_shapes(&ctx, &mut app);
    for tile in WelcomeTile::ALL {
        let (title, _) = text_shape(&shapes, tile.title()).expect("the title of a tile is drawn");
        let (press, _) = text_shape(&shapes, tile.action()).expect("the press of a tile is drawn");
        for placed in &title.galley.rows {
            let row_left = title.pos.x + placed.pos.x;
            assert!(
                (row_left - press.pos.x).abs() < 0.6,
                "a row of the title {:?} starts at {row_left}, and its tile starts its text at {}",
                tile.title(),
                press.pos.x
            );
        }
    }
}

#[test]
fn the_shortcut_line_is_drawn_whole_inside_the_canvas() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    let shapes = frame_shapes(&ctx, &mut app);
    let (text, clip) = text_shape(&shapes, SHORTCUTS_LEAD).expect("the shortcut line is drawn");
    let bottom = text.pos.y + text.galley.size().y;
    assert!(
        clip.max.y + 0.5 >= bottom,
        "the shortcut line is not cut by the page: {clip:?} against {bottom}"
    );
    assert!(
        clip.max.y <= 900.0 + 0.5,
        "the shortcut line stands inside the canvas: {clip:?}"
    );
}

#[test]
fn the_page_shows_only_on_an_empty_board() {
    assert!(welcome_page_shows(0, 0, true));
    assert!(!welcome_page_shows(1, 0, true), "a workspace ends the first start");
    assert!(!welcome_page_shows(1, 1, true));
    assert!(!welcome_page_shows(0, 0, false), "a dialog covers the page");
}

/// The weight that a text was laid out with, as the value of its `wght` axis.
fn weight_of(shape: &egui::epaint::TextShape) -> Option<f32> {
    shape
        .galley
        .job
        .sections
        .first()
        .and_then(|section| section.format.coords.as_ref().first().map(|(_, value)| *value))
}

/// A text of the page is painted once, in the weight of the mockup.
fn painted_weights(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<Option<f32>> {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Vec<Option<f32>>) {
        match shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => out.push(weight_of(shape)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, text, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, text, &mut out);
    }
    out
}

#[test]
fn the_heading_is_painted_once_in_its_own_weight() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    let shapes = frame_shapes(&ctx, &mut app);

    let heading = painted_weights(&shapes, HEADING);
    assert_eq!(heading.len(), 1, "one heading, not a second pass of it: {heading:?}");
    assert_eq!(heading[0], Some(Weight::Black.wght()));

    let title = painted_weights(&shapes, WelcomeTile::ALL[0].title());
    assert_eq!(title, vec![Some(Weight::Bold.wght())]);
    let press = painted_weights(&shapes, WelcomeTile::ALL[0].action());
    assert_eq!(press, vec![Some(Weight::Semibold.wght())]);
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

/// The keyboard reaches the tiles: Tab puts the focus ring on one, and Space
/// starts it.
#[test]
fn the_keyboard_starts_a_tile_that_it_reached() {
    let (_temp, mut app) = test_app();
    app.setup.integrations = false;
    let ctx = Context::default();
    let frame = |app: &mut crate::app::HorizonApp, events: Vec<egui::Event>| {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        input.events = events;
        ctx.run_ui(input, |ui| app.render_welcome_page(ui.ctx()))
            .discard_textures()
    };
    let _ = frame(&mut app, Vec::new());
    let _ = frame(&mut app, Vec::new());
    let key = |key: egui::Key, pressed: bool| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // Tab walks the four tiles; the second one is the machine.
    for _ in 0..2 {
        let _ = frame(&mut app, vec![key(egui::Key::Tab, true)]);
    }
    let _ = frame(&mut app, vec![key(egui::Key::Space, true)]);
    let _ = frame(&mut app, vec![key(egui::Key::Space, false)]);

    assert_eq!(
        app.setup.goal,
        horizon_core::SetupGoal::Rent,
        "Space started the second tile, which Set up receives as the goal rent"
    );
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
