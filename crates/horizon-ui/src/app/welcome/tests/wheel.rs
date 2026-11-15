//! The wheel of a real window: while the first start page shows, a wheel or a
//! touchpad scroll over the canvas moves the page, and the canvas under it
//! neither pans nor zooms.
//!
//! Every assertion here reads a frame of the whole application, so the canvas
//! takes its own scroll before the page does, exactly as it does for a person.

use egui::{Context, Event, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, TouchPhase, Vec2};
use horizon_core::{RuntimeState, StartupDecision};

use super::tile_rects;
use crate::app::HorizonApp;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::test_egui::DiscardTextures;

/// A window that holds no whole page of its size, with a scroll handle.
const SMALL: [f32; 2] = [800.0, 600.0];
/// A window that holds the whole page.
const LARGE: [f32; 2] = [1600.0, 1000.0];

/// A window of one size, with its clock: egui refuses a frame whose time is
/// before the time of the frame before it.
struct Window {
    ctx: Context,
    app: HorizonApp,
    size: [f32; 2],
    time: f64,
    /// Kept for its private home, which the window writes to.
    _temp: tempfile::TempDir,
}

impl Window {
    /// A first start with no workspace and no panel, on the frames that settle it.
    fn new(size: [f32; 2]) -> Self {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.theme_applied = true;
        app.initial_pan_done = true;
        app.root_viewport_stabilizer = None;
        app.pan_target = None;
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

    /// One frame of the window with the pointer over the middle of the page, and
    /// the shapes that it painted.
    fn frame(&mut self, events: Vec<Event>) -> Vec<egui::epaint::ClippedShape> {
        self.time += 0.016;
        let pointer = self.app.canvas_rect(&self.ctx).center();
        let mut input = raw_input(self.size, None);
        input.time = Some(self.time);
        input.events = std::iter::once(Event::PointerMoved(pointer)).chain(events).collect();
        let output = run_app_frame_with_input(&self.ctx, &mut self.app, input);
        let output: egui::FullOutput = output.discard_textures();
        output.shapes
    }

    /// Where the page stands: the top of its first tile, and the pan and the zoom
    /// of the canvas that holds it.
    fn state(&mut self) -> PageState {
        let shapes = self.frame(Vec::new());
        let tiles = tile_rects(&shapes);
        assert_eq!(
            tiles.len(),
            4,
            "the first start page stands in a frame of the whole window of {:?}: {tiles:?}",
            self.size
        );
        PageState {
            top: tiles[0].min.y,
            pan: self.app.canvas_view.pan_offset,
            zoom: self.app.canvas_view.zoom,
        }
    }

    /// One scroll of the window, and the state of the page before it and after it
    /// settled.
    fn scroll(&mut self, unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers) -> (PageState, PageState) {
        let before = self.state();
        let _ = self.frame(vec![wheel(unit, delta, modifiers)]);
        let mut after = before;
        for _ in 0..8 {
            after = self.state();
        }
        (before, after)
    }
}

/// The place of the page in the frames of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageState {
    top: f32,
    pan: [f32; 2],
    zoom: f32,
}

/// One wheel event as the mouse or the touchpad of a person reports it.
fn wheel(unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        source: MouseWheelSource::Wheel,
        unit,
        delta,
        phase: TouchPhase::Move,
        modifiers,
    }
}

/// A wheel moves the page of a small window, and the canvas does not move with
/// it.
#[test]
fn the_wheel_moves_the_page_and_leaves_the_canvas_alone() {
    let mut window = Window::new(SMALL);
    for (unit, delta) in [
        (MouseWheelUnit::Line, Vec2::new(0.0, -3.0)),
        (MouseWheelUnit::Line, Vec2::new(0.0, 3.0)),
        (MouseWheelUnit::Point, Vec2::new(0.0, -120.0)),
        (MouseWheelUnit::Point, Vec2::new(0.0, 120.0)),
    ] {
        let (before, after) = window.scroll(unit, delta, Modifiers::NONE);
        let moved = after.top - before.top;
        assert!(
            if delta.y < 0.0 { moved < -1.0 } else { moved > 1.0 },
            "the wheel {delta:?} in {unit:?} moves the page of a window of {SMALL:?}: it stands at {} and it \
             stood at {}",
            after.top,
            before.top
        );
        assert_eq!(
            (after.pan, after.zoom),
            (before.pan, before.zoom),
            "the canvas keeps its view while the wheel {delta:?} in {unit:?} scrolls the page"
        );
    }
}

/// A page that the window holds has nothing to scroll, and then the wheel moves
/// nothing at all.
#[test]
fn the_wheel_moves_nothing_when_the_page_fits() {
    let mut window = Window::new(LARGE);
    for (unit, delta) in [
        (MouseWheelUnit::Line, Vec2::new(0.0, -3.0)),
        (MouseWheelUnit::Line, Vec2::new(0.0, 3.0)),
        (MouseWheelUnit::Point, Vec2::new(0.0, -120.0)),
    ] {
        let (before, after) = window.scroll(unit, delta, Modifiers::NONE);
        assert_eq!(
            after, before,
            "a page that fits its window does not move for the wheel {delta:?} in {unit:?}"
        );
    }
}

/// A horizontal wheel and a Ctrl wheel have nothing to do on the page: it
/// scrolls down and up only, and it has nothing to zoom.
#[test]
fn a_horizontal_or_a_zoom_wheel_moves_nothing() {
    let mut window = Window::new(SMALL);
    for (delta, modifiers) in [
        (Vec2::new(-120.0, 0.0), Modifiers::NONE),
        (Vec2::new(0.0, -120.0), Modifiers::CTRL),
        (Vec2::new(0.0, -3.0), Modifiers::CTRL),
    ] {
        let (before, after) = window.scroll(MouseWheelUnit::Point, delta, modifiers);
        assert_eq!(
            after, before,
            "the wheel {delta:?} with {modifiers:?} moves nothing on the page"
        );
    }
}

/// The pointer of these tests stands over the page, not over an airy edge: the
/// page covers the middle of the canvas.
#[test]
fn the_middle_of_the_page_is_inside_the_canvas() {
    let mut window = Window::new(SMALL);
    let canvas = window.app.canvas_rect(&window.ctx);
    let shapes = window.frame(Vec::new());
    let point: Pos2 = canvas.center();
    let block = tile_rects(&shapes)
        .into_iter()
        .reduce(egui::Rect::union)
        .expect("the page paints its tiles");
    assert!(
        canvas.contains(point) && block.expand(0.6).contains(point),
        "the pointer at {point:?} points at the page inside {canvas:?}: {block:?}"
    );
}
