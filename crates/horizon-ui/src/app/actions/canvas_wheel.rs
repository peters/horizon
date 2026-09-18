use egui::{Context, Event, MouseWheelUnit, PointerButton, Vec2};

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

pub(super) fn canvas_zoom_multiplier(zoom_delta: f32, zoom_scroll: Vec2) -> Option<f32> {
    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        return Some(zoom_delta);
    }
    if zoom_scroll != Vec2::ZERO {
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

pub(super) fn primary_down_at_frame_start(events: &[Event], primary_at_end: bool) -> bool {
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

pub(super) fn classify_canvas_wheel_events(
    events: &[Event],
    mut primary_down: bool,
    pointer_over_scrollable: bool,
    pointer_over_host_overlay: bool,
    page_height: f32,
) -> CanvasWheelBuckets {
    let mut buckets = CanvasWheelBuckets::default();
    for event in events {
        match event {
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed,
                ..
            } => primary_down = *pressed,
            Event::MouseWheel {
                unit, delta, modifiers, ..
            } => {
                if pointer_over_host_overlay && !(modifiers.ctrl || modifiers.command) {
                    buckets.skipped_for_panel = true;
                    continue;
                }
                if pointer_over_scrollable && panel_content_owns_wheel(*modifiers, primary_down) {
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
            _ => {}
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::{
        CanvasWheelBuckets, CanvasWheelFollowup, canvas_zoom_multiplier, classify_canvas_wheel_events,
        primary_down_at_frame_start, resolve_smoothed_wheel, wheel_pan_scroll_input,
    };
    use egui::{Event, Modifiers, Vec2};

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
        let wheels = classify_canvas_wheel_events(&events, start, true, false, 800.0);
        assert_eq!(wheels.pan, Vec2::ZERO);
        assert_eq!(wheels.zoom, Vec2::ZERO);
    }

    #[test]
    fn mixed_ctrl_and_plain_wheels_in_one_frame_are_partitioned() {
        let events = vec![
            point_wheel(Vec2::new(0.0, 8.0), Modifiers::NONE),
            point_wheel(Vec2::new(0.0, 4.0), Modifiers::CTRL),
        ];
        let wheels = classify_canvas_wheel_events(&events, false, false, false, 800.0);
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
        let wheels = classify_canvas_wheel_events(&events, false, true, false, 800.0);
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
        let wheels = classify_canvas_wheel_events(&events, false, true, true, 800.0);
        assert!(wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::ZERO);
        assert_eq!(wheels.zoom, Vec2::ZERO);
        let ctrl = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::CTRL)];
        let zoom = classify_canvas_wheel_events(&ctrl, false, true, true, 800.0);
        assert!(!zoom.skipped_for_panel);
        assert_eq!(zoom.zoom, Vec2::new(0.0, 8.0));
    }

    #[test]
    fn shift_wheel_outside_a_panel_body_stays_on_the_canvas() {
        let events = vec![point_wheel(Vec2::new(0.0, 8.0), Modifiers::SHIFT)];
        let wheels = classify_canvas_wheel_events(&events, false, false, false, 800.0);
        assert!(!wheels.skipped_for_panel);
        assert_eq!(wheels.pan, Vec2::new(8.0, 0.0));
    }
}
