use super::*;
use crate::app::test_support::{editor_workspace_state, raw_input, test_app_with_startup};
use egui::{Event, LayerId, Modifiers, MouseWheelUnit, Rect, Sense, TouchPhase};
use horizon_core::{Panel, PanelKind, PanelOptions, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

#[test]
fn runtime_overlap_respects_foreground_and_persisted_middle_panel_order() {
    for (focused, panel_on_top, overlaps) in [
        (true, false, true),
        (false, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        let (_temp, ctx, mut app) = overlap_fixture(focused, overlaps);
        let id = app.board.panels[0].id;
        let (runtime_min, runtime_max) = app.cloud_prototype.groups.0[0].runtime_bounds();
        let panel_layer = LayerId::new(
            if focused { Order::Foreground } else { Order::Middle },
            Id::new(("panel", id.0)),
        );
        let runtime_layer = LayerId::new(Order::Middle, Id::new(("cloud-runtime", 901_u32)));
        let mut pointer = Pos2::ZERO;
        for time in [0.0, 0.016, 0.032] {
            let mut input = raw_input([1400.0, 1000.0], None);
            input.time = Some(time);
            let _ = ctx
                .run_ui(input, |ui| {
                    let canvas = app.canvas_rect(ui.ctx());
                    let geometry = app.visible_panel_geometry_for_canvas_view(canvas, None)[0].1;
                    pointer = geometry.terminal_body_screen_rect.expect("body").center();
                    let transform = canvas_scene_transform(canvas, app.canvas_view);
                    let runtime_rect = transform * Rect::from_min_max(Pos2::from(runtime_min), Pos2::from(runtime_max));
                    for (layer, rect) in [(panel_layer, geometry.screen_rect), (runtime_layer, runtime_rect)] {
                        egui::Area::new(layer.id)
                            .order(layer.order)
                            .fixed_pos(rect.min)
                            .show(ui.ctx(), |ui| {
                                ui.allocate_exact_size(rect.size(), Sense::hover());
                            });
                    }
                    ui.ctx()
                        .move_to_top(if panel_on_top { panel_layer } else { runtime_layer });
                })
                .discard_textures();
        }
        let panel_wins = focused || panel_on_top || !overlaps;
        assert_eq!(
            ctx.layer_id_at(pointer),
            Some(if panel_wins { panel_layer } else { runtime_layer })
        );
        assert_eq!(app.pointer_over_cloud_runtime(&ctx, pointer), overlaps);
        let before = app.canvas_view.pan_offset;
        let mut input = raw_input([1400.0, 1000.0], None);
        input.time = Some(1.0);
        input.events = vec![
            Event::PointerMoved(pointer),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -5.0),
                phase: TouchPhase::Start,
                modifiers: Modifiers::NONE,
            },
        ];
        let _ = ctx
            .run_ui(input, |ui| app.handle_canvas_pan(ui.ctx()))
            .discard_textures();
        assert_eq!(
            app.canvas_pan_input_claimed, panel_wins,
            "focused={focused}, panel_on_top={panel_on_top}, overlaps={overlaps}"
        );
        let expected_pan = before[1] - if panel_wins { 5.0 } else { 0.0 };
        assert!((app.canvas_view.pan_offset[1] - expected_pan).abs() < f32::EPSILON);
    }
}

fn overlap_fixture(focused: bool, overlaps: bool) -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState {
            workspaces: vec![editor_workspace_state("synthetic", [0.0, 0.0])],
            ..RuntimeState::default()
        }),
    });
    app.cloud_prototype.ready = true;
    let mut config = horizon_core::cloud_panel::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .expect("profile");
    let mut group = CloudGroup::new(
        901,
        "Cloud fixture".into(),
        "synthetic".into(),
        "/synthetic".into(),
        [0.0, 0.0],
    );
    group.size = [300.0, 400.0];
    group.remote = Some(CloudLaunch {
        deployment_started: false,
        id: "overlap-fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles.remove("dev").expect("profile"),
    });
    let (runtime_min, _) = group.runtime_bounds();
    app.cloud_prototype.groups.0.push(group);
    std::fs::write(temp.path().join("overlap.bin"), b"").expect("snapshot");
    let id = app.board.panels[0].id;
    let mut panel = Panel::spawn(
        id,
        app.board.panels[0].workspace_id,
        PanelOptions {
            kind: PanelKind::Ssh,
            local_id: Some("overlap".into()),
            transcript_root: Some(temp.path().into()),
            restore_as_disconnected_snapshot: true,
            ..PanelOptions::default()
        },
    )
    .expect("snapshot terminal");
    panel.layout.position = if overlaps { runtime_min } else { [0.0, 0.0] };
    panel.layout.size = [250.0, 300.0];
    assert_eq!(panel.terminal().expect("terminal").scrollback(), 0);
    app.board.panels = vec![panel];
    app.board.focused = focused.then_some(id);
    (temp, ctx, app)
}
