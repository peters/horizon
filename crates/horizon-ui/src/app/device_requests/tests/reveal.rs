use super::*;

#[test]
fn root_reveal_waits_for_smaller_restored_window() {
    let (_temp, ctx, mut app) = app();
    let create = request(
        &app,
        Operation::Create {
            identity: None,
            endpoint: "127.0.0.1:5900".into(),
        },
    );
    let initial = one(app.apply_device_request(&create, &ctx));
    let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
    app.board.panel_mut(id).unwrap().layout.position = [100_000.0, 5000.0];
    let before = app.canvas_view;
    app.panel_render_caches.pending_device_reveal = Some(PendingDeviceReveal {
        id,
        restored_fullscreen: Some(WindowRestore {
            fullscreen: false,
            size: egui::vec2(800.0, 600.0),
        }),
        deadline: Instant::now() + Duration::from_secs(2),
    });
    for (fullscreen, dimensions) in [
        (true, [3000.0, 1800.0]),
        (false, [3000.0, 1800.0]),
        (false, [800.0, 600.0]),
    ] {
        let mut input = crate::app::test_support::raw_input(dimensions, None);
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().fullscreen = Some(fullscreen);
        let mut output = ctx.run_ui(input, |ui| {
            app.apply_pending_root_device_reveal(ui.ctx());
            if dimensions[0] > 800.0 {
                assert_eq!(app.canvas_view, before);
                assert!(app.panel_render_caches.pending_device_reveal.is_some());
            } else {
                let canvas = app.canvas_rect(ui.ctx());
                let panel = app.board.panel(id).unwrap();
                let rect = egui::Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
                assert!(canvas.contains_rect(crate::app::view::canvas_scene_transform(canvas, app.canvas_view) * rect));
                assert!(app.panel_render_caches.pending_device_reveal.is_none());
            }
        });
        output.textures_delta.clear();
    }
}

#[cfg(feature = "cloud-workspaces")]
#[test]
fn root_reveal_leaves_cloud_fullscreen_and_survives_later_frames() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input};
    use horizon_core::cloud_panel::CloudGroup;
    for belongs in [false, true] {
        let (temp, ctx, mut app) = app();
        app.root_viewport_stabilizer = None;
        let create = request(
            &app,
            Operation::Create {
                identity: None,
                endpoint: "127.0.0.1:5900".into(),
            },
        );
        let initial = one(app.apply_device_request(&create, &ctx));
        let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
        let caller = app.board.panels[0].id;
        let workspace = app.board.panels[0].workspace_id;
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut cloud = CloudGroup::new(1, "Fixture".into(), local, temp.path().into(), [0.0, 0.0]);
        cloud.panels.push(app.board.panel(caller).unwrap().local_id.clone());
        if belongs {
            cloud.panels.push(initial.panel_id.clone());
        }
        app.cloud_prototype.groups.0.push(cloud);
        app.board.cloud_groups = app.cloud_prototype.groups.clone();
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
        app.toggle_cloud_fullscreen(&ctx, 1);
        assert!(app.cloud_prototype.fullscreen.is_some());
        if belongs {
            app.cloud_prototype.groups.0[0].layout = Some(horizon_core::WorkspaceLayout::Columns);
            let panel = app.board.panel_mut(id).unwrap();
            panel.visible = false;
            panel.layout.position = [100_000.0, 5000.0];
        }
        app.board.focus(caller);
        let active = app.board.active_workspace;
        let reveal = request(
            &app,
            Operation::Reveal {
                panel_id: initial.panel_id,
            },
        );
        let observed = one(app.apply_device_request(&reveal, &ctx));
        assert!(app.cloud_prototype.fullscreen.is_none());
        assert_eq!(app.board.focused, Some(caller));
        assert_eq!(app.board.active_workspace, active);
        assert_eq!(
            observed.diagnostics.unwrap().connection_generation,
            initial.diagnostics.unwrap().connection_generation
        );
        for _ in 0..2 {
            run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
            assert!(app.panel_render_caches.device_ui_state[&id].was_rendered());
            assert!(app.cloud_prototype.fullscreen.is_none());
        }
    }
}

#[cfg(feature = "cloud-workspaces")]
#[test]
fn detached_reveal_preserves_root_fullscreen_and_uses_the_child_canvas() {
    use horizon_core::cloud_panel::CloudGroup;
    let (temp, ctx, mut app) = app();
    let create = request(
        &app,
        Operation::Create {
            identity: None,
            endpoint: "127.0.0.1:5900".into(),
        },
    );
    let initial = one(app.apply_device_request(&create, &ctx));
    let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
    let workspace = app.board.panels[0].workspace_id;
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    app.detach_workspace(workspace);
    let root_panel = app.board.panels[1].id;
    let root_workspace = app.board.panels[1].workspace_id;
    let mut cloud = CloudGroup::new(
        1,
        "Root fixture".into(),
        app.board.workspace(root_workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    cloud.panels.push(app.board.panel(root_panel).unwrap().local_id.clone());
    app.cloud_prototype.groups.0.push(cloud);
    app.toggle_cloud_fullscreen(&ctx, 1);
    app.board.focus(root_panel);
    app.board.panel_mut(id).unwrap().layout.position = [100_000.0, 5000.0];
    let root_view = app.canvas_view;
    let reveal = request(
        &app,
        Operation::Reveal {
            panel_id: initial.panel_id,
        },
    );
    one(app.apply_device_request(&reveal, &ctx));
    assert_eq!(app.canvas_view, root_view);
    assert!(app.cloud_prototype.fullscreen.is_some());
    let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 700.0));
    app.canvas_view = app.detached_workspaces[&local].canvas_view;
    app.apply_pending_device_reveal(&local, canvas);
    let panel = app.board.panel(id).unwrap();
    let rect = egui::Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
    assert!(canvas.contains_rect(crate::app::view::canvas_scene_transform(canvas, app.canvas_view) * rect));
    assert_eq!(app.board.focused, Some(root_panel));
    assert!(app.cloud_prototype.fullscreen.is_some());
}

#[test]
fn reveal_keeps_a_distant_viewer_on_screen_at_minimum_zoom() {
    let (_temp, ctx, mut app) = app();
    let create = request(
        &app,
        Operation::Create {
            identity: None,
            endpoint: "127.0.0.1:5900".into(),
        },
    );
    let initial = one(app.apply_device_request(&create, &ctx));
    let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
    app.board.panel_mut(id).unwrap().layout.position = [100_000.0, 5000.0];
    let focused = app.board.focused;
    let reveal = request(
        &app,
        Operation::Reveal {
            panel_id: initial.panel_id,
        },
    );
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            ..Default::default()
        },
        |ui| {
            let observed = one(app.apply_device_request(&reveal, ui.ctx()));
            app.apply_pending_root_device_reveal(ui.ctx());
            assert!(
                !observed.image.image_displayed,
                "acknowledgement is not proof of a painted image"
            );
            let canvas = app.canvas_rect(ui.ctx());
            let panel = app.board.panel(id).unwrap();
            let rect = egui::Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
            let displayed = crate::app::view::canvas_scene_transform(canvas, app.canvas_view) * rect;
            assert!(
                canvas.contains_rect(displayed),
                "reveal left the viewer outside the canvas: {displayed:?} versus {canvas:?}"
            );
        },
    );
    output.textures_delta.clear();
    assert_eq!(app.board.focused, focused);
}
