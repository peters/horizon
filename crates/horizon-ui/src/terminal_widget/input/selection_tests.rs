use super::frame_events::{pointer_button_event_any_pos, pointer_button_event_pos};
use super::selection_drag::AutoScrollCadence;
use super::{PointerSupport, TerminalSelectionDragState, handle_pointer_selection_drag, handle_terminal_pointer_input};
use crate::primary_selection::PrimarySelection;
use crate::terminal_widget::layout::{GridMetrics, terminal_interaction, terminal_layout};
use egui::{
    Context, Event, FontId, Id, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, Vec2, ViewportId,
};
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};
use std::fmt::Write as _;
use std::time::Duration;

const VISIBLE_ROWS: u16 = 8;
const VISIBLE_COLS: u16 = 25;
const CHAR_WIDTH: f32 = 8.0;
const LINE_HEIGHT: f32 = 16.0;
const TEST_PANEL_SIZE: Vec2 = Vec2::new(220.0, LINE_HEIGHT * VISIBLE_ROWS as f32);

struct PointerHarness {
    ctx: Context,
    panel: Panel,
    primary_selection: PrimarySelection,
    selection_drag: TerminalSelectionDragState,
    _transcript_root: tempfile::TempDir,
    frame_index: u32,
    repaint_delay: Duration,
}

impl PointerHarness {
    fn new(initial_scrollback: usize) -> Self {
        let transcript_root = tempfile::tempdir().expect("transcript tempdir");
        let mut replay = String::new();
        for index in 0..120 {
            write!(replay, "HZNSEL-{index:03}\r\n").expect("format selection marker");
        }
        std::fs::write(transcript_root.path().join("selection-panel.bin"), replay).expect("write selection transcript");
        let mut panel = Panel::spawn(
            PanelId(42),
            WorkspaceId(7),
            PanelOptions {
                name: Some("Selection test".to_string()),
                kind: PanelKind::Ssh,
                rows: VISIBLE_ROWS,
                cols: VISIBLE_COLS,
                local_id: Some("selection-panel".to_string()),
                transcript_root: Some(transcript_root.path().to_path_buf()),
                restore_as_disconnected_snapshot: true,
                ..PanelOptions::default()
            },
        )
        .expect("spawn disconnected selection snapshot");
        panel.set_scrollback(initial_scrollback);
        assert_eq!(
            panel.terminal().expect("terminal").scrollback(),
            initial_scrollback,
            "fixture needs enough scrollback history"
        );

        Self {
            ctx: Context::default(),
            panel,
            primary_selection: PrimarySelection::new(),
            selection_drag: TerminalSelectionDragState::default(),
            _transcript_root: transcript_root,
            frame_index: 0,
            repaint_delay: Duration::MAX,
        }
    }

    fn frame(&mut self, events: Vec<Event>) -> Rect {
        let frame_time = f64::from(self.frame_index) / 60.0;
        self.frame_index += 1;
        self.frame_at(frame_time, events)
    }

    fn frame_at(&mut self, frame_time: f64, events: Vec<Event>) -> Rect {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 320.0))),
            time: Some(frame_time),
            predicted_dt: 0.0,
            events,
            ..RawInput::default()
        };
        input.viewport_id = ViewportId::ROOT;
        self.ctx.begin_pass(input);

        let mut body_rect = Rect::NOTHING;
        egui::Area::new(Id::new("terminal_selection_wheel_test"))
            .fixed_pos(Pos2::new(60.0, 80.0))
            .movable(false)
            .show(&self.ctx, |ui| {
                let metrics = grid_metrics();
                let layout = terminal_layout(TEST_PANEL_SIZE, CHAR_WIDTH, LINE_HEIGHT);
                let interaction = terminal_interaction(ui, layout, self.panel.id.0, true);
                body_rect = interaction.layout.body;
                handle_terminal_pointer_input(
                    ui,
                    &mut self.panel,
                    &interaction,
                    true,
                    PointerSupport {
                        metrics: &metrics,
                        visible_rows: VISIBLE_ROWS,
                        visible_cols: VISIBLE_COLS,
                        primary_selection: &self.primary_selection,
                        selection_drag: &mut self.selection_drag,
                    },
                );
            });
        let output = self.ctx.end_pass();
        self.repaint_delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .expect("root viewport output")
            .repaint_delay;
        body_rect
    }

    fn scrollback(&self) -> usize {
        self.panel.terminal().expect("terminal").scrollback()
    }

    fn repaint_delay(&self) -> Duration {
        self.repaint_delay
    }

    fn viewport_line(&self, row: usize) -> String {
        let terminal = self.panel.terminal().expect("terminal");
        let (lines, total_lines) = terminal.full_text_lines(usize::MAX);
        let viewport_top = total_lines
            .saturating_sub(usize::from(terminal.rows()))
            .saturating_sub(terminal.scrollback());
        lines
            .get(viewport_top + row)
            .unwrap_or_else(|| panic!("missing viewport row {row} at scrollback {}", terminal.scrollback()))
            .clone()
    }

    fn selected_first_line(&self) -> String {
        self.panel
            .terminal()
            .expect("terminal")
            .selection_to_string()
            .expect("selection")
            .lines()
            .next()
            .expect("first selected line")
            .to_string()
    }

    fn selected_last_line(&self) -> String {
        self.panel
            .terminal()
            .expect("terminal")
            .selection_to_string()
            .expect("selection")
            .lines()
            .next_back()
            .expect("last selected line")
            .to_string()
    }

    fn selection_drag_at(&mut self, position: Pos2, body_rect: Rect) -> bool {
        handle_pointer_selection_drag(
            &mut self.panel,
            position,
            body_rect,
            &grid_metrics(),
            VISIBLE_ROWS,
            VISIBLE_COLS,
        )
    }
}

fn grid_metrics() -> GridMetrics {
    GridMetrics {
        char_width: CHAR_WIDTH,
        line_height: LINE_HEIGHT,
        font_id: FontId::monospace(13.0),
    }
}

fn cell_position(body_rect: Rect, row: u16, column: u16, horizontal_fraction: f32) -> Pos2 {
    Pos2::new(
        body_rect.min.x + (f32::from(column) + horizontal_fraction) * CHAR_WIDTH,
        body_rect.min.y + (f32::from(row) + 0.5) * LINE_HEIGHT,
    )
}

fn primary_press(position: Pos2) -> Vec<Event> {
    primary_press_with_modifiers(position, Modifiers::NONE)
}

fn primary_press_with_modifiers(position: Pos2, modifiers: Modifiers) -> Vec<Event> {
    vec![
        Event::PointerMoved(position),
        Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]
}

fn primary_release(position: Pos2) -> Vec<Event> {
    vec![Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]
}

fn wheel(vertical_lines: f32) -> Vec<Event> {
    vec![Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, vertical_lines),
        modifiers: Modifiers::NONE,
    }]
}

#[test]
fn pointer_button_event_any_pos_keeps_release_outside_rect() {
    let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(20.0, 20.0));
    let events = vec![Event::PointerButton {
        pos: Pos2::new(42.0, 6.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }];

    assert_eq!(
        pointer_button_event_pos(&events, None, PointerButton::Primary, false, rect),
        None
    );
    assert_eq!(
        pointer_button_event_any_pos(&events, None, PointerButton::Primary, false),
        Some(Pos2::new(42.0, 6.0))
    );
}

#[test]
fn selection_drag_state_ignores_finish_for_other_panels() {
    let panel_id = PanelId(42);
    let other_panel_id = PanelId(7);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));

    assert!(state.active_for(panel_id));
    assert!(!state.active_for(other_panel_id));
    assert!(!state.finish(other_panel_id));
    assert!(state.active_for(panel_id));
}

#[test]
fn selection_drag_state_only_reports_copy_after_movement() {
    let panel_id = PanelId(42);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(4.0, 4.0), 6.0);
    assert!(!state.finish(panel_id));

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(16.0, 4.0), 6.0);
    assert!(state.finish(panel_id));
    assert!(!state.active_for(panel_id));
}

#[test]
fn selection_drag_state_uses_click_movement_threshold() {
    let panel_id = PanelId(42);
    let mut state = TerminalSelectionDragState::default();

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(9.0, 4.0), 6.0);
    assert!(!state.finish(panel_id));

    state.start(panel_id, Pos2::new(4.0, 4.0));
    state.mark_dragged(panel_id, Pos2::new(11.0, 4.0), 6.0);
    assert!(state.finish(panel_id));
}

#[test]
fn selection_auto_scroll_reports_only_actual_viewport_movement() {
    let mut harness = PointerHarness::new(2);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let above = Pos2::new(body_rect.center().x, body_rect.min.y - LINE_HEIGHT);
    let below = Pos2::new(body_rect.center().x, body_rect.max.y + LINE_HEIGHT);

    assert!(harness.selection_drag_at(above, body_rect));
    assert_eq!(harness.scrollback(), 3);

    let history_size = harness.panel.terminal().expect("terminal").history_size();
    harness.panel.set_scrollback(history_size);
    assert!(!harness.selection_drag_at(above, body_rect));

    harness.panel.set_scrollback(2);
    assert!(harness.selection_drag_at(below, body_rect));
    assert_eq!(harness.scrollback(), 1);

    harness.panel.set_scrollback(0);
    assert!(!harness.selection_drag_at(below, body_rect));
    assert!(!harness.selection_drag_at(body_rect.center(), body_rect));
}

#[test]
fn selection_auto_scroll_repaints_only_while_held_drag_can_move_viewport() {
    let mut harness = PointerHarness::new(5);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 4, 0, 0.25);
    let below = Pos2::new(body_rect.center().x, body_rect.max.y + LINE_HEIGHT);

    harness.frame(primary_press(anchor));
    harness.frame(vec![Event::PointerMoved(below)]);
    // Drain immediate pointer-interaction repaints before measuring the delayed continuation.
    for _ in 0..3 {
        harness.frame(Vec::new());
    }
    let before_continuation = harness.scrollback();

    harness.frame(Vec::new());

    assert_eq!(harness.scrollback(), before_continuation - 1);
    assert!(harness.repaint_delay() > Duration::ZERO);
    assert!(
        harness.repaint_delay() <= Duration::from_millis(16),
        "a held drag below the body should schedule another auto-scroll frame"
    );

    harness.panel.set_scrollback(0);
    harness.frame(Vec::new());
    assert_eq!(harness.scrollback(), 0);
    assert!(
        harness.repaint_delay() > Duration::from_millis(16),
        "auto-scroll should stop repainting at the bottom of the scrollback, got {:?}",
        harness.repaint_delay()
    );

    harness.panel.set_scrollback(2);
    harness.frame(primary_release(below));
    let scrollback_after_release = harness.scrollback();
    harness.frame(Vec::new());
    assert_eq!(harness.scrollback(), scrollback_after_release);

    harness.frame(Vec::new());
    assert_eq!(harness.scrollback(), scrollback_after_release);
    assert!(
        harness.repaint_delay() > Duration::from_millis(16),
        "an outside release should not keep the auto-scroll repaint loop alive"
    );
}

#[test]
fn selection_auto_scroll_is_gated_by_frame_time_and_release_does_not_bypass_deadline() {
    let mut harness = PointerHarness::new(10);
    harness.frame_at(0.0, Vec::new());
    let body_rect = harness.frame_at(0.0, Vec::new());
    let anchor = cell_position(body_rect, 4, 0, 0.25);
    let below = Pos2::new(body_rect.center().x, body_rect.max.y + LINE_HEIGHT);

    harness.frame_at(0.0, primary_press(anchor));
    harness.frame_at(0.0, vec![Event::PointerMoved(below)]);
    assert_eq!(harness.scrollback(), 9);

    harness.frame_at(0.005, Vec::new());
    assert_eq!(harness.scrollback(), 9, "a 5 ms repaint must not auto-scroll again");
    assert!(matches!(
        harness.selection_drag.auto_scroll_cadence(harness.panel.id, 0.005),
        Some(AutoScrollCadence::Waiting(wait)) if wait == Duration::from_millis(11)
    ));

    harness.frame_at(0.010, Vec::new());
    assert_eq!(
        harness.scrollback(),
        9,
        "a 10 ms repaint must preserve the original deadline"
    );
    assert!(matches!(
        harness.selection_drag.auto_scroll_cadence(harness.panel.id, 0.010),
        Some(AutoScrollCadence::Waiting(wait)) if wait == Duration::from_millis(6)
    ));

    harness.frame_at(0.016, Vec::new());
    assert_eq!(
        harness.scrollback(),
        8,
        "the deadline permits exactly one further scroll"
    );
    for _ in 0..8 {
        harness.frame_at(0.016, Vec::new());
    }
    assert_eq!(
        harness.scrollback(),
        8,
        "repeated passes at the same frame time must not accelerate auto-scroll"
    );

    harness.frame_at(0.020, primary_release(below));
    assert_eq!(
        harness.scrollback(),
        8,
        "a release before the next deadline must not add an untimed scroll"
    );
    assert!(!harness.selection_drag.active_for(harness.panel.id));
    harness.frame_at(0.040, Vec::new());
    assert_eq!(harness.scrollback(), 8);
}

#[test]
fn selection_auto_scroll_reentry_resets_the_deadline() {
    let mut harness = PointerHarness::new(10);
    harness.frame_at(0.0, Vec::new());
    let body_rect = harness.frame_at(0.0, Vec::new());
    let anchor = cell_position(body_rect, 4, 0, 0.25);
    let inside = cell_position(body_rect, 6, 4, 0.5);
    let below = Pos2::new(body_rect.center().x, body_rect.max.y + LINE_HEIGHT);

    harness.frame_at(0.0, primary_press(anchor));
    harness.frame_at(0.0, vec![Event::PointerMoved(below)]);
    assert_eq!(harness.scrollback(), 9);

    harness.frame_at(0.005, vec![Event::PointerMoved(inside)]);
    assert_eq!(harness.scrollback(), 9);
    harness.frame_at(0.006, vec![Event::PointerMoved(below)]);
    assert_eq!(
        harness.scrollback(),
        8,
        "leaving the body again starts a fresh auto-scroll interval"
    );

    harness.frame_at(0.007, primary_release(inside));
    assert!(!harness.selection_drag.active_for(harness.panel.id));
}

#[test]
fn wheel_down_during_selection_tracks_the_pointer_in_the_post_scroll_viewport() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 0, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);

    harness.frame(primary_press(anchor));
    harness.frame(vec![Event::PointerMoved(pointer)]);
    assert_eq!(harness.selected_last_line(), harness.viewport_line(4));

    harness.frame(wheel(-1.0));

    assert_eq!(harness.scrollback(), 9);
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(4),
        "the selection endpoint must be recomputed after wheel-down reveals newer text"
    );
}

#[test]
fn wheel_up_during_selection_does_not_overshoot_the_pointer_after_scrolling() {
    let mut harness = PointerHarness::new(2);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 7, 10, 0.75);
    let pointer = cell_position(body_rect, 3, 0, 0.25);

    harness.frame(primary_press(anchor));
    harness.frame(vec![Event::PointerMoved(pointer)]);
    assert_eq!(harness.selected_first_line(), harness.viewport_line(3));

    harness.frame(wheel(1.0));

    assert_eq!(harness.scrollback(), 3);
    assert_eq!(
        harness.selected_first_line(),
        harness.viewport_line(3),
        "the selection endpoint must stay at the pointer after wheel-up reveals older text"
    );
}

#[test]
fn wheel_before_release_finalizes_selection_in_the_post_scroll_viewport() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 0, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);

    harness.frame(primary_press(anchor));
    harness.frame(vec![Event::PointerMoved(pointer)]);

    let mut events = wheel(-1.0);
    events.extend(primary_release(pointer));
    harness.frame(events);

    assert_eq!(harness.scrollback(), 9);
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(4),
        "a release after wheel-down must finalize at the pointer's post-scroll row"
    );
}

#[test]
fn release_before_wheel_keeps_the_pre_scroll_selection_marker() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 0, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);

    harness.frame(primary_press(anchor));
    harness.frame(vec![Event::PointerMoved(pointer)]);
    let selected_before_scroll = harness.selected_last_line();

    let mut events = primary_release(pointer);
    events.extend(wheel(-1.0));
    harness.frame(events);

    assert_eq!(harness.scrollback(), 9);
    assert_eq!(
        harness.selected_last_line(),
        selected_before_scroll,
        "wheel-down after release must not remap the completed selection"
    );
}

#[test]
fn wheel_before_press_starts_selection_in_the_post_scroll_viewport() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 1, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);
    let pre_scroll_anchor = harness.viewport_line(1);

    let mut events = wheel(-1.0);
    events.extend(primary_press(anchor));
    events.push(Event::PointerMoved(pointer));
    harness.frame(events);

    assert_eq!(harness.scrollback(), 9);
    assert_ne!(harness.viewport_line(1), pre_scroll_anchor);
    assert_eq!(
        harness.selected_first_line(),
        harness.viewport_line(1),
        "a press after wheel-down must anchor in the post-scroll viewport"
    );
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(4),
        "the held drag endpoint must use the same post-scroll viewport"
    );
}

#[test]
fn wheel_before_alt_press_replays_local_anchor_in_normal_mode() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 1, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);
    let pre_scroll_anchor = harness.viewport_line(1);

    let mut events = wheel(-1.0);
    events.extend(primary_press_with_modifiers(anchor, Modifiers::ALT));
    events.push(Event::PointerMoved(pointer));
    harness.frame(events);

    assert_eq!(harness.scrollback(), 9);
    assert_ne!(harness.viewport_line(1), pre_scroll_anchor);
    assert_eq!(
        harness.selected_first_line(),
        harness.viewport_line(1),
        "Alt+press after wheel-down must anchor in the post-scroll viewport when mouse reporting is inactive"
    );
    assert_eq!(harness.selected_last_line(), harness.viewport_line(4));
}

#[test]
fn press_before_wheel_preserves_the_pre_scroll_selection_anchor() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 1, 0, 0.25);
    let pointer = cell_position(body_rect, 4, 10, 0.75);
    let pre_scroll_anchor = harness.viewport_line(1);

    let mut events = primary_press(anchor);
    events.extend(wheel(-1.0));
    events.push(Event::PointerMoved(pointer));
    harness.frame(events);

    assert_eq!(harness.scrollback(), 9);
    assert_ne!(harness.viewport_line(1), pre_scroll_anchor);
    assert_eq!(
        harness.selected_first_line(),
        pre_scroll_anchor,
        "wheel-down after the press must not remap the existing selection anchor"
    );
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(4),
        "the held drag endpoint should still track the post-scroll pointer row"
    );
}

#[test]
fn release_then_press_in_one_frame_keeps_the_new_drag_extendable() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let first_anchor = cell_position(body_rect, 0, 0, 0.25);
    let held_pointer = cell_position(body_rect, 2, 5, 0.75);
    let second_anchor = cell_position(body_rect, 5, 0, 0.25);
    let extended = cell_position(body_rect, 6, 10, 0.75);

    harness.frame(primary_press(first_anchor));
    harness.frame(vec![Event::PointerMoved(held_pointer)]);

    let mut events = primary_release(held_pointer);
    events.extend(primary_press(second_anchor));
    harness.frame(events);

    harness.frame(vec![Event::PointerMoved(extended)]);

    assert_eq!(
        harness.selected_first_line(),
        harness.viewport_line(5),
        "a same-frame release and press must anchor a fresh selection at the new press"
    );
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(6),
        "the still-held pointer must keep extending the selection restarted by a same-frame release and press"
    );
}

#[test]
fn inside_release_then_outside_press_completes_at_the_release_position() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let anchor = cell_position(body_rect, 1, 0, 0.25);
    let release = cell_position(body_rect, 4, 10, 0.75);
    let outside = Pos2::new(body_rect.max.x + 20.0, body_rect.max.y + LINE_HEIGHT);
    let farther_outside = outside + Vec2::new(20.0, LINE_HEIGHT);
    let mut events = primary_press(anchor);
    events.push(Event::PointerMoved(release));
    events.extend(primary_release(release));
    events.push(Event::PointerMoved(outside));
    events.push(Event::PointerButton {
        pos: outside,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });

    harness.frame(events);

    assert_eq!(harness.selected_first_line(), harness.viewport_line(1));
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(4),
        "the outside press must not replace the completed release endpoint"
    );
    assert!(!harness.selection_drag.active_for(harness.panel.id));
    let completed_selection = harness
        .panel
        .terminal()
        .expect("terminal")
        .selection_to_string()
        .expect("selection");

    harness.frame(vec![Event::PointerMoved(farther_outside)]);

    assert!(!harness.selection_drag.active_for(harness.panel.id));
    assert_eq!(
        harness
            .panel
            .terminal()
            .expect("terminal")
            .selection_to_string()
            .expect("selection"),
        completed_selection
    );
    harness.frame(primary_release(farther_outside));
}

#[test]
fn release_wheel_and_press_in_one_frame_anchor_the_new_drag_post_scroll() {
    let mut harness = PointerHarness::new(10);
    harness.frame(Vec::new());
    let body_rect = harness.frame(Vec::new());
    let first_anchor = cell_position(body_rect, 0, 0, 0.25);
    let held_pointer = cell_position(body_rect, 2, 5, 0.75);
    let second_anchor = cell_position(body_rect, 5, 0, 0.25);
    let extended = cell_position(body_rect, 6, 10, 0.75);

    harness.frame(primary_press(first_anchor));
    harness.frame(vec![Event::PointerMoved(held_pointer)]);

    let mut events = primary_release(held_pointer);
    events.extend(wheel(-1.0));
    events.extend(primary_press(second_anchor));
    harness.frame(events);
    assert_eq!(harness.scrollback(), 9);

    harness.frame(vec![Event::PointerMoved(extended)]);

    assert_eq!(
        harness.selected_first_line(),
        harness.viewport_line(5),
        "a press after a same-frame release and wheel must anchor in the post-scroll viewport"
    );
    assert_eq!(
        harness.selected_last_line(),
        harness.viewport_line(6),
        "the drag restarted across a same-frame scroll must stay extendable by the held pointer"
    );
}
