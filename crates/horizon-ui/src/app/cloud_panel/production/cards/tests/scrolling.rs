use super::*;
use crate::app::test_support::test_app_with_startup;
use egui::{Context, Event, Modifiers, MouseWheelUnit, RawInput, TouchPhase};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

fn frame(ctx: &Context, app: &mut HorizonApp, time: f64, position: Pos2, delta: f32) -> egui::FullOutput {
    phased_frame(ctx, app, time, position, delta, TouchPhase::Move)
}

fn phased_frame(
    ctx: &Context,
    app: &mut HorizonApp,
    time: f64,
    position: Pos2,
    delta: f32,
    phase: TouchPhase,
) -> egui::FullOutput {
    ctx.run_ui(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(3000.0, 2200.0))),
            time: Some(time),
            events: vec![
                Event::PointerMoved(position),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, delta),
                    phase,
                    modifiers: Modifiers::NONE,
                },
            ],
            ..RawInput::default()
        },
        |ui| {
            app.handle_canvas_pan(ui.ctx());
            app.render_production_runtimes(ui.ctx());
        },
    )
    .discard_textures()
}

#[test]
fn overflowing_runtime_scrolls_without_panning_at_normal_and_scaled_zoom() {
    for zoom in [1.0, 1.797] {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.cloud_prototype.ready = true;
        app.canvas_view = CanvasViewState::new([0.0, 0.0], zoom);
        let profile = super::super::super::CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap().profiles["dev"].clone();
        let mut group = CloudGroup::new(
            901,
            "Scroll fixture".into(),
            "workspace".into(),
            "/synthetic".into(),
            [10.0, 10.0],
        );
        group.remote = Some(CloudLaunch {
            deployment_started: false,
            id: "scroll-fixture".into(),
            revision: "a".repeat(40),
            profile_name: "long profile ".repeat(50),
            profile,
        });
        app.cloud_prototype.groups.0.push(group);
        for step in 0..3 {
            frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
        }
        let group = &app.cloud_prototype.groups.0[0];
        let (min, _) = group.runtime_bounds();
        let transform = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view);
        let point = transform * (Pos2::from(min) + Vec2::new(100.0, 300.0));
        assert!(app.pointer_over_cloud_runtime(&ctx, point));
        let pan = app.canvas_view.pan_offset;
        let before = frame(&ctx, &mut app, 0.9, point, 0.0);
        assert!(!visible_label(&before, "Deploy cloud"));
        frame(&ctx, &mut app, 1.0, point, -4000.0);
        let output = frame(&ctx, &mut app, 1.02, point, 0.0);
        assert!((Vec2::from(app.canvas_view.pan_offset) - Vec2::from(pan)).length() < 0.001);
        assert!(!app.canvas_pan_input_claimed);
        assert!(
            visible_label(&output, "Deploy cloud"),
            "scroll must reveal the deployment action at zoom {zoom}"
        );
        let outside = app.canvas_rect(&ctx).min + Vec2::new(10.0, 10.0);
        frame(&ctx, &mut app, 2.0, outside, -5.0);
        assert!(app.canvas_pan_input_claimed);
        let moved = app.canvas_view.pan_offset;
        assert!(moved[1] < pan[1]);
        frame(&ctx, &mut app, 2.02, point, -5.0);
        assert!(
            app.canvas_pan_input_claimed,
            "canvas gesture keeps ownership when crossing a card"
        );
        assert!(app.canvas_view.pan_offset[1] < moved[1]);
        app.cloud_prototype.groups.0[0].remote = None;
        assert!(
            !app.pointer_over_cloud_runtime(&ctx, point),
            "invisible runtime must not consume scrolling"
        );
    }
}

fn visible_label(output: &egui::FullOutput, label: &str) -> bool {
    output.shapes.iter().any(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => {
            shape.clip_rect.contains(text.pos + text.galley.size() * 0.5)
        }
        _ => false,
    })
}

#[test]
fn a_runtime_contact_cannot_scroll_another_runtime_card() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.cloud_prototype.ready = true;
    app.canvas_view = CanvasViewState::new([0.0, 0.0], 1.0);
    let profile = super::super::super::CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap().profiles["dev"].clone();
    for (id, x) in [(901, 10.0), (902, 900.0)] {
        let mut group = CloudGroup::new(
            id,
            "Scroll fixture".into(),
            "workspace".into(),
            "/synthetic".into(),
            [x, 10.0],
        );
        group.remote = Some(CloudLaunch {
            deployment_started: false,
            id: format!("scroll-fixture-{id}"),
            revision: "a".repeat(40),
            profile_name: "long profile ".repeat(50),
            profile: profile.clone(),
        });
        app.cloud_prototype.groups.0.push(group);
    }
    for step in 0..3 {
        frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    let transform = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view);
    let bounds = |group: &CloudGroup| {
        let (min, max) = group.runtime_bounds();
        transform * egui::Rect::from_min_max(Pos2::from(min), Pos2::from(max))
    };
    let first = bounds(&app.cloud_prototype.groups.0[0]);
    let second = bounds(&app.cloud_prototype.groups.0[1]);
    let a = first.min + Vec2::new(100.0, 300.0);
    let b = second.min + Vec2::new(100.0, 300.0);
    assert_eq!(app.cloud_runtime_under_pointer(&ctx, a), Some(901));
    assert_eq!(app.cloud_runtime_under_pointer(&ctx, b), Some(902));
    let visible_in_second = |output: &egui::FullOutput| {
        output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Deploy cloud" => {
                let center = text.pos + text.galley.size() * 0.5;
                second.contains(center) && shape.clip_rect.contains(center)
            }
            _ => false,
        })
    };
    let baseline = frame(&ctx, &mut app, 0.9, b, 0.0);
    assert!(!visible_in_second(&baseline));
    let pan = app.canvas_view.pan_offset;
    phased_frame(&ctx, &mut app, 1.0, a, 0.0, TouchPhase::Start);
    frame(&ctx, &mut app, 1.016, b, -4000.0);
    let drifted = frame(&ctx, &mut app, 1.032, b, 0.0);
    assert!(!visible_in_second(&drifted), "card B must not receive card A's contact");
    assert!((Vec2::from(app.canvas_view.pan_offset) - Vec2::from(pan)).length() < 0.001);
    phased_frame(&ctx, &mut app, 1.048, b, 0.0, TouchPhase::Start);
    frame(&ctx, &mut app, 1.064, b, -4000.0);
    let fresh = frame(&ctx, &mut app, 1.080, b, 0.0);
    assert!(visible_in_second(&fresh), "card B receives its own new contact");
}
