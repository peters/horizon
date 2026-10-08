use egui::emath::TSTransform;
use egui::{Context, CursorIcon, Event, Modifiers, PointerButton, RawInput, Sense};

use super::*;
use crate::test_egui::DiscardTextures;

#[derive(Default)]
struct FrameResult {
    resize_dragged: bool,
    body_dragged: bool,
    stopped: bool,
    delta: Vec2,
    cursor: CursorIcon,
    raw_body_presses: usize,
}

fn frame(ctx: &Context, zoom: f32, interactive: bool, events: Vec<Event>) -> FrameResult {
    frame_with_transform(ctx, TSTransform::from_scaling(zoom), interactive, events)
}

fn frame_with_transform(ctx: &Context, transform: TSTransform, interactive: bool, events: Vec<Event>) -> FrameResult {
    let mut result = FrameResult::default();
    let output = ctx
        .run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 1000.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ctx.set_transform_layer(ui.layer_id(), transform);
                    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
                    let body = ui.interact(panel.shrink(4.0), ui.id().with("body"), Sense::click_and_drag());
                    let resize = show_resize_control(ui, panel, transform.scaling, PanelId(1), interactive);
                    let pointer_events = ui.input(|input| input.events.clone());
                    result.raw_body_presses = pointer_events
                        .iter()
                        .filter(|event| {
                            matches!(event, Event::PointerButton { pos, pressed: true, .. }
                            if body.interact_rect.contains(transform.inverse() * *pos)
                                && ctx.layer_id_at(*pos) == Some(body.layer_id))
                        })
                        .count();
                    result.body_dragged = body.dragged();
                    result.resize_dragged = resize.dragged();
                    result.delta = drag_delta(&resize);
                    result.stopped = resize.drag_stopped();
                });
            },
        )
        .discard_textures();
    result.cursor = output.platform_output.cursor_icon;
    result
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn enlarged_grip_claims_drag_over_body_at_each_canvas_zoom() {
    for zoom in [0.25, 0.5, 1.0, 2.0] {
        let ctx = Context::default();
        frame(&ctx, zoom, true, vec![]);
        frame(&ctx, zoom, true, vec![]);
        let start = Pos2::new(480.0 * zoom - 28.0, 400.0 * zoom - 28.0);
        let end = start + Vec2::new(20.0, 12.0);
        let hover = frame(&ctx, zoom, true, vec![Event::PointerMoved(start)]);
        assert_eq!(hover.cursor, CursorIcon::ResizeNwSe, "zoom {zoom}");
        let press = frame(&ctx, zoom, true, vec![button(start, true)]);
        assert_eq!(press.raw_body_presses, 0, "raw body input leaked at zoom {zoom}");
        let drag = frame(&ctx, zoom, true, vec![Event::PointerMoved(end)]);
        assert!(drag.resize_dragged, "resize did not claim drag at zoom {zoom}");
        assert!(!drag.body_dragged, "body stole resize drag at zoom {zoom}");
        assert!((drag.delta * zoom - Vec2::new(20.0, 12.0)).length() < 0.01);
        let release = frame(&ctx, zoom, true, vec![button(end, false)]);
        assert!(release.stopped, "resize release was lost at zoom {zoom}");
    }
}

#[test]
fn pointer_outside_grip_keeps_body_interaction() {
    let ctx = Context::default();
    frame(&ctx, 1.0, true, vec![]);
    frame(&ctx, 1.0, true, vec![]);
    let start = Pos2::new(440.0, 360.0);
    let hover = frame(&ctx, 1.0, true, vec![Event::PointerMoved(start)]);
    assert_ne!(hover.cursor, CursorIcon::ResizeNwSe);
    frame(&ctx, 1.0, true, vec![button(start, true)]);
    let drag = frame(&ctx, 1.0, true, vec![Event::PointerMoved(start + Vec2::splat(20.0))]);
    assert!(drag.body_dragged);
    assert!(!drag.resize_dragged);
}

#[test]
fn disabled_grip_does_not_claim_body_input_or_resize_cursor() {
    let ctx = Context::default();
    frame(&ctx, 1.0, false, vec![]);
    frame(&ctx, 1.0, false, vec![]);
    let start = Pos2::new(452.0, 372.0);
    let hover = frame(&ctx, 1.0, false, vec![Event::PointerMoved(start)]);
    assert_ne!(hover.cursor, CursorIcon::ResizeNwSe);
    frame(&ctx, 1.0, false, vec![button(start, true)]);
    let drag = frame(&ctx, 1.0, false, vec![Event::PointerMoved(start + Vec2::splat(20.0))]);
    assert!(drag.body_dragged);
    assert!(!drag.resize_dragged);
}

#[test]
fn grip_dots_stay_inside_the_hit_target_at_each_zoom() {
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
    for zoom in [horizon_core::MIN_CANVAS_ZOOM, 0.25, 0.5, 1.0, 2.0] {
        let rect = handle_rect(panel, zoom);
        let (centers, radius) = grip_dots(rect, zoom);
        assert!(radius > 0.0, "zoom {zoom}");
        let screen_span = rect.width() * zoom;
        assert!(
            screen_span + 0.01 >= 10.0 + 2.0 * GRIP_DOT_RADIUS,
            "fixture handle should hold full dots at zoom {zoom}"
        );
        assert!(
            (radius * zoom - GRIP_DOT_RADIUS).abs() < 0.01,
            "screen radius {} at zoom {zoom} (handle {screen_span} screen points)",
            radius * zoom
        );
        for (index, center) in centers.iter().enumerate() {
            let dot = Rect::from_center_size(*center, Vec2::splat(radius * 2.0));
            assert!(
                rect.contains_rect(dot),
                "dot {index} at {center:?} radius {radius} left {rect:?} at zoom {zoom}"
            );
            for other in centers.iter().skip(index + 1) {
                assert!(
                    (*center - *other).length() + 0.01 >= radius * 2.0,
                    "dots overlap at zoom {zoom}"
                );
            }
        }
    }
    let rect = handle_rect(panel, 1.0);
    let (centers, radius) = grip_dots(rect, 1.0);
    assert!((centers[0] - (rect.min + Vec2::new(26.0, 16.0))).length() < 0.01);
    assert!((centers[3] - (rect.min + Vec2::new(16.0, 26.0))).length() < 0.01);
    assert!((centers[5] - (rect.min + Vec2::new(26.0, 26.0))).length() < 0.01);
    assert!((radius - GRIP_DOT_RADIUS).abs() < 0.01);
}

#[test]
fn grip_dots_shrink_together_when_the_corner_cannot_hold_them() {
    let panel = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(400.0, PANEL_TITLEBAR_HEIGHT + 10.0));
    let rect = handle_rect(panel, 1.0);
    assert!((rect.width() - 10.0).abs() < 0.01);
    let (centers, radius) = grip_dots(rect, 1.0);
    assert!(radius < GRIP_DOT_RADIUS);
    assert!(radius > 0.0);
    for (index, center) in centers.iter().enumerate() {
        let dot = Rect::from_center_size(*center, Vec2::splat(radius * 2.0));
        assert!(
            rect.contains_rect(dot),
            "dot {index} at {center:?} radius {radius} left {rect:?}"
        );
        for other in centers.iter().skip(index + 1) {
            assert!(
                (*center - *other).length() + 0.01 >= radius * 2.0,
                "dots overlap in a short corner"
            );
        }
    }
}

#[test]
fn grip_stays_within_tiny_panel_at_minimum_zoom() {
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
    let rect = handle_rect(panel, horizon_core::MIN_CANVAS_ZOOM);
    assert!(panel.contains_rect(rect));
    assert_eq!(rect.max, panel.max);
    assert!(rect.min.y >= panel.min.y + PANEL_TITLEBAR_HEIGHT);
}

#[test]
fn grip_blocks_raw_body_press_during_canvas_pan_and_zoom_change() {
    let ctx = Context::default();
    frame(&ctx, 1.0, true, vec![]);
    frame(&ctx, 1.0, true, vec![]);
    let transform = TSTransform::new(Vec2::new(100.0, 150.0), 0.5);
    let press_at = transform * Pos2::new(480.0, 400.0) - Vec2::splat(28.0);
    let press = frame_with_transform(&ctx, transform, true, vec![button(press_at, true)]);
    assert_eq!(press.raw_body_presses, 0);
}

#[test]
fn initial_grip_press_does_not_resize_by_motion_before_the_press() {
    let ctx = Context::default();
    frame(&ctx, 1.0, true, vec![]);
    frame(&ctx, 1.0, true, vec![Event::PointerMoved(Pos2::new(100.0, 100.0))]);
    let start = Pos2::new(452.0, 372.0);
    let press = frame(&ctx, 1.0, true, vec![Event::PointerMoved(start), button(start, true)]);
    assert_eq!(press.delta, Vec2::ZERO);
    let drag = frame(
        &ctx,
        1.0,
        true,
        vec![Event::PointerMoved(start + Vec2::new(20.0, 12.0))],
    );
    assert_eq!(drag.delta, Vec2::new(20.0, 12.0));
}
