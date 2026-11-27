//! Frames of a whole application: the canvas that ends at the dock, the minimap
//! that stays on the canvas, the parts of the dock from the header to its bottom
//! edge, and the keyboard that the dock leaves alone.

use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::{RuntimeState, StartupDecision, WorkspaceState};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::composer::{FIELD_HINT, FIELD_ID};
use crate::app::orchestrator_dock::header::{NO_WORKSPACE_LINE, PROGRAM_LINE};
use crate::app::orchestrator_dock::{LastFrame, OrchestratorDock};
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

/// The window of the dock-on tasks: wide enough that a 540 point dock and the
/// minimap both fit beside the canvas.
const WIDE: [f32; 2] = [2000.0, 1000.0];
/// The window whose canvas and minimap the dock-off task compares with the
/// numbers of the base branch.
const BASE: [f32; 2] = [1600.0, 1000.0];

/// The window of one size, with the clock of its frames.
pub(super) struct Window {
    ctx: egui::Context,
    app: HorizonApp,
    size: [f32; 2],
    time: f64,
    _temp: tempfile::TempDir,
}

/// One text that a frame painted.
#[derive(Clone, Debug)]
pub(super) struct Drawn {
    pub(super) text: String,
    pub(super) rect: Rect,
    pub(super) color: Option<egui::Color32>,
}

impl Window {
    /// A window of `size` points on a board of `workspaces`, with the switch of
    /// the orchestrator dock as `dock_on` names it, after the frames that settle
    /// its view.
    pub(super) fn new(size: [f32; 2], workspaces: Vec<WorkspaceState>, dock_on: bool) -> Self {
        Self::build(size, workspaces, OrchestratorDock::new(dock_on))
    }

    /// The same window, whose orchestrator runs the program of `program`: the
    /// seam of `HORIZON_ORCHESTRATOR_PROGRAM`, without the environment of the
    /// test process.
    pub(super) fn with_program(size: [f32; 2], workspaces: Vec<WorkspaceState>, program: &std::path::Path) -> Self {
        Self::build(
            size,
            workspaces,
            OrchestratorDock::with_program(true, program.to_path_buf()),
        )
    }

    fn build(size: [f32; 2], workspaces: Vec<WorkspaceState>, dock: OrchestratorDock) -> Self {
        let (temp, ctx, app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces,
                ..RuntimeState::default()
            }),
        });
        let mut window = Self {
            ctx,
            app,
            size,
            time: 1.0,
            _temp: temp,
        };
        window.app.theme_applied = true;
        window.app.initial_pan_done = true;
        window.app.startup_workspace_organization_pending = false;
        window.app.root_viewport_stabilizer = None;
        window.app.orchestrator_dock = dock;
        for _ in 0..3 {
            let _ = window.frame(Vec::new());
        }
        window
    }

    /// Runs frames until `wanted` holds for the app, and names what the test
    /// waited for when the bound ends first.
    pub(super) fn wait_for(&mut self, what: &str, wanted: impl Fn(&HorizonApp) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !wanted(&self.app) {
            assert!(std::time::Instant::now() < deadline, "the test waited for {what}");
            let _ = self.frame(Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The app of this window, which a test reads and acts on.
    pub(super) fn app(&mut self) -> &mut HorizonApp {
        &mut self.app
    }

    /// The context of this window, whose memory a test reads for the focus.
    pub(super) fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub(super) fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.016;
        let mut input = raw_input(self.size, None);
        input.time = Some(self.time);
        input.events = events;
        run_app_frame_with_input(&self.ctx, &mut self.app, input)
    }

    /// Makes the workspace that `local_id` names the active one.
    pub(super) fn focus_workspace(&mut self, local_id: &str) {
        let found = self
            .app
            .board
            .workspaces
            .iter()
            .find(|workspace| workspace.local_id == local_id)
            .map(|workspace| workspace.id);
        let Some(workspace) = found else {
            panic!("the board holds the workspace {local_id}");
        };
        self.app.board.focus_workspace(workspace);
    }

    /// Writes `text` into the draft of the workspace that `local_id` names, as a
    /// person who typed in the field would.
    pub(super) fn note_draft(&mut self, local_id: &str, text: &str) {
        self.app.orchestrator_dock.draft_mut(local_id).push_str(text);
    }
}

/// Every text that `output` painted.
pub(super) fn texts(output: &egui::FullOutput) -> Vec<Drawn> {
    fn walk(shape: &egui::Shape, out: &mut Vec<Drawn>) {
        match shape {
            egui::Shape::Text(text) => out.push(Drawn {
                text: text.galley.text().to_owned(),
                rect: Rect::from_min_size(text.pos, text.galley.size()),
                color: text.galley.job.sections.first().map(|section| section.format.color),
            }),
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// The rectangle shapes of `output` at `rect`: the fill of the control and, when
/// it has one, its outline.
fn control_shapes(output: &egui::FullOutput, rect: Rect) -> Vec<egui::epaint::RectShape> {
    fn walk(shape: &egui::Shape, rect: Rect, out: &mut Vec<egui::epaint::RectShape>) {
        match shape {
            egui::Shape::Rect(shape) if (shape.rect.min - rect.min).length() <= 0.5 => out.push(shape.clone()),
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, rect, out)),
            _ => (),
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, rect, &mut found);
    }
    assert!(!found.is_empty(), "the control draws its rectangle {rect:?}");
    found
}

/// The colour of the glyph `text` inside `rect` of `output`.
fn glyph_color(output: &egui::FullOutput, rect: Rect, text: &str) -> egui::Color32 {
    texts(output)
        .into_iter()
        .find(|drawn| drawn.text == text && rect.contains_rect(drawn.rect))
        .and_then(|drawn| drawn.color)
        .unwrap_or_else(|| panic!("the control draws the glyph {text:?} in {rect:?}"))
}

/// The rectangle of the dock of the last frame of `window`.
fn dock_of(window: &Window) -> Rect {
    window
        .app
        .orchestrator_dock
        .frame_rect()
        .unwrap_or_else(|| panic!("the dock of a window with the switch on"))
}

/// The rectangles of the parts of the dock in the last frame of `window`.
fn parts_of(window: &Window) -> LastFrame {
    window
        .app
        .orchestrator_dock
        .last_frame()
        .unwrap_or_else(|| panic!("the dock keeps the rectangles of its last frame"))
}

/// The one text of `drawn` that reads `text`.
fn drawn_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

/// A frame of the wide window, the active workspace `dock-work` and `draft` in
/// its field.
fn frame_with_draft(draft: &str) -> (Window, egui::FullOutput) {
    let mut window = Window::new(WIDE, vec![editor_workspace_state("dock-work", [40.0, 40.0])], true);
    window.focus_workspace("dock-work");
    window.note_draft("dock-work", draft);
    let output = window.frame(Vec::new());
    (window, output)
}

/// The rectangles of the shapes of `output` that the field draws: the fill of the
/// field and the outline that the field edit puts around it.
fn field_frames(output: &egui::FullOutput) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Rect(rect) => {
                if rect.fill == crate::theme::BG() && rect.corner_radius == egui::CornerRadius::same(12) {
                    out.push(rect.rect);
                }
            }
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

#[test]
fn the_canvas_ends_at_the_dock_and_the_minimap_stays_on_the_canvas() {
    let mut window = Window::new(WIDE, vec![editor_workspace_state("dock-work", [40.0, 40.0])], true);
    window.focus_workspace("dock-work");
    let output = window.frame(Vec::new());

    let dock = window
        .app
        .orchestrator_dock
        .frame_rect()
        .expect("the dock of a window with the switch on");
    // 27 percent of 2000 points is above the widest dock, so the dock takes 540.
    assert_eq!(
        dock,
        Rect::from_min_max(Pos2::new(1460.0, TOOLBAR_HEIGHT), Pos2::new(2000.0, 1000.0))
    );

    let canvas = window.app.canvas_rect(&window.ctx);
    assert_eq!(canvas.min, Pos2::new(SIDEBAR_WIDTH, TOOLBAR_HEIGHT));
    assert_eq!(
        canvas.max.x, dock.min.x,
        "the canvas {canvas:?} ends at the dock {dock:?}"
    );

    let map = window
        .app
        .minimap_overlay_rect(&window.ctx)
        .expect("the minimap of a wide window");
    assert!(
        canvas.contains_rect(map),
        "the minimap {map:?} stands on the canvas {canvas:?}"
    );

    let drawn = texts(&output);
    let name = drawn
        .iter()
        .find(|text| text.text == "dock-work" && dock.contains_rect(text.rect))
        .unwrap_or_else(|| panic!("the header draws the workspace name in the dock {dock:?}: {drawn:?}"));
    assert!(dock.contains_rect(name.rect));
    let state = drawn
        .iter()
        .find(|text| text.text == PROGRAM_LINE)
        .unwrap_or_else(|| panic!("the header draws {PROGRAM_LINE:?}: {drawn:?}"));
    assert!(
        dock.contains_rect(state.rect),
        "the state line {:?} of the dock {dock:?}",
        state.rect
    );
}

#[test]
fn the_canvas_and_the_minimap_keep_their_numbers_with_the_dock_off() {
    let mut window = Window::new(BASE, vec![editor_workspace_state("dock-work", [40.0, 40.0])], false);
    window.focus_workspace("dock-work");
    let output = window.frame(Vec::new());

    assert_eq!(window.app.orchestrator_dock.frame_rect(), None);
    // The numbers of the base branch, for a window of 1600 by 1000 points.
    let canvas = window.app.canvas_rect(&window.ctx);
    assert_eq!(
        canvas,
        Rect::from_min_max(Pos2::new(210.0, 46.0), Pos2::new(1600.0, 1000.0))
    );
    let map = window
        .app
        .minimap_overlay_rect(&window.ctx)
        .expect("the minimap of the base window");
    assert_eq!(
        map,
        Rect::from_min_max(Pos2::new(1252.0, 792.0), Pos2::new(1584.0, 984.0))
    );

    let drawn = texts(&output);
    for text in [PROGRAM_LINE, FIELD_HINT, "Say what you want done.", NO_WORKSPACE_LINE] {
        assert!(
            !drawn.iter().any(|drawn| drawn.text == text),
            "a dock that is off draws no {text:?}"
        );
    }
}

#[test]
fn the_dock_of_a_narrow_window_leaves_the_canvas_whole() {
    let mut window = Window::new(
        [800.0, 600.0],
        vec![editor_workspace_state("dock-work", [40.0, 40.0])],
        true,
    );
    window.focus_workspace("dock-work");
    let output = window.frame(Vec::new());

    // The sidebar and a dock of 360 points leave 230 points of canvas, so the
    // dock does not draw and the canvas keeps the window.
    assert_eq!(window.app.orchestrator_dock.frame_rect(), None);
    let canvas = window.app.canvas_rect(&window.ctx);
    assert_eq!(
        canvas,
        Rect::from_min_max(Pos2::new(210.0, 46.0), Pos2::new(800.0, 600.0))
    );
    assert!(!texts(&output).iter().any(|drawn| drawn.text == PROGRAM_LINE));
}

#[test]
fn the_field_takes_the_keyboard_only_after_a_click() {
    let mut window = Window::new(WIDE, vec![editor_workspace_state("dock-work", [40.0, 40.0])], true);
    window.focus_workspace("dock-work");
    let output = window.frame(Vec::new());
    let field = egui::Id::unique(FIELD_ID);

    // The dock came up and holds no widget of the window, so the panel that had
    // the keys keeps them: an active terminal panel reads them while no other
    // widget has the focus.
    assert!(!window.ctx.memory(|memory| memory.has_focus(field)));
    assert_eq!(
        window.ctx.memory(egui::Memory::focused),
        None,
        "the dock takes no widget of the window when it appears"
    );

    let hint = texts(&output)
        .into_iter()
        .find(|text| text.text == FIELD_HINT)
        .unwrap_or_else(|| panic!("the field draws its hint {FIELD_HINT:?}"));
    let at = hint.rect.center();
    let _ = window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    let _ = window.frame(vec![button(at, false)]);

    assert!(
        window.ctx.memory(|memory| memory.has_focus(field)),
        "a click in the field gives it the keyboard"
    );
}

#[test]
fn the_text_of_the_field_keeps_the_margin_of_the_field() {
    let (window, output) = frame_with_draft("");
    let parts = parts_of(&window);
    let drawn = texts(&output);

    // The hint starts 14 points from the left edge of the field and 12 under its
    // top edge, so it never touches the corner of the field.
    let hint = drawn_text(&drawn, FIELD_HINT);
    assert!(
        hint.rect.min.x - parts.field.min.x >= 13.0,
        "the hint {:?} starts at the left margin of the field {:?}",
        hint.rect,
        parts.field
    );
    assert!(
        hint.rect.min.y - parts.field.min.y >= 11.0,
        "the hint {:?} starts at the top margin of the field {:?}",
        hint.rect,
        parts.field
    );
    // The field draws its frame on the rectangle that the dock worked out.
    let frames = field_frames(&output);
    assert_eq!(frames.len(), 1, "the field draws one frame: {frames:?}");
    assert!(
        (frames[0].min - parts.field.min).length() <= 1.0 && (frames[0].max - parts.field.max).length() <= 1.0,
        "the frame {:?} of the field {:?}",
        frames[0],
        parts.field
    );
}

#[test]
fn the_field_stands_at_the_bottom_of_the_dock_under_the_empty_state() {
    let (window, output) = frame_with_draft("");
    let dock = dock_of(&window);
    let parts = parts_of(&window);
    let drawn = texts(&output);

    // The field hangs on the bottom edge of the dock: its text lies in the lowest
    // 150 points and the composer keeps the room of its bottom padding under it.
    let hint = drawn_text(&drawn, FIELD_HINT);
    assert!(
        dock.contains_rect(parts.field),
        "the field {:?} stands in the dock {dock:?}",
        parts.field
    );
    assert!(
        hint.rect.center().y > dock.max.y - 150.0,
        "the text of the field {:?} lies in the lowest 150 points of the dock {dock:?}",
        hint.rect
    );
    assert!(parts.field.max.y < dock.max.y);

    // The empty state stands in the body, over the field and under the header.
    let title = drawn_text(&drawn, "Say what you want done.");
    let body = drawn
        .iter()
        .find(|text| text.text.starts_with("The orchestrator reads the repository"))
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws the sentences of the empty state: {drawn:?}"));
    assert!(dock.contains_rect(title.rect), "the title {:?} of the dock", title.rect);
    assert!(
        dock.contains_rect(body.rect),
        "the sentences {:?} of the dock",
        body.rect
    );
    assert!(
        parts.body.contains_rect(title.rect),
        "the title {:?} of the body {:?}",
        title.rect,
        parts.body
    );
    assert!(title.rect.min.y > parts.header.max.y);
    assert!(body.rect.min.y > title.rect.max.y);
    assert!(body.rect.max.y < parts.field.min.y);
    // The body is all the room between the header and the composer, and the
    // composer hangs on the bottom edge of the dock.
    assert_eq!(parts.body.min.y, parts.header.max.y);
    assert_eq!(parts.body.max.y, parts.composer.min.y);
    assert!(parts.composer.max.y <= dock.max.y);
}

#[test]
fn the_chips_stand_under_the_field_inside_the_dock() {
    let (window, output) = frame_with_draft("");
    let dock = dock_of(&window);
    let parts = parts_of(&window);
    let drawn = texts(&output);

    for text in [
        "Orchestrator",
        "Claude Code",
        "Workers",
        "not chosen",
        "Place",
        "this computer",
    ] {
        let chip = drawn_text(&drawn, text);
        assert!(
            parts.chips.contains_rect(chip.rect),
            "the text {text:?} {:?} stands in the room of the chips {:?}",
            chip.rect,
            parts.chips
        );
        assert!(
            dock.contains_rect(chip.rect),
            "the text {text:?} {:?} stands in the dock {dock:?}",
            chip.rect
        );
        assert!(
            chip.rect.min.y > parts.field.max.y,
            "the text {text:?} {:?} stands under the field {:?}",
            chip.rect,
            parts.field
        );
        assert!(
            chip.rect.max.y < dock.max.y,
            "the text {text:?} {:?} stands over the bottom edge of the dock {dock:?}",
            chip.rect
        );
    }
}

#[test]
fn the_send_control_stands_inside_the_field_at_its_right_end() {
    let (window, output) = frame_with_draft("");
    let dock = dock_of(&window);
    let parts = parts_of(&window);
    let drawn = texts(&output);

    assert!(dock.contains_rect(parts.send), "the send control {:?}", parts.send);
    // The control stands inside the field, 7 points from its right inner edge.
    assert!(
        parts.field.contains_rect(parts.send),
        "the send control {:?} stands inside the field {:?}",
        parts.send,
        parts.field
    );
    assert!((parts.field.max.x - parts.send.max.x - 7.0).abs() <= 0.5);
    // The control is 34 by 34 and shares the centre line of a field of one row.
    assert!((parts.send.width() - 34.0).abs() <= 0.5 && (parts.send.height() - 34.0).abs() <= 0.5);
    assert!((parts.send.center().y - parts.field.center().y).abs() <= 0.5);
    assert!(
        (parts.field.height() - 48.0).abs() <= 0.5,
        "a field of one row is 48 high"
    );
    // The field takes the full width of the composer, which keeps 16 points of
    // room at each side of its content.
    assert!((parts.field.min.x - parts.composer.min.x - 16.0).abs() <= 0.5);
    assert!((parts.composer.max.x - parts.field.max.x - 16.0).abs() <= 0.5);
    // The panel of the dock keeps its own line inside that room.
    assert!(parts.field.min.x - dock.min.x >= 16.0 && parts.field.min.x - dock.min.x <= 19.0);
    assert!(dock.max.x - parts.field.max.x >= 16.0 && dock.max.x - parts.field.max.x <= 19.0);
    let glyph = drawn
        .iter()
        .find(|text| text.text == "\u{2191}" && parts.send.contains_rect(text.rect))
        .cloned()
        .unwrap_or_else(|| panic!("the send control {:?} draws its glyph: {drawn:?}", parts.send));
    assert!(parts.send.contains_rect(glyph.rect));
    // A taller draft keeps the control at the bottom right of the field.
    let (tall_window, _) = frame_with_draft("one\ntwo\nthree");
    let tall = parts_of(&tall_window);
    assert!(tall.send.min.y > tall.field.min.y);
    assert!((tall.field.max.y - tall.send.max.y - 7.0).abs() <= 0.5);
}

#[test]
fn the_control_shows_what_a_press_will_do() {
    // (a) A draft that can go out: the accent fill and the arrow in the
    // background colour. A pointer on it blends it toward the foreground.
    let (mut window, output) = frame_with_draft("hello");
    let parts = parts_of(&window);
    let shapes = control_shapes(&output, parts.send);
    assert_eq!(shapes.len(), 1, "the accent form draws its fill and no outline");
    assert_eq!(shapes[0].fill, crate::theme::ACCENT());
    assert_eq!(glyph_color(&output, parts.send, "\u{2191}"), crate::theme::BG());
    let _ = window.frame(vec![Event::PointerMoved(parts.send.center())]);
    let hovered = control_shapes(&window.frame(Vec::new()), parts.send);
    assert_eq!(
        hovered[0].fill,
        crate::theme::blend(crate::theme::ACCENT(), crate::theme::FG(), 0.12)
    );

    // (b) An empty draft, and a window with no workspace: the quiet form.
    let mut quiet = vec![frame_with_draft("").0, Window::new(WIDE, Vec::new(), true)];
    for mut window in quiet.drain(..) {
        let output = window.frame(Vec::new());
        let parts = parts_of(&window);
        let shapes = control_shapes(&output, parts.send);
        assert_eq!(shapes[0].fill, crate::theme::PANEL_BG_ALT());
        assert_eq!(shapes[1].stroke.color, crate::theme::BORDER_SUBTLE());
        assert_eq!(
            glyph_color(&output, parts.send, "\u{2191}"),
            crate::theme::FG_DIM(),
            "a draft that cannot go out keeps the dim arrow"
        );
    }

    // (c) A busy conversation: the stop form, a filled square in the foreground.
    let (mut window, _) = frame_with_draft("");
    window
        .app()
        .orchestrator_dock
        .session_mut("dock-work")
        .say("a turn that runs");
    let output = window.frame(Vec::new());
    let parts = parts_of(&window);
    let shapes = control_shapes(&output, parts.send);
    assert_eq!(shapes[0].fill, crate::theme::PANEL_BG_ALT());
    assert_eq!(shapes[1].stroke.color, crate::theme::BORDER_STRONG());
    assert_eq!(
        glyph_color(&output, parts.send, "\u{25a0}"),
        crate::theme::FG(),
        "the stop form draws the filled square in the foreground"
    );
}

#[test]
fn the_two_example_rows_stand_between_the_sentences_and_the_composer() {
    let (window, output) = frame_with_draft("");
    let dock = dock_of(&window);
    let parts = parts_of(&window);
    let drawn = texts(&output);

    let title = drawn_text(&drawn, "Say what you want done.");
    let body = drawn
        .iter()
        .find(|text| text.text.starts_with("The orchestrator reads the repository"))
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws the sentences of the empty state: {drawn:?}"));
    let first = drawn_text(&drawn, "Fix #1987 on a rented machine");
    let second = drawn_text(&drawn, "Why is the cold start slow? Measure it first.");

    // The title, the sentences and the two rows follow each other, 8 and 12
    // points apart, inside the room between the header and the composer.
    assert!(title.rect.max.y <= body.rect.min.y);
    assert!(body.rect.min.y - title.rect.max.y <= 9.0);
    assert!(body.rect.max.y < first.rect.min.y);
    assert!(parts.body.contains_rect(first.rect) && parts.body.contains_rect(second.rect));
    assert!(first.rect.max.y < second.rect.min.y);
    assert!(second.rect.max.y < parts.composer.min.y);
    assert!(dock.contains_rect(second.rect));
    // Both rows begin at the left edge of the empty state block, and they take
    // its full width.
    assert!((first.rect.min.x - second.rect.min.x).abs() <= 0.5);
    assert!(first.rect.min.x - dock.min.x >= 27.0);
}

#[test]
fn a_click_on_an_example_fills_an_empty_draft_and_sends_nothing() {
    let mut window = Window::new(WIDE, vec![editor_workspace_state("dock-work", [40.0, 40.0])], true);
    window.focus_workspace("dock-work");
    let output = window.frame(Vec::new());
    let example = drawn_text(&texts(&output), "Fix #1987 on a rented machine");
    let at = example.rect.center();
    let _ = window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    let _ = window.frame(vec![button(at, false)]);

    assert_eq!(
        window.app().orchestrator_dock.draft("dock-work"),
        "Fix #1987 on a rented machine",
        "the click puts the text of the example into the empty draft"
    );
    assert!(
        window
            .ctx()
            .memory(|memory| memory.has_focus(egui::Id::unique(FIELD_ID))),
        "the field takes the keyboard after the click"
    );
    assert!(
        window.app().orchestrator_dock.session("dock-work").is_none(),
        "the click sends nothing"
    );
}

#[test]
fn a_click_on_an_example_keeps_the_draft_of_the_person() {
    let (mut window, output) = frame_with_draft("mine");
    let example = drawn_text(&texts(&output), "Why is the cold start slow? Measure it first.");
    let at = example.rect.center();
    let _ = window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    let _ = window.frame(vec![button(at, false)]);

    assert_eq!(
        window.app().orchestrator_dock.draft("dock-work"),
        "mine",
        "a draft that holds text stays as the person wrote it"
    );
    assert!(
        !window
            .ctx()
            .memory(|memory| memory.has_focus(egui::Id::unique(FIELD_ID)))
    );
}

#[test]
fn the_header_names_the_state_in_a_chip_at_its_right_edge() {
    let (window, output) = frame_with_draft("");
    let dock = dock_of(&window);
    let drawn = texts(&output);

    let chip = drawn_text(&drawn, "not connected");
    let line = drawn_text(&drawn, PROGRAM_LINE);
    assert!(
        dock.contains_rect(chip.rect),
        "the chip {:?} of the dock {dock:?}",
        chip.rect
    );
    assert!(
        chip.rect.max.x > dock.max.x - 40.0,
        "the chip {:?} stands at the right edge of the dock {dock:?}",
        chip.rect
    );
    assert!(
        !drawn.iter().any(|text| text.text.contains("\u{b7} not connected")),
        "the state word left the second line"
    );
    assert!(!line.text.contains("not connected"));
    assert!(line.rect.max.x < chip.rect.min.x);
}

#[test]
fn the_chip_stays_in_a_dock_of_the_least_width() {
    let mut window = Window::new(
        [1200.0, 900.0],
        vec![editor_workspace_state(
            "a name that is far too long for the header",
            [40.0, 40.0],
        )],
        true,
    );
    window.focus_workspace("a name that is far too long for the header");
    let output = window.frame(Vec::new());
    let dock = dock_of(&window);
    assert!((dock.width() - 360.0).abs() <= 0.5, "the least dock is 360 wide");
    let drawn = texts(&output);
    let chip = drawn_text(&drawn, "not connected");
    let name = drawn
        .iter()
        .find(|text| dock.contains_rect(text.rect) && text.text.starts_with("a name that is"))
        .cloned()
        .unwrap_or_else(|| panic!("the header draws the name: {drawn:?}"));
    assert!(name.rect.max.x <= chip.rect.min.x - 7.0);
    assert!(chip.rect.max.x <= dock.max.x - 15.0);
    assert!(dock.contains_rect(chip.rect));
}

#[test]
fn a_draft_of_eight_lines_leaves_the_field_at_six_rows() {
    let (window, output) = frame_with_draft("one\ntwo\nthree\nfour\nfive\nsix\nseven\neight");
    let dock = dock_of(&window);
    let parts = parts_of(&window);
    let (five_window, _) = frame_with_draft("one\ntwo\nthree\nfour\nfive");
    let (six_window, _) = frame_with_draft("one\ntwo\nthree\nfour\nfive\nsix");
    let five_rows = parts_of(&five_window).field.height();
    let six_rows = parts_of(&six_window).field.height();
    let eight_rows = parts.field.height();

    // The field grows with the rows of the draft, and it stops after six of them.
    assert!(five_rows < six_rows, "five rows {five_rows} and six rows {six_rows}");
    assert!(
        (six_rows - eight_rows).abs() <= 0.5,
        "six rows {six_rows} and eight rows {eight_rows}"
    );
    // The room that the field takes goes upward, so the header and the chips keep
    // their place and the dock keeps every part.
    assert!(parts.field.min.y > parts.header.max.y);
    assert!(dock.contains_rect(parts.chips));
    assert!(dock.contains_rect(parts.field));
    // The field keeps the rectangle of the dock and clips the rows that it has no
    // room for: it does not grow into the room of the chips.
    let frames = field_frames(&output);
    assert_eq!(frames.len(), 1, "the field draws one frame: {frames:?}");
    assert!(
        (frames[0].height() - parts.field.height()).abs() <= 1.0,
        "the frame {:?} of the field {:?}",
        frames[0],
        parts.field
    );
}

#[test]
fn the_minimap_needs_three_times_its_width_of_canvas() {
    // The map is 332 by 192 points, so the rule of the minimap asks for a canvas
    // of at least 996 by 384 points.
    let mut roomy = Window::new(
        [1206.0, 1000.0],
        vec![editor_workspace_state("dock-work", [40.0, 40.0])],
        false,
    );
    roomy.focus_workspace("dock-work");
    let _ = roomy.frame(Vec::new());
    let map = roomy
        .app
        .minimap_overlay_rect(&roomy.ctx)
        .expect("the minimap of a canvas of 996 points");
    assert_eq!(map.size(), egui::Vec2::new(332.0, 192.0));
    assert_eq!(roomy.app.canvas_rect(&roomy.ctx).width(), 996.0);

    // The dock takes 399.6 of the 1480 points of this window, so its canvas is
    // 870.4 points wide. The same canvas width beside no dock is the canvas of a
    // window of 1080 points, and the rule hides the map on it too: the dock does
    // not hide the map, the narrower canvas does.
    let mut base = Window::new(
        [1080.0, 900.0],
        vec![editor_workspace_state("dock-work", [40.0, 40.0])],
        false,
    );
    base.focus_workspace("dock-work");
    let _ = base.frame(Vec::new());
    let base_canvas = base.app.canvas_rect(&base.ctx);
    assert_eq!(base_canvas.width(), 870.0);
    assert_eq!(base.app.minimap_overlay_rect(&base.ctx), None);

    let mut docked = Window::new(
        [1480.0, 900.0],
        vec![editor_workspace_state("dock-work", [40.0, 40.0])],
        true,
    );
    docked.focus_workspace("dock-work");
    let _ = docked.frame(Vec::new());
    let docked_canvas = docked.app.canvas_rect(&docked.ctx);
    assert!(
        (docked_canvas.width() - 870.4).abs() < 0.1,
        "the canvas beside the dock is {} wide",
        docked_canvas.width()
    );
    assert_eq!(docked.app.minimap_overlay_rect(&docked.ctx), None);
}
