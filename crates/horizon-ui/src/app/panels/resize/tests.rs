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
    frame_with_foot(ctx, transform, interactive, events, None)
}

/// One pass over a panel whose grip may stop above a strip at the foot of the
/// panel, as the footer of a card does.
fn frame_with_foot(
    ctx: &Context,
    transform: TSTransform,
    interactive: bool,
    events: Vec<Event>,
    foot_span: Option<f32>,
) -> FrameResult {
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
                    let body = ui.interact(panel.shrink(4.0), ui.scope_id().with("body"), Sense::click_and_drag());
                    let resize = show_resize_control(ui, panel, transform.scaling, PanelId(1), interactive, foot_span);
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
fn a_card_grip_stops_at_the_footer_strip_at_each_zoom() {
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(420.0, 500.0));
    let foot = crate::block_widget::CARD_FOOT_SPAN;
    let foot_top = |zoom: f32| (panel.max.y - foot) * zoom;

    for zoom in [1.0, 0.73, 0.5] {
        let rect = handle_rect(panel, zoom, Some(foot));
        let screen_height = rect.height() * zoom;

        assert_eq!(rect.max, panel.max, "the target keeps the corner at zoom {zoom}");
        assert!(
            screen_height <= foot * zoom + 0.01,
            "the target stops at the footer strip at zoom {zoom}: {screen_height} against {}",
            foot * zoom
        );
        assert!(
            rect.min.y * zoom + 0.01 >= foot_top(zoom),
            "no part of the target reaches above the footer at zoom {zoom}: {} against {}",
            rect.min.y * zoom,
            foot_top(zoom)
        );
        assert!(
            screen_height >= FOOT_GRIP_MIN_SCREEN_HEIGHT - 0.01,
            "the target stays usable at zoom {zoom}: {screen_height}"
        );
        assert!(
            (rect.width() * zoom - 32.0).abs() < 0.01,
            "the target keeps the width of a grip at zoom {zoom}"
        );
    }

    let full = handle_rect(panel, 1.0, Some(foot));
    assert!((full.height() - 32.0).abs() < 0.01, "a card at 100 % keeps the square");
    let narrow = handle_rect(panel, 0.73, Some(foot));
    assert!(
        (narrow.height() * 0.73 - foot * 0.73).abs() < 0.01,
        "below full zoom the strip decides the height"
    );
    let square = handle_rect(panel, 0.5, None);
    assert!(
        (square.height() * 0.5 - 32.0).abs() < 0.01,
        "the grip of every other panel keeps its square"
    );
}

#[test]
fn a_button_above_the_footer_keeps_its_press_at_half_zoom() {
    let ctx = Context::default();
    let transform = TSTransform::from_scaling(0.5);
    // The panel that `frame_with_foot` draws.
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
    let foot = crate::block_widget::CARD_FOOT_SPAN;
    frame_with_foot(&ctx, transform, true, vec![], Some(foot));
    frame_with_foot(&ctx, transform, true, vec![], Some(foot));

    // The centre of the danger button of a card: right aligned, one button
    // height above the footer strip.
    let button_centre = transform * Pos2::new(panel.max.x - 6.0, panel.max.y - foot - 20.0);
    let hover = frame_with_foot(
        &ctx,
        transform,
        true,
        vec![Event::PointerMoved(button_centre)],
        Some(foot),
    );
    assert_ne!(
        hover.cursor,
        CursorIcon::ResizeNwSe,
        "the grip leaves the button row alone at 50 % zoom"
    );
    let press = frame_with_foot(&ctx, transform, true, vec![button(button_centre, true)], Some(foot));
    assert_eq!(
        press.raw_body_presses, 1,
        "the press reaches the panel body at 50 % zoom"
    );
    let drag = frame_with_foot(
        &ctx,
        transform,
        true,
        vec![Event::PointerMoved(button_centre + Vec2::new(10.0, 5.0))],
        Some(foot),
    );
    assert!(!drag.resize_dragged, "a press in the button row never resizes");
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
        let rect = handle_rect(panel, zoom, None);
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
    let rect = handle_rect(panel, 1.0, None);
    let (centers, radius) = grip_dots(rect, 1.0);
    assert!((centers[0] - (rect.min + Vec2::new(26.0, 16.0))).length() < 0.01);
    assert!((centers[3] - (rect.min + Vec2::new(16.0, 26.0))).length() < 0.01);
    assert!((centers[5] - (rect.min + Vec2::new(26.0, 26.0))).length() < 0.01);
    assert!((radius - GRIP_DOT_RADIUS).abs() < 0.01);
}

#[test]
fn grip_dots_shrink_together_when_the_corner_cannot_hold_them() {
    let panel = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(400.0, PANEL_TITLEBAR_HEIGHT + 10.0));
    let rect = handle_rect(panel, 1.0, None);
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
    let rect = handle_rect(panel, horizon_core::MIN_CANVAS_ZOOM, None);
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

#[test]
fn the_terminal_scrollbar_stays_clear_of_the_grip_at_each_zoom() {
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
    let available = Vec2::new(
        panel.width() - 2.0 * PANEL_PADDING,
        panel.height() - PANEL_TITLEBAR_HEIGHT - 2.0 * PANEL_PADDING,
    );
    let terminal_origin = panel.min + Vec2::new(PANEL_PADDING, PANEL_TITLEBAR_HEIGHT + PANEL_PADDING);
    for zoom in [0.5, 1.0, 1.5, 2.0, 4.0] {
        let clearance = scrollbar_bottom_clearance(panel, zoom);
        let scrollbar_hit =
            crate::terminal_widget::scrollbar_hit_rect(available, clearance).translate(terminal_origin.to_vec2());
        let grip = handle_rect(panel, zoom, None);
        assert!(
            !scrollbar_hit.intersects(grip),
            "zoom {zoom}: scrollbar hit {scrollbar_hit:?} meets grip {grip:?}"
        );
        assert!(scrollbar_hit.height() > 0.0, "zoom {zoom}");
    }
}

#[test]
fn a_zoomed_out_grip_keeps_a_usable_track_rather_than_the_clearance() {
    let panel = Rect::from_min_size(Pos2::new(80.0, 80.0), Vec2::new(400.0, 320.0));
    let available = Vec2::new(
        panel.width() - 2.0 * PANEL_PADDING,
        panel.height() - PANEL_TITLEBAR_HEIGHT - 2.0 * PANEL_PADDING,
    );
    let zoom = horizon_core::MIN_CANVAS_ZOOM;
    let scrollbar_hit = crate::terminal_widget::scrollbar_hit_rect(available, scrollbar_bottom_clearance(panel, zoom));

    assert!(scrollbar_hit.height() >= 24.0);
    assert!(scrollbar_hit.max.y <= available.y);
}
