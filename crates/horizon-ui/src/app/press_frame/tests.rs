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

    /// A quick drag from `start`: the press and every motion step arrive in one batch,
    /// as when a window system delivers them before a slow frame.
    fn quick_drag(&mut self, start: Pos2, movement: Vec2) {
        let mut batch = vec![Event::PointerMoved(start), button(start, true)];
        batch.extend((1..=20u8).map(|step| Event::PointerMoved(start + movement * (f32::from(step) / 20.0))));
        self.frame(batch);
        self.frame(Vec::new());
        self.frame(vec![button(start + movement, false)]);
        self.frame(Vec::new());
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
fn a_quick_titlebar_drag_moves_the_panel_in_every_direction_region_and_zoom() {
    // The title text starts at the left; the right part of the titlebar is blank.
    for (region, along) in [("title", 0.2), ("blank", 0.6)] {
        for zoom in [0.5, 1.0, 1.5] {
            for focused in [true, false] {
                for movement in [Vec2::new(0.0, 90.0), Vec2::new(0.0, -70.0), Vec2::new(120.0, 0.0)] {
                    let mut fixture = Fixture::new(vec![panel("only")], None, zoom);
                    let (id, before) = fixture.panels()[0];
                    fixture.app.board.focused = focused.then_some(id);
                    fixture.frame(Vec::new());
                    fixture.quick_drag(titlebar_point(before, zoom, along), movement);
                    let moved = fixture.screen_rect(id).min - before.min;
                    assert!(
                        (moved - movement).length() < 1.0,
                        "{region} drag at zoom {zoom}, focused {focused}, by {movement:?} moved {moved:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_quick_vertical_titlebar_drag_swaps_arranged_rows() {
    for zoom in [0.5, 1.0] {
        let mut fixture = Fixture::new(vec![panel("upper"), panel("lower")], Some(WorkspaceLayout::Rows), zoom);
        let mut rows = fixture.panels();
        rows.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
        let [(upper, upper_rect), (lower, lower_rect)] = rows[..] else {
            panic!("two arranged rows: {rows:?}");
        };
        fixture.quick_drag(titlebar_point(upper_rect, zoom, 0.6), lower_rect.min - upper_rect.min);
        let workspace = &fixture.app.board.workspaces[0];
        assert_eq!(workspace.layout, Some(WorkspaceLayout::Rows), "zoom {zoom}");
        assert_eq!(workspace.panels, [lower, upper], "rows did not swap at zoom {zoom}");
    }
}
