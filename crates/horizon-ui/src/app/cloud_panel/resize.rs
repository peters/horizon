//! Corner handle that resizes a cloud frame on the canvas.
use egui::{Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::WorkspaceId;

use super::super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::theme;

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
                    paint_corner(ui, local);
                    let response = ui.interact(
                        local,
                        ui.id().with("resize"),
                        if interactive {
                            Sense::click_and_drag()
                        } else {
                            Sense::hover()
                        },
                    );
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

fn paint_corner(ui: &egui::Ui, rect: Rect) {
    let weight = rect.width().min(rect.height()) * 0.08;
    let stroke = Stroke::new(weight, theme::alpha(theme::FG_DIM(), 190));
    let inset = rect.width().min(rect.height()) * 0.12;
    let span = rect.width().min(rect.height()) * 0.62;
    let corner = rect.right_bottom() - Vec2::splat(inset);
    ui.painter()
        .line_segment([corner, corner - Vec2::new(span, 0.0)], stroke);
    ui.painter()
        .line_segment([corner, corner - Vec2::new(0.0, span)], stroke);
    let inner = span * 0.55;
    ui.painter().line_segment(
        [
            corner - Vec2::splat(inner * 0.45),
            corner - Vec2::new(inner, inner * 0.45),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            corner - Vec2::splat(inner * 0.45),
            corner - Vec2::new(inner * 0.45, inner),
        ],
        stroke,
    );
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
