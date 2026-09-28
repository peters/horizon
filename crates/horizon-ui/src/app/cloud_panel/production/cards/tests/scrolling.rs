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
fn toolbar_controls_stay_visible_without_scrolling_or_panning_at_scaled_zoom() {
    for zoom in [1.0, 1.797] {
        let (_temp, ctx, mut app) = verbose_card();
        app.cloud_prototype
            .production
            .runtimes
            .entry(901)
            .or_default()
            .detail_view = toolbar::View::Closed;
        app.canvas_view = CanvasViewState::new([0.0, 0.0], zoom);
        for step in 0..3 {
            frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
        }
        let transform = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view);
        let (min, _) = app.cloud_prototype.groups.0[0].runtime_bounds();
        let point = transform * (Pos2::from(min) + Vec2::new(100.0, 100.0));
        assert!(app.pointer_over_cloud_runtime(&ctx, point));
        let before = frame(&ctx, &mut app, 0.5, point, 0.0);
        let control = label_pos(&before, "▸  Show controls").expect("disclosure remains visible");
        let pan = app.canvas_view.pan_offset;
        phased_frame(&ctx, &mut app, 0.6, point, -800.0, TouchPhase::Start);
        let after = frame(&ctx, &mut app, 0.7, point, 0.0);
        assert_eq!(label_pos(&after, "▸  Show controls"), Some(control));
        assert_eq!(app.canvas_view.pan_offset.map(f32::to_bits), pan.map(f32::to_bits));
    }
}

fn label_pos(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    label_center(output, label).and_then(|(center, clip)| clip.contains(center).then_some(center))
}

fn label_center(output: &egui::FullOutput, label: &str) -> Option<(Pos2, egui::Rect)> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => {
            Some((text.pos + text.galley.size() * 0.5, shape.clip_rect))
        }
        _ => None,
    })
}

#[test]
fn moving_a_scroll_contact_between_toolbars_does_not_move_either_cloud() {
    let (_temp, ctx, mut app) = verbose_card();
    app.cloud_prototype
        .production
        .runtimes
        .entry(901)
        .or_default()
        .detail_view = toolbar::View::Closed;
    let mut second = app.cloud_prototype.groups.0[0].clone();
    second.issue = 902;
    second.position[0] += 900.0;
    app.cloud_prototype.groups.0.push(second);
    for step in 0..3 {
        frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    let transform = canvas_scene_transform(app.canvas_rect(&ctx), app.canvas_view);
    let points: Vec<_> = app
        .cloud_prototype
        .groups
        .0
        .iter()
        .map(|group| transform * (Pos2::from(group.runtime_bounds().0) + Vec2::new(100.0, 100.0)))
        .collect();
    assert_eq!(app.cloud_runtime_under_pointer(&ctx, points[0]), Some(901));
    assert_eq!(app.cloud_runtime_under_pointer(&ctx, points[1]), Some(902));
    let pan = app.canvas_view.pan_offset;
    phased_frame(&ctx, &mut app, 1.0, points[0], 0.0, TouchPhase::Start);
    frame(&ctx, &mut app, 1.016, points[1], -4000.0);
    assert_eq!(app.canvas_view.pan_offset.map(f32::to_bits), pan.map(f32::to_bits));
    phased_frame(&ctx, &mut app, 1.048, points[1], 0.0, TouchPhase::Start);
    frame(&ctx, &mut app, 1.064, points[1], -4000.0);
    assert_eq!(app.canvas_view.pan_offset.map(f32::to_bits), pan.map(f32::to_bits));
}

fn click_frame(ctx: &Context, app: &mut HorizonApp, time: f64, position: Pos2, pressed: bool) -> egui::FullOutput {
    ctx.run_ui(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(3000.0, 2200.0))),
            time: Some(time),
            events: vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
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

fn verbose_card() -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.cloud_prototype.ready = true;
    app.canvas_view = CanvasViewState::new([0.0, 0.0], 1.0);
    let profile = super::super::super::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
    .clone();
    let mut group = CloudGroup::new(
        901,
        "Scroll fixture".into(),
        "workspace".into(),
        "/synthetic".into(),
        [10.0, 10.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "scroll-fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype
        .production
        .runtimes
        .entry(901)
        .or_default()
        .detail_view = toolbar::View::Activity;
    (temp, ctx, app)
}

fn open_verbose(ctx: &Context, app: &mut HorizonApp) -> (Pos2, egui::FullOutput) {
    for step in 0..3 {
        frame(ctx, app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    let opened = frame(ctx, app, 0.2, Pos2::new(80.0, 400.0), 0.0);
    let header = label_pos(&opened, "Verbose output").expect("verbose header is visible");
    frame(ctx, app, 0.21, header, 0.0);
    click_frame(ctx, app, 0.22, header, true);
    let mut latest = click_frame(ctx, app, 0.24, header, false);
    let mut time = 0.24;
    for _ in 0..24 {
        time += 0.02;
        latest = frame(ctx, app, time, header, 0.0);
    }
    (header, latest)
}

#[test]
fn expanded_verbose_output_scrolls_back_to_its_first_line_and_the_heading() {
    let (_temp, ctx, mut app) = verbose_card();
    {
        let runtime = app.cloud_prototype.production.runtimes.entry(901).or_default();
        runtime.logs = (0..80).map(|index| format!("LOG-LINE-{index:03}")).collect();
    }
    let (header, opened) = open_verbose(&ctx, &mut app);
    assert!(
        label_pos(&opened, "Scroll fixture — Activity").is_some(),
        "expanding the log keeps the heading"
    );
    assert!(
        label_pos(&opened, "LOG-LINE-000").is_none(),
        "a long log opens on its latest line"
    );
    let mut time = 1.0;
    let mut latest = opened;
    let mut pointer = header;
    for _ in 0..4 {
        time += 0.2;
        frame(&ctx, &mut app, time, pointer, -400.0);
        time += 0.08;
        latest = frame(&ctx, &mut app, time, pointer, 0.0);
        if let Some(pos) = label_pos(&latest, "LOG-LINE-079") {
            pointer = pos;
        }
    }
    assert!(
        label_pos(&latest, "LOG-LINE-079").is_some(),
        "the open log shows its latest line"
    );
    for _ in 0..12 {
        time += 0.2;
        frame(&ctx, &mut app, time, pointer, 4000.0);
        time += 0.08;
        latest = frame(&ctx, &mut app, time, pointer, 0.0);
        if let Some(pos) = label_pos(&latest, "LOG-LINE-000") {
            pointer = pos;
        }
    }
    assert!(
        label_pos(&latest, "LOG-LINE-000").is_some(),
        "scrolling up through verbose output returns to its first line"
    );
    assert!(
        label_pos(&latest, "Scroll fixture — Activity").is_some(),
        "scrolling the log back up does not lose the card heading"
    );
}

#[test]
fn scrolled_up_verbose_output_keeps_its_first_line_while_more_lines_arrive() {
    let (_temp, ctx, mut app) = verbose_card();
    {
        let runtime = app.cloud_prototype.production.runtimes.entry(901).or_default();
        runtime.logs = (0..80).map(|index| format!("LOG-LINE-{index:03}")).collect();
    }
    let (header, opened) = open_verbose(&ctx, &mut app);
    let mut time = 1.0;
    let mut latest = opened;
    let mut pointer = header;
    for _ in 0..4 {
        time += 0.2;
        frame(&ctx, &mut app, time, pointer, -400.0);
        time += 0.08;
        latest = frame(&ctx, &mut app, time, pointer, 0.0);
        if let Some(pos) = label_pos(&latest, "LOG-LINE-079") {
            pointer = pos;
        }
    }
    for _ in 0..12 {
        time += 0.2;
        frame(&ctx, &mut app, time, pointer, 4000.0);
        time += 0.08;
        latest = frame(&ctx, &mut app, time, pointer, 0.0);
        if let Some(pos) = label_pos(&latest, "LOG-LINE-000") {
            pointer = pos;
        }
    }
    assert!(label_pos(&latest, "LOG-LINE-000").is_some());
    {
        let runtime = app.cloud_prototype.production.runtimes.get_mut(&901).unwrap();
        // More than the 150-line follow cap, so a trim of the visible log would drop the first line.
        for line in 0..100 {
            runtime.push_log(format!("BURST-{line}"));
        }
        assert_eq!(runtime.logs.front().map(String::as_str), Some("LOG-LINE-000"));
        assert_eq!(runtime.logs.len(), 80);
        assert_eq!(runtime.pending_logs.len(), 100);
    }
    time += 0.05;
    latest = frame(&ctx, &mut app, time, pointer, 0.0);
    assert!(
        label_pos(&latest, "LOG-LINE-000").is_some(),
        "new verbose lines must not pull a scrolled-up log back to the end or drop its first line"
    );
    assert!(label_pos(&latest, "Scroll fixture — Activity").is_some());
}
