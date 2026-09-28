use super::*;
use crate::app::cloud_panel::production::rebuild::tests::{Phase, deployment};
use crate::app::cloud_panel::scroll_bar_tests::{assert_press_reaches_lower_control, input};
use crate::app::test_support::test_app_with_startup;
use egui::{Context, Event, LayerId};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

const ID: u32 = 901;

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
    (temp, ctx, app)
}

#[test]
fn entering_the_card_scroll_area_keeps_lower_controls_under_the_pointer() {
    for zoom in [1.0_f32, 1.797] {
        let (_temp, ctx, mut app) = ready_card(zoom);
        let (min, _) = app.cloud_prototype.groups.0[0].runtime_bounds();
        let layer = LayerId::new(Order::Middle, Id::new(("cloud-runtime", ID)));
        let mut time = 0.0;
        assert_press_reaches_lower_control(
            &ctx,
            layer,
            Pos2::from(min) + Vec2::new(100.0, 300.0),
            Pos2::new(2900.0, 2100.0),
            &format!("runtime card at zoom {zoom}"),
            |position, events| {
                time += 0.05;
                frame(&ctx, &mut app, time, position, events);
            },
        );
    }
}
