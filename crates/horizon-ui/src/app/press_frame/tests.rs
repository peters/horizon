use eframe::App as _;
use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};
use horizon_core::{PanelId, PanelState, RuntimeState, StartupDecision, WorkspaceLayout, WorkspaceState};
use tempfile::TempDir;

use super::PressFrame;
use crate::app::HorizonApp;
use crate::app::test_support::{editor_panel_state, raw_input, run_app_frame_with_input, test_app_with_startup};

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn key(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn motion_after_a_press_waits_for_the_next_frame_and_other_input_stays() {
    let start = Pos2::new(10.0, 10.0);
    let end = Pos2::new(10.0, 60.0);
    let mut split = PressFrame::default();
    let mut events = vec![
        Event::PointerMoved(start),
        button(start, true),
        Event::PointerMoved(end),
        key(Key::A),
        button(end, false),
    ];

    assert!(split.split(&mut events));
    assert_eq!(events, [Event::PointerMoved(start), button(start, true), key(Key::A)]);

    let mut next = vec![key(Key::B)];
    assert!(!split.split(&mut next));
    assert_eq!(next, [Event::PointerMoved(end), button(end, false), key(Key::B)]);
}

#[test]
fn a_press_without_later_motion_keeps_its_frame() {
    let start = Pos2::new(10.0, 10.0);
    let mut split = PressFrame::default();
    for batch in [
        vec![Event::PointerMoved(start), button(start, true)],
        vec![button(start, true), button(start, false)],
        vec![Event::PointerMoved(start), Event::PointerMoved(start + Vec2::Y)],
    ] {
        let mut events = batch.clone();
        assert!(!split.split(&mut events));
        assert_eq!(events, batch);
    }
}

#[test]
fn each_press_in_held_back_input_gets_its_own_frame() {
    let first = Pos2::new(10.0, 10.0);
    let second = Pos2::new(90.0, 10.0);
    let mut split = PressFrame::default();
    let mut events = vec![
        button(first, true),
        Event::PointerMoved(first + Vec2::Y),
        button(first + Vec2::Y, false),
        button(second, true),
        Event::PointerMoved(second + Vec2::Y),
    ];

    assert!(split.split(&mut events));
    assert_eq!(events, [button(first, true)]);
    let mut next = Vec::new();
    assert!(split.split(&mut next));
    assert_eq!(
        next,
        [
            Event::PointerMoved(first + Vec2::Y),
            button(first + Vec2::Y, false),
            button(second, true)
        ]
    );
    let mut last = Vec::new();
    assert!(!split.split(&mut last));
    assert_eq!(last, [Event::PointerMoved(second + Vec2::Y)]);
}

struct Fixture {
    _temp: TempDir,
    ctx: Context,
    app: HorizonApp,
    time: f64,
    /// False gives the app each batch as it came, to show what the split prevents.
    split: bool,
}

impl Fixture {
    fn new(panels: Vec<PanelState>, layout: Option<WorkspaceLayout>, zoom: f32) -> Self {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![WorkspaceState {
                    local_id: "synthetic".into(),
                    name: "synthetic".into(),
                    position: Some([40.0, 80.0]),
                    panels,
                    ..WorkspaceState::default()
                }],
                ..RuntimeState::default()
            }),
        });
        app.theme_applied = true;
        app.initial_pan_done = true;
        app.root_viewport_stabilizer = None;
        app.canvas_view.zoom = zoom;
        if let Some(layout) = layout {
            let workspace = app.board.workspaces[0].id;
            app.board.arrange_workspace(workspace, layout);
        }
        let mut fixture = Self {
            _temp: temp,
            ctx,
            app,
            time: 0.0,
            split: true,
        };
        for _ in 0..3 {
            fixture.frame(Vec::new());
        }
        fixture.app.pan_target = None;
        fixture
    }

    /// Runs one frame the way eframe does: input passes the app's raw input hook first.
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(self.time);
        input.events = events;
        self.app.raw_input_hook(&self.ctx, &mut input);
        if !self.split {
            // Only pointer input was held back, so appending it restores the batch order.
            input.events.append(&mut self.app.press_frame.deferred);
        }
        let _ = run_app_frame_with_input(&self.ctx, &mut self.app, input);
    }

    fn panels(&self) -> Vec<(PanelId, Rect)> {
        self.app
            .visible_panel_geometry_for_canvas_view(self.app.canvas_rect(&self.ctx), None)
            .into_iter()
            .map(|(id, geometry)| (id, geometry.screen_rect))
            .collect()
    }

    fn screen_rect(&self, panel: PanelId) -> Rect {
        self.panels()
            .into_iter()
            .find_map(|(id, rect)| (id == panel).then_some(rect))
            .expect("panel on screen")
    }

    /// A drag from `start` in 20 motion steps. With `Delivery::OneBatch` the press and every
    /// step arrive in one batch, as when a window system delivers them before a slow frame.
    /// With `Delivery::FramePerEvent` the press and each step get their own frame.
    fn drag(&mut self, start: Pos2, movement: Vec2, delivery: Delivery) {
        let steps = (1..=20u8).map(|step| Event::PointerMoved(start + movement * (f32::from(step) / 20.0)));
        match delivery {
            Delivery::OneBatch => {
                let mut batch = vec![Event::PointerMoved(start), button(start, true)];
                batch.extend(steps);
                self.frame(batch);
            }
            Delivery::FramePerEvent => {
                self.frame(vec![Event::PointerMoved(start)]);
                self.frame(vec![button(start, true)]);
                for step in steps {
                    self.frame(vec![step]);
                }
            }
        }
        self.frame(Vec::new());
        self.frame(vec![button(start + movement, false)]);
        self.frame(Vec::new());
    }
}

#[derive(Clone, Copy, Debug)]
enum Delivery {
    OneBatch,
    FramePerEvent,
}

const DELIVERIES: [Delivery; 2] = [Delivery::OneBatch, Delivery::FramePerEvent];

impl Delivery {
    /// How far the panel may stop short of the drag. egui starts a drag only when the pointer
    /// has moved past its click distance, so with a frame per event the first of the 20 steps
    /// does not move the panel. In one batch that step is in the frame where the drag starts.
    fn allowed_shortfall(self, movement: Vec2) -> f32 {
        match self {
            Self::OneBatch => 1.0,
            Self::FramePerEvent => movement.length() / 20.0 + 1.0,
        }
    }
}

fn panel(local_id: &str) -> PanelState {
    editor_panel_state(local_id, [60.0, 140.0])
}

/// A titlebar point: `along` is the share of the titlebar's width from its left end.
fn titlebar_point(rect: Rect, zoom: f32, along: f32) -> Pos2 {
    Pos2::new(
        rect.min.x + rect.width() * along,
        rect.min.y + super::super::PANEL_TITLEBAR_HEIGHT * zoom * 0.5,
    )
}

#[test]
fn a_titlebar_drag_in_one_batch_moves_the_panel_in_every_direction_region_and_zoom() {
    titlebar_drags_move_the_panel(Delivery::OneBatch);
}

#[test]
fn a_titlebar_drag_with_a_frame_per_event_moves_the_panel_in_every_direction_region_and_zoom() {
    titlebar_drags_move_the_panel(Delivery::FramePerEvent);
}

fn titlebar_drags_move_the_panel(delivery: Delivery) {
    // The title text starts at the left; the right part of the titlebar is blank.
    for (region, along) in [("title", 0.2), ("blank", 0.6)] {
        for zoom in [0.5, 1.0, 1.5] {
            for focused in [true, false] {
                for movement in [Vec2::new(0.0, 90.0), Vec2::new(0.0, -70.0), Vec2::new(120.0, 0.0)] {
                    let mut fixture = Fixture::new(vec![panel("only")], None, zoom);
                    let (id, before) = fixture.panels()[0];
                    fixture.app.board.focused = focused.then_some(id);
                    fixture.frame(Vec::new());
                    fixture.drag(titlebar_point(before, zoom, along), movement, delivery);
                    let moved = fixture.screen_rect(id).min - before.min;
                    assert!(
                        (moved - movement).length() < delivery.allowed_shortfall(movement),
                        "{delivery:?} {region} drag at zoom {zoom}, focused {focused}, by {movement:?} moved {moved:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_vertical_titlebar_drag_swaps_arranged_rows_in_one_batch_or_a_frame_per_event() {
    for delivery in DELIVERIES {
        for zoom in [0.5, 1.0] {
            let mut fixture = Fixture::new(vec![panel("upper"), panel("lower")], Some(WorkspaceLayout::Rows), zoom);
            let mut rows = fixture.panels();
            rows.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
            let [(upper, upper_rect), (lower, lower_rect)] = rows[..] else {
                panic!("two arranged rows: {rows:?}");
            };
            fixture.drag(
                titlebar_point(upper_rect, zoom, 0.6),
                lower_rect.min - upper_rect.min,
                delivery,
            );
            let workspace = &fixture.app.board.workspaces[0];
            assert_eq!(
                workspace.layout,
                Some(WorkspaceLayout::Rows),
                "{delivery:?} zoom {zoom}"
            );
            assert_eq!(
                workspace.panels,
                [lower, upper],
                "{delivery:?}: rows did not swap at zoom {zoom}"
            );
        }
    }
}

#[test]
fn without_the_split_a_one_batch_vertical_drag_is_lost_and_a_frame_per_event_is_not() {
    // The control for the tests above: the one-batch delivery reproduces the lost drag.
    for zoom in [0.5, 1.0] {
        for (delivery, moves) in [(Delivery::OneBatch, false), (Delivery::FramePerEvent, true)] {
            let mut fixture = Fixture::new(vec![panel("only")], None, zoom);
            fixture.split = false;
            let (id, before) = fixture.panels()[0];
            let movement = Vec2::new(0.0, 90.0);
            fixture.drag(titlebar_point(before, zoom, 0.6), movement, delivery);
            let moved = fixture.screen_rect(id).min - before.min;
            assert_eq!(
                (moved - movement).length() < delivery.allowed_shortfall(movement),
                moves,
                "{delivery:?} drag at zoom {zoom} without the split moved {moved:?}"
            );
        }
    }
}
