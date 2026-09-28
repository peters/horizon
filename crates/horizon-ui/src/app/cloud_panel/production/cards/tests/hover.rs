use super::*;
use crate::app::cloud_panel::production::rebuild::tests::{Phase, deployment};
use crate::app::test_support::test_app_with_startup;
use egui::{Context, Event, LayerId, Modifiers, MouseWheelUnit, RawInput, Rect, TouchPhase};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

const ID: u32 = 901;

fn frame(ctx: &Context, app: &mut HorizonApp, time: f64, position: Pos2, event: Option<Event>) {
    let mut events = vec![Event::PointerMoved(position)];
    events.extend(event);
    let _ = ctx
        .run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(3000.0, 2200.0))),
                time: Some(time),
                events,
                ..RawInput::default()
            },
            |ui| {
                app.handle_canvas_pan(ui.ctx());
                app.render_production_runtimes(ui.ctx());
            },
        )
        .discard_textures();
}

fn wheel(delta: f32) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, delta),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    }
}

fn press(position: Pos2) -> Event {
    Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }
}

/// The card's buttons in card coordinates, top to bottom.
fn controls(ctx: &Context) -> Vec<(egui::Id, Rect)> {
    let layer = LayerId::new(Order::Middle, Id::new(("cloud-runtime", ID)));
    ctx.viewport(|viewport| {
        viewport
            .this_pass
            .widgets
            .get_layer(layer)
            .filter(|widget| {
                widget.sense.senses_click() && widget.sense.is_focusable() && widget.interact_rect.is_positive()
            })
            .map(|widget| (widget.id, widget.interact_rect))
            .collect()
    })
}

fn ready_card(zoom: f32) -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.cloud_prototype.ready = true;
    app.canvas_view = CanvasViewState::new([0.0, 0.0], zoom);
    let state = deployment(std::path::Path::new("/synthetic"), true, Phase::None);
    let mut group = CloudGroup::new(
        ID,
        "Hover fixture".into(),
        "workspace".into(),
        "/synthetic".into(),
        [10.0, 10.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "hover-fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: state.profile.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    let runtime = app.cloud_prototype.production.runtimes.entry(ID).or_default();
    runtime.stage = Some(Stage::Ready);
    runtime.state = Some(state);
    (temp, ctx, app)
}

#[test]
fn entering_the_card_scroll_area_keeps_lower_controls_under_the_pointer() {
    for zoom in [1.0_f32, 1.797] {
        let (_temp, ctx, mut app) = ready_card(zoom);
        let outside = Pos2::new(2900.0, 2100.0);
        let transform = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view);
        let (min, max) = app.cloud_prototype.groups.0[0].runtime_bounds();
        let card = Rect::from_min_max(Pos2::from(min), Pos2::from(max));
        let mut time = 0.0;
        let mut step = |app: &mut HorizonApp, position: Pos2, event: Option<Event>| {
            time += 0.05;
            frame(&ctx, app, time, position, event);
        };
        for _ in 0..3 {
            step(&mut app, outside, None);
        }
        // Scroll part way down, so a jump in either direction would show.
        let body = transform * (card.min + Vec2::new(100.0, 300.0));
        step(&mut app, body, Some(wheel(-120.0)));
        for _ in 0..40 {
            step(&mut app, outside, None);
        }
        let before = controls(&ctx);
        let (target, rect) = *before
            .iter()
            .filter(|(_, rect)| rect.bottom() < card.bottom() - 40.0 && rect.width() > card.width() * 0.6)
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("the card shows a full-width button near its bottom");
        // Aim at the button's right end, where a floating scroll bar would lie.
        let aim = transform * Pos2::new(rect.right() - 2.0, rect.center().y);
        for _ in 0..20 {
            step(&mut app, aim, None);
            assert_eq!(
                controls(&ctx),
                before,
                "hovering the card moved its controls at zoom {zoom}"
            );
        }
        step(&mut app, aim, Some(press(aim)));
        step(&mut app, aim, None);
        assert_eq!(
            controls(&ctx),
            before,
            "pressing a control moved the card at zoom {zoom}"
        );
        assert!(
            ctx.read_response(target)
                .is_some_and(|response| response.is_pointer_button_down_on()),
            "the press at zoom {zoom} must land on the control the pointer aimed at"
        );
    }
}
