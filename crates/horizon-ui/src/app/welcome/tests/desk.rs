//! The first start page of a window that shows the orchestrator dock: its form,
//! the places of its parts, and the link that turns the four tiles over.

use egui::Context;

use super::{desk_app, frame_shapes_at, frame_texts_at, press_named, text_shape, tiles_app};
use crate::app::SIDEBAR_WIDTH;
use crate::app::util::usize_to_f32;
use crate::app::welcome::WelcomeTile;
use crate::app::welcome::front_desk::{FEWER, HEADING, LEAD, MORE, STEPS, block_top};
use crate::test_egui::DiscardTextures;
/// The pads of the page on the left and on the right of the block.
const PAGE_PAD: f32 = 60.0;
/// The side of a chip, and its corner radius, as the design names them.
const CHIP: f32 = 22.0;
const CHIP_RADIUS: u8 = 11;
/// The width of a step.
const STEP: f32 = 200.0;

/// The width of a window whose canvas is `canvas` wide beside the sidebar and the
/// dock, which keeps 27 percent of the window between 360 and 540 points.
fn window_for_canvas(canvas: f32) -> f32 {
    let middle = (canvas + SIDEBAR_WIDTH) / 0.73;
    if middle * 0.27 >= 360.0 {
        middle
    } else {
        canvas + SIDEBAR_WIDTH + 360.0
    }
}

/// A first start in a window whose canvas is `canvas` wide beside the dock.
fn desk_window(canvas: f32) -> (tempfile::TempDir, crate::app::HorizonApp) {
    desk_app(window_for_canvas(canvas))
}

/// The rectangles of the chips of the steps that this frame painted.
fn chip_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.corner_radius == egui::CornerRadius::same(CHIP_RADIUS) => {
                if !out.contains(&rect.rect) {
                    out.push(rect.rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The horizontal lines of one point that this frame painted: the lines that join
/// the chips of two steps.
fn join_lines(shapes: &[egui::epaint::ClippedShape]) -> Vec<[egui::Pos2; 2]> {
    fn walk(shape: &egui::Shape, out: &mut Vec<[egui::Pos2; 2]>) {
        match shape {
            egui::Shape::LineSegment { points, stroke }
                if (stroke.width - 1.0).abs() < 0.01 && (points[0].y - points[1].y).abs() < 0.01 =>
            {
                out.push(*points);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The lowest bottom of a text that this frame painted, which is the bottom of
/// the block of the page while the row of shortcuts keeps one row.
fn lowest_text(shapes: &[egui::epaint::ClippedShape]) -> f32 {
    fn walk(shape: &egui::Shape, out: &mut f32) {
        match shape {
            egui::Shape::Text(text) => *out = out.max(text.pos.y + text.galley.size().y),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = 0.0_f32;
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The page of a window with the dock names the start at the front desk, and the
/// four tiles stay behind its link.
#[test]
fn the_page_of_a_window_with_the_dock_sends_the_person_to_the_front_desk() {
    let window = window_for_canvas(1100.0);
    let (_temp, mut app) = desk_app(window);
    let ctx = Context::default();
    let texts = frame_texts_at(&ctx, &mut app, [window, 900.0]);

    assert!(texts.iter().any(|text| text == HEADING), "the heading: {texts:?}");
    assert!(texts.iter().any(|text| text == LEAD), "the line under it: {texts:?}");
    assert!(texts.iter().any(|text| text == MORE), "the link: {texts:?}");
    for step in &STEPS {
        assert!(texts.iter().any(|text| text == step.title), "{}: {texts:?}", step.title);
        assert!(
            texts.iter().any(|text| text == step.sentence),
            "the sentence of {}: {texts:?}",
            step.title
        );
    }
    for tile in WelcomeTile::ALL {
        assert!(
            !texts.iter().any(|text| text == tile.title()),
            "{:?} is not on the page: {texts:?}",
            tile.title()
        );
    }
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::SHORTCUTS_LEAD),
        "the row of shortcuts of this computer: {texts:?}"
    );
}

/// The form of the page follows the dock: a window without the dock, and a window
/// that is too narrow for it, keep the page of four tiles.
#[test]
fn the_form_of_the_page_follows_the_dock() {
    let window = window_for_canvas(1100.0);
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let texts = frame_texts_at(&ctx, &mut app, [window, 900.0]);
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::HEADING),
        "a window without the dock: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == HEADING),
        "no page of the dock: {texts:?}"
    );

    app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::new(true);
    app.orchestrator_dock.set_frame_rect(Some(super::dock_rect_of(window)));
    let texts = frame_texts_at(&ctx, &mut app, [window, 900.0]);
    assert!(
        texts.iter().any(|text| text == HEADING),
        "a window that shows the dock: {texts:?}"
    );

    app.orchestrator_dock.set_frame_rect(None);
    let texts = frame_texts_at(&ctx, &mut app, [window, 900.0]);
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::HEADING),
        "a window too narrow for the dock keeps the four tiles: {texts:?}"
    );

    app.orchestrator_dock.set_frame_rect(Some(super::dock_rect_of(window)));
    app.orchestrator_dock.toggle();
    let texts = frame_texts_at(&ctx, &mut app, [window, 900.0]);
    assert!(
        texts.iter().any(|text| text == crate::app::welcome::HEADING),
        "a switch that hides the dock turns the page back: {texts:?}"
    );
}

/// The heading, the three steps and the link stand centred in the block, the
/// steps are 200 wide with 40 between them, and two lines join their chips.
#[test]
fn the_steps_of_a_wide_canvas_stand_in_one_row_of_three() {
    let window = window_for_canvas(1100.0);
    let (_temp, mut app) = desk_app(window);
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, [window, 900.0]);
    let canvas = app.canvas_rect(&ctx);
    assert!(
        (canvas.width() - 1100.0).abs() < 1.0,
        "the canvas is 1100 wide: {canvas:?}"
    );
    let block_left = canvas.min.x + PAGE_PAD;

    let chips = chip_rects(&shapes);
    assert_eq!(chips.len(), 3, "three chips: {chips:?}");
    for chip in &chips {
        assert!((chip.width() - CHIP).abs() < 0.01, "a chip is {CHIP} wide: {chip:?}");
        assert!((chip.height() - CHIP).abs() < 0.01, "a chip is {CHIP} high: {chip:?}");
    }
    // The group is 3 x 200 + 2 x 40 wide and centred in the block.
    for (index, chip) in chips.iter().enumerate() {
        let expected = block_left + 150.0 + usize_to_f32(index) * (STEP + 40.0);
        assert!(
            (chip.left() - expected).abs() < 0.6,
            "the chip of step {} starts at {expected}, not at {}",
            index + 1,
            chip.left()
        );
    }
    let group = chips[0].left()..chips[2].left() + STEP;
    assert!(
        (f32::midpoint(group.start, group.end) - canvas.center().x).abs() < 0.6,
        "the steps are centred in the canvas: {group:?} against {:?}",
        canvas.center().x
    );

    let lines = join_lines(&shapes);
    assert_eq!(lines.len(), 2, "one line between each two chips: {lines:?}");
    for (index, line) in lines.iter().enumerate() {
        let (from, to) = (chips[index], chips[index + 1]);
        assert!(
            (line[0].y - from.center().y).abs() < 0.6 && (line[1].y - from.center().y).abs() < 0.6,
            "the line stands at the height of the chip centre: {line:?}"
        );
        assert!(
            (line[0].x - (from.right() + 10.0)).abs() < 0.6,
            "the line starts 10 right of the chip: {line:?}"
        );
        assert!(
            (line[1].x - (to.left() - 10.0)).abs() < 0.6,
            "the line ends 10 left of the next chip: {line:?}"
        );
    }

    for (index, step) in STEPS.iter().enumerate() {
        let (title, _) = text_shape(&shapes, step.title).expect("the title of a step is drawn");
        assert!(
            (title.pos.x - chips[index].left()).abs() < 0.6,
            "the title of {:?} stands at the left edge of its step: {}",
            step.title,
            title.pos.x
        );
        let (sentence, _) = text_shape(&shapes, step.sentence).expect("the sentence of a step is drawn");
        assert!(
            (sentence.pos.x - chips[index].left()).abs() < 0.6,
            "the sentence of {:?} stands at the left edge of its step: {}",
            step.title,
            sentence.pos.x
        );
        assert!(
            sentence.galley.size().x <= STEP + 0.6,
            "the sentence of {:?} wraps at {STEP}: {}",
            step.title,
            sentence.galley.size().x
        );
    }

    let (link, _) = text_shape(&shapes, MORE).expect("the link is drawn");
    let band = link.pos.x..link.pos.x + link.galley.size().x + 8.0 + 5.0;
    assert!(
        (f32::midpoint(band.start, band.end) - canvas.center().x).abs() < 0.6,
        "the link stands in the middle of the block: {band:?}"
    );

    let (heading, _) = text_shape(&shapes, HEADING).expect("the heading is drawn");
    assert!(
        (heading.pos.x + heading.galley.size().x * 0.5 - canvas.center().x).abs() < 0.6,
        "the heading is centred"
    );
    assert!(heading.pos.y < chips[0].top(), "the heading stands above the steps");
    assert!(link.pos.y > chips[0].top(), "the link stands under the steps");

    // The middle of the block stands at 46 percent of the height of the canvas.
    let bottom = lowest_text(&shapes);
    let centre = heading.pos.y + (bottom - heading.pos.y) * 0.5 - canvas.min.y;
    assert!(
        (centre - canvas.height() * 0.46).abs() < 1.5,
        "the middle of the block is {centre}, not {}",
        canvas.height() * 0.46
    );
}

/// A canvas narrower than three steps and their air stack the steps in one
/// column at the left edge of the block, with no line between them.
#[test]
fn a_narrow_canvas_stacks_the_steps_in_one_column() {
    let window = window_for_canvas(600.0);
    let (_temp, mut app) = desk_app(window);
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, [window, 900.0]);
    let canvas = app.canvas_rect(&ctx);
    assert!(
        (canvas.width() - 600.0).abs() < 1.0,
        "the canvas is 600 wide: {canvas:?}"
    );
    let block_left = canvas.min.x + PAGE_PAD;

    let chips = chip_rects(&shapes);
    assert_eq!(chips.len(), 3, "three chips: {chips:?}");
    for chip in &chips {
        assert!(
            (chip.left() - block_left).abs() < 0.6,
            "the chips stand in one column at the left edge of the block: {chips:?}"
        );
    }
    assert!(
        chips[0].top() < chips[1].top() && chips[1].top() < chips[2].top(),
        "the steps stack: {chips:?}"
    );
    assert!(join_lines(&shapes).is_empty(), "a column joins no chips");
    for step in &STEPS {
        let (sentence, _) = text_shape(&shapes, step.sentence).expect("the sentence of a step is drawn");
        assert!(
            (sentence.pos.x - block_left).abs() < 0.6,
            "the sentence of {:?} stands at the left edge: {}",
            step.title,
            sentence.pos.x
        );
    }
}

/// The middle of the block stands at 46 percent of the canvas, and a block that
/// is taller than the canvas keeps the top pad of the page.
#[test]
fn the_middle_of_the_block_stands_at_46_percent_of_the_canvas() {
    assert!(
        (block_top(1000.0, 400.0) - 260.0).abs() < 0.01,
        "460 less half the block"
    );
    assert!(
        (block_top(300.0, 400.0) - f32::from(crate::app::welcome::PAGE_PAD_TOP)).abs() < 0.01,
        "a block that does not fit keeps the top pad"
    );
}

/// The link shows the four tiles and hides them again, and it names the state of
/// the run.
#[test]
fn the_link_shows_and_hides_the_four_tiles() {
    let (_temp, mut app) = desk_window(1100.0);
    let mut ctx = Context::default();
    ctx.enable_accesskit();

    assert!(
        press_named(&mut ctx, &mut app, MORE),
        "the link is a control of the page"
    );
    let texts = frame_texts_at(&ctx, &mut app, [window_for_canvas(1100.0), 900.0]);
    assert!(
        texts.iter().any(|text| text == FEWER),
        "the link names the tiles: {texts:?}"
    );
    for tile in WelcomeTile::ALL {
        assert!(
            texts.iter().any(|text| text == tile.title()),
            "{:?} stands under the link: {texts:?}",
            tile.title()
        );
    }

    assert!(
        press_named(&mut ctx, &mut app, FEWER),
        "the link is still a control of the page"
    );
    let texts = frame_texts_at(&ctx, &mut app, [window_for_canvas(1100.0), 900.0]);
    assert!(
        texts.iter().any(|text| text == MORE),
        "the link names the tiles again: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == WelcomeTile::ALL[0].title()),
        "the tiles are hidden again: {texts:?}"
    );
}

/// A tile under the link starts what it starts on the page of four tiles.
#[test]
fn a_tile_under_the_link_starts_what_it_starts_today() {
    let (_temp, mut app) = desk_window(1100.0);
    app.setup.integrations = false;
    let mut ctx = Context::default();
    ctx.enable_accesskit();

    assert!(
        press_named(&mut ctx, &mut app, MORE),
        "the link is a control of the page"
    );
    assert!(
        super::press_tile(&mut ctx, &mut app, WelcomeTile::Machine),
        "the tile of a machine is a control under the link"
    );
    assert_eq!(app.setup.goal, horizon_core::SetupGoal::Rent);
}

/// A frame that changes nothing lays nothing out, and a press on the link lays
/// the page out, with the grid of the four tiles that it shows.
#[test]
fn a_still_page_lays_nothing_out_and_a_press_lays_it_out_once() {
    let (_temp, mut app) = desk_window(1100.0);
    let ctx = Context::default();
    let frame = |app: &mut crate::app::HorizonApp| {
        let _ = ctx
            .run_ui(
                crate::app::test_support::raw_input([window_for_canvas(1100.0), 900.0], None),
                |ui| app.render_welcome_page(ui.ctx()),
            )
            .discard_textures();
    };

    frame(&mut app);
    let first = app.welcome.layouts();
    assert_eq!(first, 1, "the first frame lays the page out once");

    frame(&mut app);
    assert_eq!(app.welcome.layouts(), first, "a still page lays nothing out again");

    app.welcome.toggle_link();
    frame(&mut app);
    let open = app.welcome.layouts();
    assert_eq!(
        open,
        first + 2,
        "the press lays the page out with the grid of the four tiles, which is the second layout"
    );

    frame(&mut app);
    assert_eq!(app.welcome.layouts(), open, "the page under the link stays laid out");
}
