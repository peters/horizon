//! Corner handle that resizes a cloud frame on the canvas.
use egui::{Id, Order, Pos2, Rect, Sense, Vec2};
use horizon_core::WorkspaceId;

use super::super::HorizonApp;
use crate::app::view::canvas_scene_transform;

impl HorizonApp {
    pub(in crate::app) fn render_cloud_resize_handles(&mut self, ctx: &egui::Context) {
        if !self.cloud_prototype.ready || ctx.viewport_id() != egui::ViewportId::ROOT {
            return;
        }
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let clip = transform.inverse() * canvas;
        let fullscreen = self.cloud_prototype.fullscreen.as_ref().map(|view| view.id);
        let interactive = !self.canvas_pan_input_claimed && !self.host_dialog_open();
        // Keep the grip about one panel-handle wide on screen. Canvas zoom would
        // otherwise shrink an 18-point corner until it could not be grabbed.
        let zoom = self.canvas_view.zoom.max(0.05);
        let count = self.cloud_prototype.groups.0.len();
        let mut commit = None;
        let mut changed = false;
        for index in 0..count {
            let Some((issue, size, corner)) = (|| {
                let group = &self.cloud_prototype.groups.0[index];
                if group.collapsed || fullscreen.is_some_and(|id| id != group.issue) {
                    return None;
                }
                let (_, max) = group.bounds();
                let handle = corner_span(group.size, zoom);
                let corner = Rect::from_min_size(Pos2::new(max[0] - handle, max[1] - handle), Vec2::splat(handle));
                (transform * corner)
                    .intersects(canvas)
                    .then_some((group.issue, group.size, corner))
            })() else {
                continue;
            };
            let response = egui::Area::new(Id::new(("cloud-resize", issue)))
                .order(Order::Foreground)
                .fixed_pos(corner.min)
                .constrain(false)
                .show(ctx, |ui| {
                    self.apply_canvas_layer_transform(ui, canvas);
                    ui.set_clip_rect(clip);
                    let (local, _) = ui.allocate_exact_size(corner.size(), Sense::hover());
                    let response = ui.interact(
                        local,
                        ui.id().with("resize"),
                        if interactive {
                            Sense::click_and_drag()
                        } else {
                            Sense::hover()
                        },
                    );
                    let active = interactive && (response.hovered() || response.dragged());
                    crate::app::panels::paint_grip(ui, local, active, zoom);
                    if response.hovered() || response.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                    }
                    response.on_hover_text("Drag to resize this cloud.")
                })
                .inner;
            if interactive && response.dragged() {
                let delta = response.drag_delta();
                if delta != Vec2::ZERO {
                    let scope = self.workspace_collision_scope(None);
                    if self.resize_cloud_frame(issue, [size[0] + delta.x, size[1] + delta.y], &scope) {
                        changed = true;
                    }
                }
            }
            if interactive && response.drag_stopped() {
                commit = Some(issue);
            }
        }
        if let Some(issue) = commit {
            self.commit_cloud_member_terminals(ctx, issue);
            changed = true;
        }
        if changed {
            self.save_cloud_prototype();
        }
    }

    /// Resize one cloud. `size` is the frame size in canvas points.
    pub(super) fn resize_cloud_frame(
        &mut self,
        issue: u32,
        size: [f32; 2],
        workspace_collision_ids: &[WorkspaceId],
    ) -> bool {
        let Some(workspace_local) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == issue)
            .map(|group| group.workspace.clone())
        else {
            return false;
        };
        let workspace = self.board.workspace_id_by_local_id(&workspace_local);
        let live = self.cloud_state_is_live();
        let mut before = None;
        if live {
            std::mem::swap(&mut self.board.cloud_groups, &mut self.cloud_prototype.groups);
            before = workspace.and_then(|id| self.board.workspace_frame_rect(id));
            std::mem::swap(&mut self.board.cloud_groups, &mut self.cloud_prototype.groups);
        }
        let resized =
            self.cloud_prototype
                .groups
                .resize_frame(&mut self.board, issue, size, super::super::PANEL_MIN_SIZE);
        if resized
            && live
            && let Some(workspace) = workspace
        {
            self.board.cloud_groups.clone_from(&self.cloud_prototype.groups);
            self.board
                .resolve_workspace_frame_growth_in_scope(workspace, before, workspace_collision_ids);
            self.cloud_prototype.groups.clone_from(&self.board.cloud_groups);
        }
        resized
    }

    fn commit_cloud_member_terminals(&mut self, ctx: &egui::Context, issue: u32) {
        let Some(locals) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == issue)
            .map(|group| group.panels.clone())
        else {
            return;
        };
        for local in locals {
            let Some(id) = self.board.panel_id_by_local_id(&local) else {
                continue;
            };
            let Some(size) = self
                .board
                .panel(id)
                .filter(|panel| panel.visible)
                .map(|panel| panel.layout.size)
            else {
                continue;
            };
            // Match `PanelFrame`: title bar, then padding on every side of the body.
            let body = Vec2::new(
                (size[0] - 2.0 * super::super::PANEL_PADDING).max(1.0),
                (size[1] - super::super::PANEL_TITLEBAR_HEIGHT - 2.0 * super::super::PANEL_PADDING).max(1.0),
            );
            let viewport = crate::terminal_widget::viewport_for_available_space(ctx, body);
            if let Some(panel) = self.board.panel_mut(id) {
                panel.resize_immediately(viewport.rows, viewport.cols, viewport.cell_width, viewport.cell_height);
            }
        }
        ctx.request_repaint();
    }
}

/// Canvas size of the corner. It is one panel handle wide on screen, and it
/// may use the whole frame when that frame is smaller than the grip.
fn corner_span(frame: [f32; 2], zoom: f32) -> f32 {
    let extent = frame[0].min(frame[1]).max(1.0);
    (super::super::RESIZE_HANDLE_SIZE / zoom.max(0.05)).min(extent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_panel::CloudGroup;
    use horizon_core::{PanelKind, PanelOptions};

    #[test]
    fn resize_handle_is_present_for_an_expanded_cloud_only() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("fixture");
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut collapsed = CloudGroup::new(
            102,
            "Collapsed".into(),
            local.clone(),
            temp.path().into(),
            [900.0, 80.0],
        );
        collapsed.set_collapsed(&mut app.board, true);
        app.cloud_prototype.groups.0.push(CloudGroup::new(
            101,
            "Open".into(),
            local,
            temp.path().into(),
            [40.0, 80.0],
        ));
        app.cloud_prototype.groups.0.push(collapsed);
        app.cloud_prototype.ready = true;
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            ..Default::default()
        });
        app.render_cloud_resize_handles(&ctx);
        assert!(
            ctx.memory(|memory| memory.area_rect(Id::new(("cloud-resize", 101u32))))
                .is_some()
        );
        assert!(
            ctx.memory(|memory| memory.area_rect(Id::new(("cloud-resize", 102u32))))
                .is_none()
        );
    }

    #[test]
    fn resizing_the_frame_grows_members_and_pushes_the_neighbour_workspace() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace_at("Cloud fixture", [0.0, 0.0]);
        let member = app
            .board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap();
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(101, "Fixture".into(), local, "/fixture".into(), [60.0, 120.0]);
        group.attach(&mut app.board, member);
        app.board.cloud_groups.0.push(group);
        let ctx = egui::Context::default();
        app.prepare_cloud_prototype(&ctx);
        let before = app.board.panel(member).unwrap().layout.size;
        let cloud_right = app.cloud_prototype.groups.0[0].overview_bounds().1[0];
        let neighbour = app.board.create_workspace_at("Neighbour", [cloud_right + 40.0, 0.0]);
        app.board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    position: Some([cloud_right + 80.0, 140.0]),
                    size: Some([420.0, 300.0]),
                    ..PanelOptions::default()
                },
                neighbour,
            )
            .unwrap();
        let frame = app.cloud_prototype.groups.0[0].size;
        assert!(app.resize_cloud_frame(101, [frame[0] + 700.0, frame[1] + 160.0], &[workspace, neighbour]));
        let grown = app.board.panel(member).unwrap().layout.size;
        assert!(grown[0] > before[0] + 400.0, "member width {grown:?} from {before:?}");
        assert!(grown[1] > before[1] + 80.0, "member height {grown:?} from {before:?}");
        let (min, max) = app.cloud_prototype.groups.0[0].overview_bounds();
        let cloud = egui::Rect::from_min_max(Pos2::from(min), Pos2::from(max));
        let neighbour_frame = app.board.workspace_frame_rect(neighbour).unwrap();
        let neighbour_frame = egui::Rect::from_min_max(
            egui::pos2(neighbour_frame[0], neighbour_frame[1]),
            egui::pos2(neighbour_frame[2], neighbour_frame[3]),
        );
        assert!(
            !cloud.intersects(neighbour_frame),
            "neighbour {neighbour_frame:?} overlaps cloud {cloud:?}"
        );
    }

    #[test]
    fn a_deployed_cloud_in_a_preset_resizes_with_its_slot_and_follows_board_layouts() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace_at("Cloud fixture", [0.0, 0.0]);
        let neighbour_panel = app
            .board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap();
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(101, "Fixture".into(), local, "/fixture".into(), [2000.0, 120.0]);
        let config = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
        )
        .unwrap();
        group.environment.id = "slot-ui-fixture".into();
        group.remote = Some(horizon_core::cloud_panel::CloudLaunch {
            deployment_started: true,
            id: "slot-ui-fixture".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: config.profiles["dev"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        });
        app.board.cloud_groups.0.push(group);
        app.board
            .arrange_workspace(workspace, horizon_core::WorkspaceLayout::Grid);
        let ctx = egui::Context::default();
        app.prepare_cloud_prototype(&ctx);
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05;
        assert!(app.board.cloud_takes_slot(&app.cloud_prototype.groups.0[0]));
        let slot = app.board.panel(neighbour_panel).unwrap().layout.size;
        assert!(near(app.cloud_prototype.groups.0[0].size, slot));

        let size = [slot[0] + 240.0, slot[1] + 120.0];
        assert!(app.resize_cloud_frame(101, size, &[workspace]));
        assert!(near(app.board.panel(neighbour_panel).unwrap().layout.size, size));
        assert!(near(app.cloud_prototype.groups.0[0].size, size));

        // A layout chosen on the board alone moves the cloud, and the UI copy follows.
        app.board
            .arrange_workspace(workspace, horizon_core::WorkspaceLayout::Rows);
        app.prepare_cloud_prototype(&ctx);
        let placed = app
            .board
            .cloud_groups
            .0
            .iter()
            .find(|group| group.issue == 101)
            .unwrap()
            .position;
        assert!(near(app.cloud_prototype.groups.0[0].position, placed));
        let panel = app.board.panel(neighbour_panel).unwrap().layout;
        assert!((placed[0] - panel.position[0]).abs() < 0.05, "Rows stack the slots");
        assert!(placed[1] > panel.position[1] + panel.size[1]);
    }

    #[test]
    fn dragging_a_slotted_cloud_by_its_header_swaps_it_with_a_panel_at_any_zoom() {
        for zoom in [1.0_f32, 0.62] {
            let (_temp, mut app) = crate::app::test_support::test_app();
            let workspace = app.board.create_workspace_at("Cloud fixture", [0.0, 0.0]);
            let neighbour_panel = app
                .board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        ..PanelOptions::default()
                    },
                    workspace,
                )
                .unwrap();
            let local = app.board.workspace(workspace).unwrap().local_id.clone();
            let mut group = CloudGroup::new(101, "Fixture".into(), local, "/fixture".into(), [2000.0, 120.0]);
            let config = horizon_core::cloud_panel::CloudConfig::parse(
                "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
            )
            .unwrap();
            group.environment.id = "slot-drag-fixture".into();
            group.remote = Some(horizon_core::cloud_panel::CloudLaunch {
                deployment_started: true,
                id: "slot-drag-fixture".into(),
                revision: "a".repeat(40),
                profile_name: "dev".into(),
                profile: config.profiles["dev"].clone(),
                placement: horizon_core::cloud_panel::Placement::default(),
            });
            app.board.cloud_groups.0.push(group);
            app.board
                .arrange_workspace(workspace, horizon_core::WorkspaceLayout::Grid);
            let ctx = egui::Context::default();
            app.prepare_cloud_prototype(&ctx);
            app.cloud_prototype.ready = true;
            app.canvas_view = horizon_core::CanvasViewState::new([0.0, 0.0], zoom);
            let panel_slot = app.board.panel(neighbour_panel).unwrap().layout;
            let cloud_slot = app.cloud_prototype.groups.0[0].position;
            let canvas = app.canvas_rect(&ctx);
            let transform = crate::app::view::canvas_scene_transform(canvas, app.canvas_view);
            let header = transform * egui::pos2(cloud_slot[0] + 200.0, cloud_slot[1] + 40.0);
            let target = transform
                * egui::pos2(
                    panel_slot.position[0] + panel_slot.size[0] * 0.5,
                    panel_slot.position[1] + panel_slot.size[1] * 0.5,
                );
            let mut time = 0.0;
            let mut frame = |position: egui::Pos2, events: Vec<egui::Event>| {
                time += 0.05;
                let _ = ctx
                    .run_ui(
                        crate::app::cloud_panel::scroll_bar_tests::input(
                            egui::vec2(1600.0, 1000.0),
                            time,
                            position,
                            events,
                        ),
                        |ui| app.render_active_view(ui, false),
                    )
                    .discard_textures();
            };
            for _ in 0..3 {
                frame(header, Vec::new());
            }
            let press = |pressed| egui::Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(header, vec![press(true)]);
            for step in 1..=12 {
                let t = step as f32 / 12.0;
                frame(header + (target - header) * t, Vec::new());
            }
            frame(
                target,
                vec![egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05;
            assert!(
                near(app.cloud_prototype.groups.0[0].position, panel_slot.position),
                "zoom {zoom}: cloud at {:?}, panel slot {:?}",
                app.cloud_prototype.groups.0[0].position,
                panel_slot.position
            );
            assert!(near(
                app.board.panel(neighbour_panel).unwrap().layout.position,
                cloud_slot
            ));
        }
    }

    #[test]
    fn zoomed_corner_drag_resizes_by_the_canvas_delta_without_panning() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("fixture");
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        app.cloud_prototype.groups.0.push(CloudGroup::new(
            101,
            "Open".into(),
            local,
            temp.path().into(),
            [80.0, 120.0],
        ));
        app.cloud_prototype.ready = true;
        app.canvas_view.zoom = 0.5;
        app.canvas_view.pan_offset = [0.0, 0.0];
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let mut step = 0.0_f64;
        let mut frame = |app: &mut crate::app::HorizonApp, events: Vec<egui::Event>| {
            step += 1.0;
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(step),
                events,
                ..Default::default()
            };
            input.viewport_id = egui::ViewportId::ROOT;
            ctx.begin_pass(input);
            let canvas = app.canvas_rect(&ctx);
            app.render_cloud_resize_handles(&ctx);
            let _ = ctx.end_pass().discard_textures();
            canvas
        };
        let canvas = frame(&mut app, Vec::new());
        let transform = crate::app::view::canvas_scene_transform(canvas, app.canvas_view);
        // The first resize reconciles the cloud onto its workspace. Later drags
        // must not move it again.
        let initial = app.cloud_prototype.groups.0[0].size;
        assert!(app.resize_cloud_frame(101, initial, &[]));
        let (before, origin, press) = {
            let group = &app.cloud_prototype.groups.0[0];
            let (_, max) = group.bounds();
            let handle = super::corner_span(group.size, app.canvas_view.zoom);
            let corner =
                egui::Rect::from_min_size(egui::pos2(max[0] - handle, max[1] - handle), egui::vec2(handle, handle));
            (group.size, group.position, (transform * corner).center())
        };
        let screen_delta = egui::vec2(24.0, 10.0);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(&mut app, vec![egui::Event::PointerMoved(press)]);
        frame(&mut app, vec![button(press, true)]);
        frame(&mut app, vec![egui::Event::PointerMoved(press + screen_delta)]);
        let dragged = app.cloud_prototype.groups.0[0].size;
        let canvas_delta = screen_delta / app.canvas_view.zoom;
        assert!(
            (dragged[0] - before[0] - canvas_delta.x).abs() < 0.5
                && (dragged[1] - before[1] - canvas_delta.y).abs() < 0.5,
            "frame {before:?} -> {dragged:?}, expected canvas delta {canvas_delta:?}"
        );
        assert_eq!(
            app.cloud_prototype.groups.0[0].position.map(f32::to_bits),
            origin.map(f32::to_bits)
        );
        assert!(!app.canvas_pan_input_claimed);
        frame(&mut app, vec![button(press + screen_delta, false)]);
        assert_eq!(
            app.cloud_prototype.groups.0[0].size.map(f32::to_bits),
            dragged.map(f32::to_bits)
        );
        assert_eq!(
            app.cloud_prototype.groups.0[0].position.map(f32::to_bits),
            origin.map(f32::to_bits)
        );
    }

    #[test]
    fn minimum_zoom_grip_uses_the_whole_minimum_frame() {
        let frame = [348.0, 318.0];
        let zoom = 0.05;
        let span = super::corner_span(frame, zoom);
        let screen = span * zoom;
        assert!(span <= frame[0].min(frame[1]));
        assert!(
            (screen - frame[1] * zoom).abs() < 0.05,
            "screen grip {screen} should use the 318-point frame"
        );
        assert!((super::corner_span([1088.0, 598.0], 1.0) - super::super::super::RESIZE_HANDLE_SIZE).abs() < 0.05);
    }
}
