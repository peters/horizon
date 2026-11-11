use crate::test_egui::DiscardTextures;
use egui::{Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use horizon_core::{AgentSessionBinding, PanelKind};

use super::{CanvasClippedArea, clip_screen_rect_to_canvas, render_session_rebind_options};

fn rebind_options_frame(
    ctx: &Context,
    events: Vec<Event>,
    options: &[AgentSessionBinding],
) -> super::SessionRebindRenderOutcome {
    rebind_options_frame_output(ctx, events, options).0
}

fn rebind_options_frame_output(
    ctx: &Context,
    events: Vec<Event>,
    options: &[AgentSessionBinding],
) -> (super::SessionRebindRenderOutcome, Vec<egui::OutputCommand>) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 640.0))),
        events,
        ..RawInput::default()
    };
    let mut outcome = None;
    let output = ctx
        .run_ui(input, |ui| {
            outcome = Some(
                egui::CentralPanel::default()
                    .show(ui, |ui| render_session_rebind_options(ui, options))
                    .inner,
            );
        })
        .discard_textures();
    (
        outcome.expect("rebind options frame ran"),
        output.platform_output.commands,
    )
}

fn session_binding(session_id: &str) -> AgentSessionBinding {
    AgentSessionBinding::new(
        PanelKind::Codex,
        session_id.to_string(),
        Some("/repo".to_string()),
        None,
        None,
    )
}

fn overlay_click_frame(ctx: &Context, events: Vec<Event>, panel_clip: Rect) -> bool {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 240.0))),
        events,
        ..RawInput::default()
    };
    let mut clicked = false;
    let _ = ctx
        .run_ui(input, |ui| {
            clicked = ui
                .put(
                    Rect::from_min_size(Pos2::new(300.0, 80.0), Vec2::new(100.0, 30.0)),
                    egui::Button::new("Settings"),
                )
                .clicked();
            egui::Area::new(Id::unique("panel-overlap-test"))
                .fixed_pos(Pos2::ZERO)
                .clip_to_canvas(panel_clip)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let _ = ui.allocate_exact_size(Vec2::new(500.0, 200.0), egui::Sense::click_and_drag());
                });
        })
        .discard_textures();
    clicked
}

fn click_overlay_through_panel_clip(panel_clip: Rect) -> bool {
    let ctx = Context::default();
    let target = Pos2::new(350.0, 95.0);
    assert!(!overlay_click_frame(&ctx, Vec::new(), panel_clip));
    assert!(!overlay_click_frame(&ctx, Vec::new(), panel_clip));
    assert!(!overlay_click_frame(
        &ctx,
        vec![
            Event::PointerMoved(target),
            Event::PointerButton {
                pos: target,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        panel_clip,
    ));
    overlay_click_frame(
        &ctx,
        vec![Event::PointerButton {
            pos: target,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        panel_clip,
    )
}

#[test]
fn clip_screen_rect_to_canvas_intersects_with_canvas_bounds() {
    let canvas_rect = Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(420.0, 320.0));
    let raw_rect = Rect::from_min_max(Pos2::new(60.0, 40.0), Pos2::new(180.0, 180.0));

    assert_eq!(
        clip_screen_rect_to_canvas(raw_rect, canvas_rect),
        Some(Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(180.0, 180.0)))
    );
}

#[test]
fn clip_screen_rect_to_canvas_rejects_non_positive_intersections() {
    let canvas_rect = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(320.0, 240.0));
    let raw_rect = Rect::from_min_size(Pos2::new(430.0, 90.0), Vec2::new(80.0, 80.0));

    assert_eq!(clip_screen_rect_to_canvas(raw_rect, canvas_rect), None);
}

#[test]
fn canvas_clipped_panel_area_does_not_block_overlay_input() {
    let full_panel_clip = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 240.0));
    assert!(!click_overlay_through_panel_clip(full_panel_clip));

    let canvas_clip = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 240.0));
    assert!(click_overlay_through_panel_clip(canvas_clip));
}

#[test]
fn clicking_a_rebind_and_restart_row_returns_the_exact_session() {
    let ctx = Context::default();
    let options = vec![session_binding("session-1"), session_binding("session-2")];
    let initial = rebind_options_frame(&ctx, Vec::new(), &options);
    let second_row = initial.option_rects.get(1).expect("second rebind row").center();

    let pressed = rebind_options_frame(
        &ctx,
        vec![
            Event::PointerMoved(second_row),
            Event::PointerButton {
                pos: second_row,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        &options,
    );
    assert!(pressed.binding.is_none());

    let released = rebind_options_frame(
        &ctx,
        vec![Event::PointerButton {
            pos: second_row,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        &options,
    );

    assert_eq!(released.binding, Some(options[1].clone()));
}

#[test]
fn copying_a_session_id_does_not_resume_the_session() {
    let ctx = Context::default();
    let options = vec![session_binding("01a0f8a8-d7e2-7810-9e6b-8903152a6187")];
    let initial = rebind_options_frame(&ctx, Vec::new(), &options);
    let at = initial.copy_rects[0].center();
    let pressed = rebind_options_frame(
        &ctx,
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        &options,
    );
    assert!(pressed.binding.is_none());
    let (released, commands) = rebind_options_frame_output(
        &ctx,
        vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        &options,
    );
    assert!(released.binding.is_none());
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == &options[0].session_id))
    );
}

#[test]
fn long_session_labels_leave_the_copy_action_inside_a_narrow_menu() {
    let ctx = Context::default();
    let mut binding = session_binding("01a0f8a8-d7e2-7810-9e6b-8903152a6187");
    binding.label = Some("W".repeat(100));
    binding.updated_at = Some(1_790_883_985_973);
    let mut outcome = None;
    let _ = ctx
        .run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 600.0))),
                ..RawInput::default()
            },
            |ui| {
                outcome = Some(
                    egui::CentralPanel::default()
                        .show(ui, |ui| render_session_rebind_options(ui, &[binding.clone()]))
                        .inner,
                );
            },
        )
        .discard_textures();
    let outcome = outcome.expect("narrow menu frame ran");
    assert!(outcome.copy_rects[0].right() < 320.0, "copy action must stay on screen");
    assert!(outcome.option_rects[0].right() <= outcome.copy_rects[0].left());
}
