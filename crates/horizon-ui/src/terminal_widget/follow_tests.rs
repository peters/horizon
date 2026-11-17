use std::time::{Duration, Instant};

use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};
use horizon_core::{Board, FollowFeed, FollowSpec, Panel, PanelId, PanelKind};

use super::{PanelNoticeAction, TerminalKeyboardContext, TerminalSelectionDragState, TerminalView, grid_metrics};
use crate::primary_selection::PrimarySelection;
use crate::test_egui::DiscardTextures;

/// The grid of the stream of every test here: 30 columns and 6 rows.
const GRID: (u16, u16) = (30, 6);

/// A follow panel of a board, and the frames that draw it.
struct Follower {
    ctx: Context,
    board: Board,
    id: PanelId,
    feed: FollowFeed,
    primary_selection: PrimarySelection,
    clipboard_images: crate::clipboard_image::ClipboardImages,
    selection_drag: TerminalSelectionDragState,
}

impl Follower {
    fn new() -> Self {
        let mut board = Board::new();
        let workspace = board.create_workspace("shared");
        let id = board
            .create_follow_panel(
                workspace,
                FollowSpec {
                    device: "device-1".to_owned(),
                    label: "other computer".to_owned(),
                    panel: "panel-1".to_owned(),
                    title: "build".to_owned(),
                    kind: PanelKind::Shell,
                    cols: GRID.0,
                    rows: GRID.1,
                    typing: true,
                },
            )
            .expect("a follow panel opens");
        let feed = board.panel(id).and_then(Panel::follow_feed).expect("its feed");
        Self {
            ctx: Context::default(),
            board,
            id,
            feed,
            primary_selection: PrimarySelection::new(),
            clipboard_images: crate::clipboard_image::ClipboardImages::new(),
            selection_drag: TerminalSelectionDragState::default(),
        }
    }

    /// Gives the stream `output` and waits until the screen shows `text`.
    fn show(&mut self, output: &[u8], text: &str) {
        self.feed.output(output);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.board.process_output();
            let terminal = self.board.panel(self.id).and_then(Panel::terminal).expect("terminal");
            if terminal.last_lines_text(usize::from(GRID.1)).contains(text) {
                return;
            }
            assert!(Instant::now() < deadline, "the screen never showed {text}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Columns and rows of the terminal of the panel.
    fn grid(&self) -> (u16, u16) {
        let terminal = self.board.panel(self.id).and_then(Panel::terminal).expect("terminal");
        (terminal.cols(), terminal.rows())
    }

    /// One frame that draws the panel with `available` as its size, and its
    /// terminal from the top left of `available`.
    fn frame(&mut self, available: Rect, events: Vec<Event>) -> egui::FullOutput {
        self.board.process_output();
        let panel = self.board.panel_mut(self.id).expect("the follow panel");
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            events,
            ..RawInput::default()
        };
        self.ctx
            .run_ui(input, |ui| {
                let place = egui::UiBuilder::new()
                    .max_rect(available)
                    .layout(egui::Layout::top_down(egui::Align::Min));
                ui.scope_builder(place, |ui| {
                    TerminalView::new(panel, None, 0.0).show(
                        ui,
                        true,
                        true,
                        &mut self.selection_drag,
                        TerminalKeyboardContext {
                            keyboard_events: &[],
                            primary_selection: &self.primary_selection,
                            clipboard_images: &self.clipboard_images,
                            local_ssh_reconnect_enabled: false,
                            notice_action: &mut PanelNoticeAction::default(),
                            take_keyboard_focus: false,
                        },
                    );
                });
            })
            .discard_textures()
    }
}

/// The clip rectangles of the drawn texts that contain `text`.
fn clips_of_text(output: &egui::FullOutput, text: &str) -> Vec<Rect> {
    output
        .shapes
        .iter()
        .filter(|shape| matches!(&shape.shape, Shape::Text(drawn) if drawn.galley.text().contains(text)))
        .map(|shape| shape.clip_rect)
        .collect()
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A panel body that is smaller or larger than the grid of the stream changes
/// nothing of that grid: the widget draws it from the top left of the body,
/// clips what does not fit and never asks the follower to resize.
#[test]
fn a_frame_draws_the_grid_of_the_stream_and_never_resizes_it() {
    let mut follower = Follower::new();
    follower.show(b"first row\r\nsecond row", "second row");

    for size in [Vec2::new(120.0, 40.0), Vec2::new(900.0, 600.0)] {
        let available = Rect::from_min_size(Pos2::new(40.0, 30.0), size);
        let output = follower.frame(available, Vec::new());

        assert_eq!(follower.grid(), GRID, "a body of {size:?} keeps the grid of the stream");
        let clips = clips_of_text(&output, "first row");
        assert!(!clips.is_empty(), "the first row of the grid is drawn in {size:?}");
        for clip in clips {
            assert!(available.contains_rect(clip), "{clip:?} leaves the body {available:?}");
            assert_eq!(clip.min, available.min, "the grid starts at the top left of the body");
        }
    }
    assert!(follower.feed.take_typed().is_empty(), "a frame types nothing");
}

/// The pointer maps to the cell of the grid of the stream at the normal cell
/// size, in a body that is far larger than that grid.
#[test]
fn a_press_reports_the_cell_of_the_grid_of_the_stream() {
    let mut follower = Follower::new();
    follower.show(b"\x1b[?1000h\x1b[?1006hready", "ready");
    let available = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(900.0, 600.0));
    follower.frame(available, Vec::new());
    let metrics = grid_metrics(&follower.ctx);
    // The middle of the cell in column 4 and row 3, counted from one.
    let cell = available.min + Vec2::new(3.5 * metrics.char_width, 2.5 * metrics.line_height);

    follower.frame(available, vec![Event::PointerMoved(cell)]);
    follower.frame(available, vec![press(cell, true)]);
    follower.frame(available, vec![press(cell, false)]);

    let typed = String::from_utf8(follower.feed.take_typed()).expect("mouse reports are text");
    assert_eq!(typed, "\x1b[<0;4;3M\x1b[<0;4;3m");

    // The body is larger than the grid. A press in the empty part of it reports
    // the nearest cell that the stream has, and never a cell beyond its grid.
    let beyond = available.min + Vec2::new(50.5 * metrics.char_width, 10.5 * metrics.line_height);
    follower.frame(available, vec![Event::PointerMoved(beyond)]);
    follower.frame(available, vec![press(beyond, true)]);
    follower.frame(available, vec![press(beyond, false)]);
    let typed = String::from_utf8(follower.feed.take_typed()).expect("mouse reports are text");
    assert_eq!(typed, "\x1b[<0;30;6M\x1b[<0;30;6m");
    assert_eq!(follower.grid(), GRID);
}

#[test]
fn a_stream_that_ended_says_so_under_the_grid() {
    let mut follower = Follower::new();
    follower.show(b"last screen", "last screen");
    let available = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(900.0, 600.0));
    let line = "The stream of this terminal ended: the server is gone.";
    assert!(clips_of_text(&follower.frame(available, Vec::new()), line).is_empty());

    follower.feed.ended("the server is gone");
    let output = follower.frame(available, Vec::new());

    assert_eq!(
        clips_of_text(&output, line).len(),
        1,
        "one line says that the stream ended"
    );
    assert!(
        !clips_of_text(&output, "last screen").is_empty(),
        "the last screen stays"
    );
}
