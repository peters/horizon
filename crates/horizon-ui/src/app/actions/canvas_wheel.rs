use egui::{Context, Event, MouseWheelUnit, PointerButton, Pos2, Vec2};
use horizon_core::PanelId;

use crate::input::panel_content_owns_wheel;

// egui feeds every wheel/trackpad event into both raw_scroll_delta and
// smooth_scroll_delta; summing them would pan the canvas twice per event.
pub(super) fn wheel_pan_scroll_input(input: &egui::InputState) -> Vec2 {
    input.smooth_scroll_delta
}

/// Matches egui's default `InputOptions::scroll_zoom_speed` so Ctrl+scroll
/// zoom is identical whether egui converted the wheel or we apply it here.
const SCROLL_ZOOM_SPEED: f32 = 1.0 / 200.0;

/// Native default of `InputOptions::line_scroll_speed`.
const LINE_SCROLL_SPEED: f32 = 40.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CanvasWheelBuckets {
    pub zoom: Vec2,
    pub pan: Vec2,
    pub skipped_for_panel: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CanvasWheelFollowup {
    Pan,
    Zoom,
}

pub(super) fn canvas_wheel_followup_id(ctx: &Context) -> egui::Id {
    egui::Id::new(("horizon_canvas_wheel_followup", ctx.viewport_id()))
}

pub(super) fn resolve_smoothed_wheel(
    has_wheel_events: bool,
    buckets: CanvasWheelBuckets,
    smoothed: Vec2,
    previous: Option<CanvasWheelFollowup>,
) -> (Vec2, Vec2, Option<CanvasWheelFollowup>) {
    if has_wheel_events {
        let mixed = buckets.pan != Vec2::ZERO && buckets.zoom != Vec2::ZERO;
        if mixed || (buckets.skipped_for_panel && (buckets.pan != Vec2::ZERO || buckets.zoom != Vec2::ZERO)) {
            return (buckets.zoom, buckets.pan, None);
        }
        if buckets.zoom != Vec2::ZERO {
            // egui may have already converted Ctrl-wheel into zoom_delta and
            // left smooth_scroll_delta at zero; keep the raw bucket then.
            let zoom = if smoothed == Vec2::ZERO { buckets.zoom } else { smoothed };
            return (zoom, Vec2::ZERO, Some(CanvasWheelFollowup::Zoom));
        }
        if buckets.pan != Vec2::ZERO {
            let pan = if smoothed == Vec2::ZERO { buckets.pan } else { smoothed };
            return (Vec2::ZERO, pan, Some(CanvasWheelFollowup::Pan));
        }
        return (Vec2::ZERO, Vec2::ZERO, None);
    }
    if smoothed == Vec2::ZERO {
        return (Vec2::ZERO, Vec2::ZERO, None);
    }
    match previous {
        Some(CanvasWheelFollowup::Zoom) => (smoothed, Vec2::ZERO, previous),
        Some(CanvasWheelFollowup::Pan) => (Vec2::ZERO, smoothed, previous),
        None => (Vec2::ZERO, Vec2::ZERO, None),
    }
}

/// egui latches Ctrl on a trackpad gesture until `TouchPhase::End`, so
/// `zoom_delta` can stay off 1 after the raw event is already classified as
/// pan. The same physical motion must not pan and zoom together.
pub(super) fn drop_pan_when_egui_already_zoomed(zoom_delta: f32, wheels: CanvasWheelBuckets, pan_scroll: Vec2) -> Vec2 {
    if (zoom_delta - 1.0).abs() > f32::EPSILON && wheels.zoom == Vec2::ZERO && wheels.pan != Vec2::ZERO {
        Vec2::ZERO
    } else {
        pan_scroll
    }
}

pub(super) fn canvas_zoom_multiplier(zoom_delta: f32, zoom_scroll: Vec2) -> Option<f32> {
    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        return Some(zoom_delta);
    }
    if zoom_scroll != Vec2::ZERO {
        // Match egui 0.36 `InputState`: Ctrl-wheel zoom is
        // `(scroll_zoom_speed * (delta.x + delta.y)).exp()`.
        return Some((SCROLL_ZOOM_SPEED * (zoom_scroll.x + zoom_scroll.y)).exp());
    }
    None
}

fn wheel_event_points(unit: MouseWheelUnit, delta: Vec2, page_height: f32) -> Vec2 {
    match unit {
        MouseWheelUnit::Point => delta,
        MouseWheelUnit::Line => LINE_SCROLL_SPEED * delta,
        MouseWheelUnit::Page => Vec2::new(delta.x * page_height, delta.y * page_height),
    }
}

#[cfg(test)]
fn primary_down_at_frame_start(events: &[Event], primary_at_end: bool) -> bool {
    let mut primary = primary_at_end;
    for event in events.iter().rev() {
        if let Event::PointerButton {
            button: PointerButton::Primary,
            pressed,
            ..
        } = event
        {
            primary = !*pressed;
        }
    }
    primary
}

pub(super) fn apply_primary_gesture_event(
    gesture_panel: &mut Option<PanelId>,
    event: &Event,
    panel_at: impl Fn(Pos2) -> Option<PanelId>,
) {
    let Event::PointerButton {
        button: PointerButton::Primary,
        pressed,
        pos,
        ..
    } = event
    else {
        return;
    };
    *gesture_panel = if *pressed { panel_at(*pos) } else { None };
}

pub(super) fn classify_canvas_wheel_events(
    events: &[Event],
    mut gesture_panel: Option<PanelId>,
    current_topmost: Option<PanelId>,
    pointer_over_scrollable: bool,
    pointer_over_host_overlay: bool,
    page_height: f32,
    panel_at: impl Fn(Pos2) -> Option<PanelId>,
) -> (CanvasWheelBuckets, Option<PanelId>) {
    let mut buckets = CanvasWheelBuckets::default();
    for event in events {
        apply_primary_gesture_event(&mut gesture_panel, event, &panel_at);
        let Event::MouseWheel {
            unit, delta, modifiers, ..
        } = event
        else {
            continue;
        };
        let panel_primary = gesture_panel.is_some() && gesture_panel == current_topmost;
        if pointer_over_host_overlay && !(modifiers.ctrl || modifiers.command) {
            buckets.skipped_for_panel = true;
            continue;
        }
        if pointer_over_scrollable && panel_content_owns_wheel(*modifiers, panel_primary) {
            buckets.skipped_for_panel = true;
            continue;
        }
        let points = wheel_event_points(*unit, *delta, page_height);
        if modifiers.ctrl || modifiers.command {
            buckets.zoom += points;
        } else if modifiers.shift && points.x == 0.0 {
            buckets.pan += Vec2::new(points.y, 0.0);
        } else {
            buckets.pan += points;
        }
    }
    (buckets, gesture_panel)
}

#[cfg(test)]
mod tests {
    use super::{
        CanvasWheelBuckets, CanvasWheelFollowup, canvas_zoom_multiplier, classify_canvas_wheel_events,
        drop_pan_when_egui_already_zoomed, primary_down_at_frame_start, resolve_smoothed_wheel, wheel_event_points,
        wheel_pan_scroll_input,
    };
    use egui::{Event, Modifiers, Vec2};
    use horizon_core::PanelId;

    const PANEL: PanelId = PanelId(1);

    fn classify(
        events: &[Event],
        primary_at_start: bool,
        over_scrollable: bool,
        over_overlay: bool,
    ) -> CanvasWheelBuckets {
        classify_at(events, primary_at_start, over_scrollable, over_overlay, |_| Some(PANEL)).0
    }

    fn classify_at(
        events: &[Event],
        primary_at_start: bool,
        over_scrollable: bool,
        over_overlay: bool,
        panel_at: impl Fn(egui::Pos2) -> Option<PanelId>,
    ) -> (CanvasWheelBuckets, Option<PanelId>) {
        classify_canvas_wheel_events(
            events,
            primary_at_start.then_some(PANEL),
            Some(PANEL),
            over_scrollable,
            over_overlay,
            800.0,
            panel_at,
        )
    }

    fn point_wheel(delta: Vec2, modifiers: Modifiers) -> Event {
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers,
        }
    }

    #[test]
    fn wheel_pan_scroll_input_counts_each_wheel_event_once() {
        let delta = Vec2::new(3.0, -5.0);
        let raw_input = egui::RawInput {
            events: vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };

        let input = egui::InputState::default().begin_pass(raw_input, false, 1.0, egui::InputOptions::default());

        // A point-unit trackpad delta must land exactly once in the smoothed
        // delta that wheel pan consumes; egui no longer exposes a raw scroll
        // delta, so this single field is the whole pan input.
        assert_eq!(input.smooth_scroll_delta, delta);
        assert_eq!(wheel_pan_scroll_input(&input), delta);
    }

    #[test]
    fn ctrl_scroll_zooms_when_egui_leaves_zoom_delta_unchanged() {
        assert_eq!(canvas_zoom_multiplier(1.0, Vec2::ZERO), None);
        let factor = canvas_zoom_multiplier(1.0, Vec2::new(0.0, 12.0)).expect("ctrl+scroll");
        assert!((factor - (12.0_f32 / 200.0).exp()).abs() < f32::EPSILON);
        let diagonal = canvas_zoom_multiplier(1.0, Vec2::new(4.0, 8.0)).expect("egui x+y");
        assert!((diagonal - (12.0_f32 / 200.0).exp()).abs() < f32::EPSILON);
        assert_eq!(canvas_zoom_multiplier(1.25, Vec2::ZERO), Some(1.25));
    }

    #[test]
    fn wheel_before_primary_release_stays_on_the_panel() {
        let events = vec![
            point_wheel(Vec2::new(0.0, -12.0), Modifiers::NONE),
            Event::PointerButton {
                pos: egui::pos2(1.0, 1.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ];
        let start = primary_down_at_frame_start(&events, false);
        assert!(start);
        let wheels = classify(&events, start, true, false);
        assert_eq!(wheels.pan, Vec2::ZERO);
        assert_eq!(wheels.zoom, Vec2::ZERO);
    }

    #[test]
    fn mixed_ctrl_and_plain_wheels_in_one_frame_are_partitioned() {
        let events = vec![
            point_wheel(Vec2::new(0.0, 8.0), Modifiers::NONE),
            point_wheel(Vec2::new(0.0, 4.0), Modifiers::CTRL),
        ];
        let wheels = classify(&events, false, false, false);
        assert_eq!(wheels.pan, Vec2::new(0.0, 8.0));
        assert_eq!(wheels.zoom, Vec2::new(0.0, 4.0));
        let (zoom, pan, followup) = resolve_smoothed_wheel(true, wheels, Vec2::new(0.0, 99.0), None);
        assert_eq!(zoom, wheels.zoom);
        assert_eq!(pan, wheels.pan);
        assert_eq!(followup, None);
    }

    #[test]
    fn uniform_wheel_uses_smoothed_delta_and_followup_frames() {
        let pan_buckets = CanvasWheelBuckets {
            pan: Vec2::new(0.0, 8.0),
            zoom: Vec2::ZERO,
            skipped_for_panel: false,
        };
        let smoothed = Vec2::new(0.0, 3.0);
        let (zoom, pan, followup) = resolve_smoothed_wheel(true, pan_buckets, smoothed, None);
        assert_eq!(zoom, Vec2::ZERO);
        assert_eq!(pan, smoothed);
        assert_eq!(followup, Some(CanvasWheelFollowup::Pan));
        let leftover = Vec2::new(0.0, 2.0);
        let (zoom, pan, followup) = resolve_smoothed_wheel(false, CanvasWheelBuckets::default(), leftover, followup);
        assert_eq!(zoom, Vec2::ZERO);
        assert_eq!(pan, leftover);
        assert_eq!(followup, Some(CanvasWheelFollowup::Pan));
    }

    #[test]
    fn ctrl_wheel_event_frame_keeps_raw_zoom_when_smoothed_delta_is_empty() {
        let zoom_buckets = CanvasWheelBuckets {
            pan: Vec2::ZERO,
            zoom: Vec2::new(0.0, 12.0),
            skipped_for_panel: false,
        };
        let (zoom, pan, followup) = resolve_smoothed_wheel(true, zoom_buckets, Vec2::ZERO, None);
        assert_eq!(zoom, Vec2::new(0.0, 12.0));
        assert_eq!(pan, Vec2::ZERO);
        assert_eq!(followup, Some(CanvasWheelFollowup::Zoom));
    }

    #[test]
    fn mixed_panel_and_canvas_wheels_use_raw_canvas_bucket() {
        let events = vec![
            point_wheel(Vec2::new(0.0, 5.0), Modifiers::SHIFT),
            point_wheel(Vec2::new(0.0, 8.0), Modifiers::NONE),
        ];
        let wheels = classify(&events, false, true, false);
        assert!(wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::new(0.0, 8.0));
        let (zoom, pan, followup) = resolve_smoothed_wheel(true, wheels, Vec2::new(0.0, 13.0), None);
        assert_eq!(zoom, Vec2::ZERO);
        assert_eq!(pan, Vec2::new(0.0, 8.0));
        assert_eq!(followup, None);
    }

    #[test]
    fn native_select_popup_keeps_unmodified_wheel() {
        let events = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::NONE)];
        let wheels = classify(&events, false, true, true);
        assert!(wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::ZERO);
        assert_eq!(wheels.zoom, Vec2::ZERO);
        let ctrl = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::CTRL)];
        let zoom = classify(&ctrl, false, true, true);
        assert!(!zoom.skipped_for_panel);
        assert_eq!(zoom.zoom, Vec2::new(0.0, 8.0));
    }

    #[test]
    fn shift_wheel_outside_a_panel_body_stays_on_the_canvas() {
        let events = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::SHIFT)];
        let wheels = classify(&events, false, false, false);
        assert!(!wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::new(8.0, 0.0));
    }

    #[test]
    fn alt_wheel_over_a_panel_body_stays_on_the_panel() {
        let events = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::ALT)];
        let wheels = classify(&events, false, true, false);
        assert!(wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::ZERO);
        assert_eq!(wheels.zoom, Vec2::ZERO);
    }

    #[test]
    fn primary_held_off_the_panel_does_not_block_canvas_pan() {
        let events = vec![
            Event::PointerButton {
                pos: egui::pos2(1.0, 1.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            point_wheel(Vec2::new(0.0, 8.0), Modifiers::NONE),
        ];
        let (wheels, gesture) = classify_at(&events, false, true, false, |_| None);
        assert!(!wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::new(0.0, 8.0));
        assert_eq!(gesture, None);
    }

    #[test]
    fn page_wheel_uses_the_passed_viewport_height() {
        assert_eq!(
            wheel_event_points(egui::MouseWheelUnit::Page, Vec2::new(0.0, 1.0), 900.0),
            Vec2::new(0.0, 900.0)
        );
    }

    #[test]
    fn latched_zoom_delta_does_not_also_pan() {
        let wheels = CanvasWheelBuckets {
            pan: Vec2::new(0.0, 8.0),
            zoom: Vec2::ZERO,
            skipped_for_panel: false,
        };
        assert_eq!(
            drop_pan_when_egui_already_zoomed(1.25, wheels, Vec2::new(0.0, 8.0)),
            Vec2::ZERO
        );
        assert_eq!(
            drop_pan_when_egui_already_zoomed(1.0, wheels, Vec2::new(0.0, 8.0)),
            Vec2::new(0.0, 8.0)
        );
    }
}
