//! The first start page: its rule, its tiles and the press that each starts.

use egui::Context;
use horizon_core::{PanelKind, PanelResume, PresetConfig};

use super::{
    HEADING, LEAD, PAGE_PAD_TOP, SHORTCUTS_LEAD, TILE_INNER_GAP, TILE_PAD, TILE_RADIUS, WelcomeTile, page_top,
    welcome_page_shows,
};
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::test_support::{run_app_frame, test_app};
use crate::test_egui::DiscardTextures;
use crate::typeface::Weight;

mod wheel;

/// A first start in a window whose dock is off, so the page shows its four
/// tiles. A test of the page that the dock shows states the other switch itself.
pub(super) fn tiles_app() -> (tempfile::TempDir, crate::app::HorizonApp) {
    let (temp, mut app) = test_app();
    app.orchestrator_dock = OrchestratorDock::new(false);
    (temp, app)
}

/// A first start in a window `width` points wide whose dock took its column at
/// the right edge, so the page sends the person to the front desk.
///
/// The dock keeps 27 percent of the window, between 360 and 540 points, and a
/// frame reads that rectangle before the page draws.
pub(super) fn desk_app(width: f32) -> (tempfile::TempDir, crate::app::HorizonApp) {
    let (temp, mut app) = test_app();
    app.orchestrator_dock = OrchestratorDock::new(true);
    app.orchestrator_dock.set_frame_rect(Some(dock_rect_of(width)));
    (temp, app)
}

/// The rectangle of the dock at the right edge of a window `width` points wide.
pub(super) fn dock_rect_of(width: f32) -> egui::Rect {
    let dock = (width * 0.27).clamp(360.0, 540.0);
    egui::Rect::from_min_max(egui::pos2(width - dock, 46.0), egui::pos2(width, 900.0))
}

/// The shapes of the last frame of the page, with the rectangle each is
/// clipped to.
pub(super) fn frame_shapes(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<egui::epaint::ClippedShape> {
    frame_shapes_at(ctx, app, [1200.0, 900.0])
}

/// The shapes of the last frame of the page on a window of `size` points.
pub(super) fn frame_shapes_at(
    ctx: &Context,
    app: &mut crate::app::HorizonApp,
    size: [f32; 2],
) -> Vec<egui::epaint::ClippedShape> {
    let _ = frame_texts_at(ctx, app, size);
    let output = ctx.run_ui(crate::app::test_support::raw_input(size, None), |ui| {
        app.render_welcome_page(ui.ctx());
    });
    let output: egui::FullOutput = output.discard_textures();
    output.shapes
}

/// The rectangles of the four tiles that this frame painted.
pub(super) fn tile_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.corner_radius == egui::CornerRadius::same(TILE_RADIUS) => {
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

/// The shapes of one frame of the page of `size`, with `events` fed to it.
pub(super) fn frame_with(
    ctx: &Context,
    app: &mut crate::app::HorizonApp,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> Vec<egui::epaint::ClippedShape> {
    let mut input = crate::app::test_support::raw_input(size, None);
    input.events = events;
    let output = ctx.run_ui(input, |ui| app.render_welcome_page(ui.ctx()));
    let output: egui::FullOutput = output.discard_textures();
    output.shapes
}

/// One press of `key`, as the person makes it.
pub(super) fn key_press(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// A window of one size that shows the whole application, with the clock of its
/// frames.
///
/// Every frame of this window paints the canvas and the page, in the order of a
/// frame of a person, so the canvas takes the input of the frame before the page
/// does. egui refuses a frame whose time is before the time of the frame before
/// it, so the window keeps the clock.
pub(super) struct AppWindow {
    ctx: Context,
    app: crate::app::HorizonApp,
    size: [f32; 2],
    time: f64,
    /// The private home of the window, which its frames write to.
    _temp: tempfile::TempDir,
}

impl AppWindow {
    /// A first start with no workspace and no panel, on the frames that settle it.
    pub(super) fn new(size: [f32; 2]) -> Self {
        Self::with_state(size, |_| {})
    }

    /// The same window, with the state that a test needs before its first frame.
    pub(super) fn with_state(size: [f32; 2], setup: impl FnOnce(&mut crate::app::HorizonApp)) -> Self {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::new(horizon_core::RuntimeState::default()),
            });
        app.theme_applied = true;
        app.initial_pan_done = true;
        // The page of four tiles is the page of a window without the dock.
        app.orchestrator_dock = OrchestratorDock::new(false);
        app.root_viewport_stabilizer = None;
        app.pan_target = None;
        setup(&mut app);
        let mut window = Self {
            ctx,
            app,
            size,
            time: 1.0,
            _temp: temp,
        };
        for _ in 0..3 {
            let _ = window.frame(Vec::new());
        }
        window
    }

    pub(super) fn app(&self) -> &crate::app::HorizonApp {
        &self.app
    }

    pub(super) fn ctx(&self) -> &Context {
        &self.ctx
    }

    /// One frame of the window, with the pointer over the middle of the canvas and
    /// with `events` fed to it, and the shapes that it painted.
    pub(super) fn frame(&mut self, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
        self.time += 0.016;
        let pointer = self.app.canvas_rect(&self.ctx).center();
        let mut input = crate::app::test_support::raw_input(self.size, None);
        input.time = Some(self.time);
        input.events = std::iter::once(egui::Event::PointerMoved(pointer))
            .chain(events)
            .collect();
        let output = crate::app::test_support::run_app_frame_with_input(&self.ctx, &mut self.app, input);
        let output: egui::FullOutput = output.discard_textures();
        output.shapes
    }

    /// Frames until `ready` answers true for the window, or the deadline ends the
    /// test with `what`.
    pub(super) fn frames_until(&mut self, what: &str, mut ready: impl FnMut(&crate::app::HorizonApp) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let _ = self.frame(Vec::new());
            if ready(&self.app) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// The box of the text `text` that this frame painted.
pub(super) fn text_box(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
    text_shape(shapes, text).map(|(shape, _)| egui::Rect::from_min_size(shape.pos, shape.galley.size()))
}

/// The first text that this frame painted whose text starts with `starts`, and
/// the box that it covers.
pub(super) fn painted_text_starting_with<'a>(
    shapes: &'a [egui::epaint::ClippedShape],
    starts: &str,
) -> Option<(&'a egui::epaint::TextShape, egui::Rect)> {
    fn walk<'a>(shape: &'a egui::Shape, starts: &str, out: &mut Option<(&'a egui::epaint::TextShape, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) if out.is_none() && text.galley.text().starts_with(starts) => {
                *out = Some((text, egui::Rect::from_min_size(text.pos, text.galley.size())));
            }
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    walk(shape, starts, out);
                }
            }
            _ => {}
        }
    }
    let mut out = None;
    for clipped in shapes {
        walk(&clipped.shape, starts, &mut out);
        if out.is_some() {
            break;
        }
    }
    out
}

/// The block of the page, from the tiles that this frame painted.
pub(super) fn block_rect(shapes: &[egui::epaint::ClippedShape]) -> egui::Rect {
    tile_rects(shapes)
        .into_iter()
        .reduce(egui::Rect::union)
        .expect("the page paints its tiles")
}

/// One drawing that a frame painted: the band that it stands on, and the box of
/// every shape of it.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaintedDrawing {
    /// The plate of the drawing, as the frame painted it.
    pub band: egui::Rect,
    /// The box of every shape of the drawing, its band included.
    pub bounds: egui::Rect,
}

/// The drawings that this frame painted, in the order of the four tiles.
///
/// A drawing is the group of shapes that `art::art` builds: one shape of the
/// frame holds the band of the drawing and the shapes on it.
pub(super) fn painted_drawings(shapes: &[egui::epaint::ClippedShape]) -> Vec<PaintedDrawing> {
    fn walk(shape: &egui::Shape, out: &mut Vec<PaintedDrawing>) {
        if let egui::Shape::Vec(inner) = shape {
            let band = inner.iter().find_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.corner_radius == egui::CornerRadius::same(super::art::BAND_RADIUS) => {
                    Some(rect.rect)
                }
                _ => None,
            });
            if let Some(band) = band {
                let bounds = inner
                    .iter()
                    .map(egui::Shape::visual_bounding_rect)
                    .filter(|rect| rect.is_finite() && rect.is_positive())
                    .reduce(egui::Rect::union)
                    .unwrap_or(band);
                out.push(PaintedDrawing { band, bounds });
            }
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Every tile keeps the gap of the page between its title, its sentence and its
/// action, also when a title or a sentence takes one row more: the four tiles
/// have one height, which follows the tallest content, and the four presses stand
/// on one line.
#[test]
fn the_four_tiles_keep_their_gaps_and_one_height() {
    for width in [1_240.0, 1_300.0, 1_400.0] {
        let (_temp, mut app) = tiles_app();
        let ctx = Context::default();
        let shapes = frame_shapes_at(&ctx, &mut app, [width, 900.0]);

        let mut action_tops = Vec::new();
        for tile in WelcomeTile::ALL {
            let (title, _) = text_shape(&shapes, tile.title()).expect("the title of a tile is drawn");
            let (sentence, _) = text_shape(&shapes, tile.sentence()).expect("the sentence of a tile is drawn");
            let (action, _) = text_shape(&shapes, tile.action()).expect("the press of a tile is drawn");
            let title_bottom = title.pos.y + title.galley.size().y;
            let sentence_bottom = sentence.pos.y + sentence.galley.size().y;
            assert!(
                sentence.pos.y - title_bottom >= TILE_INNER_GAP - 0.5,
                "the title of {:?} keeps the inner gap above its sentence at {width}: {}",
                tile.title(),
                sentence.pos.y - title_bottom
            );
            assert!(
                action.pos.y - sentence_bottom >= TILE_INNER_GAP - 0.5,
                "the sentence of {:?} ends at {sentence_bottom} and keeps the inner gap {TILE_INNER_GAP} above its \
                 press at {} (a width of {width})",
                tile.title(),
                action.pos.y
            );
            action_tops.push(action.pos.y);
        }

        let rects = tile_rects(&shapes);
        assert_eq!(
            rects.len(),
            WelcomeTile::ALL.len(),
            "the page paints four tiles: {rects:?}"
        );
        let height = rects[0].height();
        for rect in &rects {
            assert!(
                (rect.height() - height).abs() < 0.01,
                "the four tiles have one height at {width}: {rects:?}"
            );
        }
        for top in &action_tops {
            assert!(
                (top - action_tops[0]).abs() < 0.01,
                "the presses of the four tiles stand on one line at {width}: {action_tops:?}"
            );
        }
    }
}

/// Every text that a frame of the window painted.
fn frame_texts(ctx: &Context, app: &mut crate::app::HorizonApp) -> Vec<String> {
    frame_texts_at(ctx, app, [1200.0, 900.0])
}

/// Every text that a frame of the window of `size` painted.
fn frame_texts_at(ctx: &Context, app: &mut crate::app::HorizonApp, size: [f32; 2]) -> Vec<String> {
    let mut texts = Vec::new();
    // A panel shows from its second frame on.
    for _ in 0..2 {
        texts.clear();
        let output = ctx.run_ui(crate::app::test_support::raw_input(size, None), |ui| {
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
pub(super) fn text_shape<'a>(
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
fn the_first_tile_starts_a_panel_of_the_first_agent_that_this_computer_has() {
    let (temp, mut app) = tiles_app();
    let folder = programs_folder(&temp);
    // The program of the second agent preset is a file of this computer, and the
    // program of the first is not.
    install_program(&folder, "claude");
    app.presets = first_start_presets(&folder);
    let mut ctx = Context::default();
    ctx.enable_accesskit();

    let pressed = press_tile(&mut ctx, &mut app, WelcomeTile::Agents);

    assert!(pressed, "the first tile is a control of the page");
    assert_eq!(
        asked_preset(&ctx, &mut app),
        "Claude Code",
        "the first tile starts the agent that this computer has, not the shell that the settings keep first"
    );
}

/// A computer with no agent program keeps the first preset, and one line says
/// why.
#[test]
fn the_first_tile_keeps_the_first_preset_and_says_so_when_no_agent_program_is_here() {
    let (temp, mut app) = tiles_app();
    let folder = programs_folder(&temp);
    app.presets = first_start_presets(&folder);
    let mut ctx = Context::default();
    ctx.enable_accesskit();

    assert!(
        press_tile(&mut ctx, &mut app, WelcomeTile::Agents),
        "the first tile is a control of the page"
    );

    assert_eq!(asked_preset(&ctx, &mut app), "Shell", "no agent program is here");
    let notice = app.sidebar_notice.clone().unwrap_or_default();
    assert!(notice.contains("No agent program was found"), "{notice}");
    assert!(
        notice.contains("codex") && notice.contains("claude"),
        "the line names the programs that Horizon looked for: {notice}"
    );
}

/// Settings with no agent preset give a line with no empty list of programs.
#[test]
fn the_line_of_settings_with_no_agent_preset_names_no_program() {
    let line = super::no_agent_program(&[]);
    assert!(line.contains("No preset in the settings"), "{line}");
    assert!(!line.contains("()"), "{line}");
}

/// The folder of the stand-in agent programs of a test.
fn programs_folder(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let folder = temp.path().join("bin");
    std::fs::create_dir_all(&folder).expect("the folder of the programs");
    folder
}

/// The settings list of a first start: a terminal, and two agent programs whose
/// paths are files of `folder` that the test makes or leaves out.
fn first_start_presets(folder: &std::path::Path) -> Vec<PresetConfig> {
    let preset = |name: &str, kind: PanelKind, program: Option<&str>| PresetConfig {
        name: name.to_owned(),
        alias: None,
        kind,
        command: program.map(|program| folder.join(program).display().to_string()),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    };
    vec![
        preset("Shell", PanelKind::Shell, None),
        preset("Codex", PanelKind::Codex, Some("codex")),
        preset("Claude Code", PanelKind::Claude, Some("claude")),
    ]
}

/// Makes `program` an executable file of `folder`, so the search of the agent
/// programs of this computer finds it.
fn install_program(folder: &std::path::Path, program: &str) {
    horizon_core::stand_in_program_in(folder, program, "exit 0\n").expect("the program of the test");
}

/// The name of the preset that the press asked for, once the search of the agent
/// programs of this computer answered it.
///
/// The search reads the folders of the `PATH` on the coordination worker, so the
/// question opens on a later frame. The bound is the hang guard of the test, not a
/// rule of the product.
fn asked_preset(ctx: &Context, app: &mut crate::app::HorizonApp) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let asked = app
            .dir_picker
            .as_ref()
            .and_then(|picker| picker.purpose())
            .and_then(|purpose| match purpose {
                crate::dir_picker::DirPickerPurpose::AddPanel { preset, .. } => Some(preset.name.clone()),
                crate::dir_picker::DirPickerPurpose::NewWorkspace { .. } => None,
            });
        if let Some(name) = asked {
            return name;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the first tile asked nothing within the bound"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        run_app_frame(ctx, app);
    }
}

#[test]
fn a_tile_of_a_goal_starts_set_up_with_that_goal() {
    let (_temp, mut app) = tiles_app();
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
    let (_temp, mut app) = tiles_app();
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
    press_named(ctx, app, &tile.spoken())
}

/// Presses the control of the page whose accessible name is `label`, as a pointer
/// does, and answers whether the page drew such a control.
pub(super) fn press_named(ctx: &mut Context, app: &mut crate::app::HorizonApp, label: &str) -> bool {
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
                (node.label().or_else(|| node.value()) == Some(label))
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

mod block;
mod desk;
mod drawings;
mod rows;
mod scroll;
