use super::*;
use crate::app::cloud_panel::production::rebuild::tests::{Phase, deployment};
use crate::app::cloud_panel::scroll_bar_tests::{assert_press_reaches_lower_control, input};
use crate::app::test_support::test_app_with_startup;
use egui::{Context, Event, LayerId};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

const ID: u32 = 901;

#[test]
fn management_wheel_scrolls_with_the_complete_canvas_render_path() {
    let (temp, ctx, mut app) = ready_card(1.0);
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.cloud_prototype.root = Some(temp.path().into());
    let workspace = app.board.create_workspace("Cloud fixture");
    app.cloud_prototype.groups.0[0].workspace = app.board.workspace(workspace).unwrap().local_id.clone();
    let mut time = 0.0;
    let mut render = |position, events| {
        time += 0.05;
        ctx.run_ui(input(Vec2::new(1600.0, 1000.0), time, position, events), |ui| {
            app.render_active_view(ui, false);
        })
        .discard_textures()
    };
    for _ in 0..4 {
        render(Pos2::ZERO, Vec::new());
    }
    let center = ctx
        .memory(|memory| memory.area_rect(Id::new(("cloud-drawer", ID))))
        .unwrap()
        .center();
    let before = render(center, Vec::new());
    let marker = |output: &egui::FullOutput| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Workspace" => Some(text.pos.y),
            _ => None,
        })
    };
    let first = marker(&before).unwrap();
    render(
        center,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -7.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let mut after = render(center, Vec::new());
    for _ in 0..10 {
        after = render(center, Vec::new());
    }
    assert!(
        marker(&after).is_none_or(|last| last < first - 10.0),
        "wheel moves the Manage tab's content"
    );
}

fn frame(ctx: &Context, app: &mut HorizonApp, time: f64, position: Pos2, events: Vec<Event>) {
    let _ = ctx
        .run_ui(input(Vec2::new(3000.0, 2200.0), time, position, events), |ui| {
            app.handle_canvas_pan(ui.ctx());
            app.render_production_runtimes(ui.ctx());
        })
        .discard_textures();
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
    runtime.drawer = Some(super::super::Tab::Manage);
    // The long confirmation keeps Manage taller than the drawer at every zoom.
    runtime.confirmation = super::super::super::Confirmation::Delete;
    (temp, ctx, app)
}

#[test]
fn entering_the_card_scroll_area_keeps_lower_controls_under_the_pointer() {
    for zoom in [1.0_f32, 1.797] {
        let (_temp, ctx, mut app) = ready_card(zoom);
        let layer = LayerId::new(Order::Foreground, Id::new(("cloud-drawer", ID)));
        let mut time = 0.0;
        for _ in 0..3 {
            time += 0.05;
            frame(&ctx, &mut app, time, Pos2::new(2900.0, 2100.0), Vec::new());
        }
        let body = ctx
            .memory(|memory| memory.area_rect(layer.id))
            .expect("drawer")
            .center();
        assert_press_reaches_lower_control(
            &ctx,
            layer,
            body,
            Pos2::new(2900.0, 2100.0),
            &format!("runtime card at zoom {zoom}"),
            |position, events| {
                time += 0.05;
                frame(&ctx, &mut app, time, position, events);
            },
        );
    }
}
