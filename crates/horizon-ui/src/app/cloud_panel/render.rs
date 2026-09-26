use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use horizon_core::cloud_panel::{CloudGroup, HEADER};

use super::super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::app::{RenameEditAction, panel_chrome::show_inline_rename_editor};
use crate::theme;

#[derive(Clone, Copy)]
enum Action {
    Rename(u32),
    Collapse(u32),
    Remove(u32),
}

impl HorizonApp {
    pub(in crate::app) fn cloud_runtime_under_pointer(&self, ctx: &egui::Context, position: Pos2) -> Option<u32> {
        if !self.cloud_prototype.ready || ctx.viewport_id() != egui::ViewportId::ROOT {
            return None;
        }
        let canvas = self.canvas_rect(ctx);
        if !canvas.contains(position) {
            return None;
        }
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let fixture_mode = std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some();
        let mut candidates = self.cloud_prototype.groups.0.iter().filter(|group| {
            if self
                .cloud_prototype
                .fullscreen
                .as_ref()
                .is_some_and(|view| view.id != group.issue)
                || if fixture_mode {
                    self.cloud_prototype.profiles.is_none()
                } else {
                    group.remote.is_none()
                }
            {
                return false;
            }
            let (min, max) = group.runtime_bounds();
            (transform * Rect::from_min_max(Pos2::from(min), Pos2::from(max))).contains(position)
        });
        let layer = ctx.layer_id_at(position);
        candidates
            .clone()
            .find(|group| layer.is_some_and(|layer| layer.id == Id::new(("cloud-runtime", group.issue))))
            .or_else(|| candidates.next_back())
            .map(|group| group.issue)
    }

    #[cfg(test)]
    pub(in crate::app) fn pointer_over_cloud_runtime(&self, ctx: &egui::Context, position: Pos2) -> bool {
        self.cloud_runtime_under_pointer(ctx, position).is_some()
    }

    pub(in crate::app) fn render_cloud_frames(&mut self, ctx: &egui::Context) {
        if !self.cloud_prototype.ready {
            return;
        }
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let clip = transform.inverse() * canvas;
        let mut action = None;
        let mut title_action = RenameEditAction::None;
        let mut moved = false;
        let now = std::time::SystemTime::now();
        for group in &mut self.cloud_prototype.groups.0 {
            if self
                .cloud_prototype
                .fullscreen
                .as_ref()
                .is_some_and(|view| view.id != group.issue)
            {
                continue;
            }
            let (min, max) = group.bounds();
            let rect = Rect::from_min_max(Pos2::from(min), Pos2::from(max));
            if !(transform * rect).intersects(canvas) {
                continue;
            }
            let accent = cloud_accent(group.issue);
            frame_background(ctx, group.issue, rect, transform, clip, accent);
            let editing = self.cloud_prototype.renaming == Some(group.issue);
            let cost = self
                .cloud_prototype
                .production
                .runtimes
                .get(&group.issue)
                .and_then(|runtime| runtime.cost_badge(now));
            let response = egui::Area::new(Id::new(("cloud-header", group.issue)))
                .order(Order::Middle)
                .fixed_pos(rect.min)
                .constrain(false)
                .show(ctx, |ui| {
                    ui.ctx().set_transform_layer(ui.layer_id(), transform);
                    ui.set_clip_rect(clip);
                    let (header, _) = ui.allocate_exact_size(Vec2::new(rect.width(), HEADER), Sense::hover());
                    let cost_width = paint_header(ui, group, header, accent, editing, cost);
                    if editing {
                        let field = Rect::from_min_size(
                            header.min + Vec2::new(64.0, 16.0),
                            Vec2::new(header.width() - 180.0 - cost_width, 32.0),
                        );
                        title_action = show_inline_rename_editor(
                            ui,
                            field,
                            &mut self.cloud_prototype.title_draft,
                            FontId::proportional(21.0),
                        );
                        if ui.input(|input| {
                            input.pointer.any_pressed()
                                && input
                                    .pointer
                                    .interact_pos()
                                    .is_some_and(|p| !(transform * field).contains(p))
                        }) {
                            title_action = RenameEditAction::Commit;
                        }
                    }
                    let drag = ui.interact(
                        header,
                        ui.id().with("drag"),
                        if editing {
                            Sense::hover()
                        } else {
                            Sense::click_and_drag()
                        },
                    );
                    cloud_context(&drag, group, &mut action);
                    drag.on_hover_text("Double-click to rename. Drag to move this cloud.")
                })
                .inner;
            if response.dragged() {
                let delta = response.drag_delta();
                group.translate(&mut self.board, [delta.x, delta.y]);
                moved = true;
            }
            if response.double_clicked() {
                action = Some(Action::Rename(group.issue));
            }
            if group.panels.is_empty() && !group.collapsed {
                empty_group(ctx, group, rect, transform, clip);
            }
        }
        if moved {
            self.save_cloud_prototype();
        }
        self.apply_cloud_title_edit(title_action);
        if let Some(action) = action {
            self.cloud_action(action, ctx);
        }
    }

    fn apply_cloud_title_edit(&mut self, title_action: RenameEditAction) {
        if title_action != RenameEditAction::None {
            if title_action == RenameEditAction::Commit
                && !self.cloud_prototype.title_draft.trim().is_empty()
                && let Some(group) = self
                    .cloud_prototype
                    .groups
                    .0
                    .iter_mut()
                    .find(|g| Some(g.issue) == self.cloud_prototype.renaming)
            {
                group.title = self.cloud_prototype.title_draft.trim().to_string();
            }
            self.cloud_prototype.renaming = None;
            self.save_cloud_prototype();
        }
    }

    fn cloud_action(&mut self, action: Action, ctx: &egui::Context) {
        let issue = match action {
            Action::Rename(i) | Action::Collapse(i) | Action::Remove(i) => i,
        };
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == issue) else {
            return;
        };
        let mut removed_from = None;
        match action {
            Action::Rename(_) => {
                self.cloud_prototype.renaming = Some(issue);
                self.cloud_prototype
                    .title_draft
                    .clone_from(&self.cloud_prototype.groups.0[index].title);
            }
            Action::Collapse(_) => {
                let group = &mut self.cloud_prototype.groups.0[index];
                group.set_collapsed(&mut self.board, !group.collapsed);
            }
            Action::Remove(_) => {
                if self.cloud_prototype.groups.0[index].panels.is_empty()
                    && self.cloud_prototype.groups.0[index].remote.is_none()
                {
                    removed_from = Some(self.cloud_prototype.groups.0.remove(index).workspace);
                }
            }
        }
        self.save_cloud_prototype();
        if let Some(workspace) = removed_from {
            self.release_removed_cloud_workspace(&workspace, ctx);
        }
    }

    pub(in crate::app) fn render_cloud_controls(&mut self, ctx: &egui::Context) {
        if self.cloud_prototype.root.is_none() {
            return;
        }
        if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some() {
            self.render_cloud_runtimes(ctx);
        } else {
            self.render_production_runtimes(ctx);
        }
        self.render_cloud_dialogs(ctx);
    }

    pub(in crate::app) fn render_cloud_dialogs(&mut self, ctx: &egui::Context) {
        self.render_cloud_creation(ctx);
        self.render_cloud_accounts(ctx);
        self.release_workspaces_after_creation(ctx);
    }
}

fn cloud_context(response: &egui::Response, group: &CloudGroup, action: &mut Option<Action>) {
    response.context_menu(|ui| {
        for (text, next) in [
            ("Rename", Action::Rename(group.issue)),
            (
                if group.collapsed { "Expand" } else { "Collapse" },
                Action::Collapse(group.issue),
            ),
        ] {
            if ui.button(text).clicked() {
                *action = Some(next);
                ui.close();
            }
        }
        ui.separator();
        if ui
            .add_enabled(
                group.panels.is_empty() && group.remote.is_none(),
                egui::Button::new("Remove empty cloud"),
            )
            .clicked()
        {
            *action = Some(Action::Remove(group.issue));
            ui.close();
        }
    });
}

fn cloud_accent(id: u32) -> Color32 {
    theme::workspace_accent(id.saturating_sub(101) as usize)
}

/// Returns the width the optional cost badge takes from the title.
fn paint_header(
    ui: &egui::Ui,
    group: &CloudGroup,
    rect: Rect,
    accent: Color32,
    editing: bool,
    cost: Option<String>,
) -> f32 {
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius {
            nw: 14,
            ne: 14,
            sw: 0,
            se: 0,
        },
        theme::blend(theme::PANEL_BG_ALT(), accent, 0.045),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, theme::alpha(accent, 45)),
    );
    let mark = rect.min + Vec2::new(35.0, 34.0);
    painter.rect_filled(
        Rect::from_center_size(mark, Vec2::splat(34.0)),
        10,
        theme::blend(theme::PANEL_BG(), accent, 0.15),
    );
    cloud_glyph(painter, mark, accent);
    let badge = Rect::from_min_size(rect.right_top() + Vec2::new(-110.0, 23.0), Vec2::new(90.0, 25.0));
    let fill = theme::blend(theme::PANEL_BG(), accent, 0.09);
    let reserved = cost.map_or(0.0, |cost| cost_badge(painter, badge, cost, fill));
    if !editing {
        let title_rect = Rect::from_min_size(
            rect.min + Vec2::new(64.0, 16.0),
            Vec2::new(rect.width() - 178.0 - reserved, 30.0),
        );
        painter.with_clip_rect(title_rect).text(
            title_rect.left_center(),
            Align2::LEFT_CENTER,
            &group.title,
            FontId::proportional(21.0),
            theme::FG(),
        );
    }
    let provider = group.environment.provider.as_deref().unwrap_or("Local");
    let provider = match provider {
        "runpod" => "RunPod",
        "daytona" => "Daytona",
        "fly" => "Fly.io",
        "hetzner" => "Hetzner",
        "local" => "Local",
        other => other,
    };
    let profile = group.environment.profile.as_deref().unwrap_or("Development");
    painter.text(
        rect.min + Vec2::new(64.0, 59.0),
        Align2::LEFT_CENTER,
        format!("{provider}  /  {profile}"),
        FontId::proportional(13.0),
        theme::FG_SOFT(),
    );
    painter.rect_filled(badge, 12, fill);
    painter.circle_filled(badge.left_center() + Vec2::new(12.0, 0.0), 3.0, accent);
    let label = match group.panels.len() {
        0 => "Empty".to_string(),
        1 => "1 panel".to_string(),
        count => format!("{count} panels"),
    };
    painter.text(
        badge.center() + Vec2::new(5.0, 0.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        theme::FG_SOFT(),
    );
    reserved
}

/// Paints the run and total cost left of the panel badge and returns the width it occupies.
fn cost_badge(painter: &egui::Painter, panels: Rect, cost: String, fill: Color32) -> f32 {
    const GAP: f32 = 8.0;
    let galley = painter.layout_no_wrap(cost, FontId::proportional(12.0), theme::FG_SOFT());
    let width = galley.size().x + 20.0;
    let badge = Rect::from_min_max(
        Pos2::new(panels.left() - GAP - width, panels.top()),
        Pos2::new(panels.left() - GAP, panels.bottom()),
    );
    painter.rect_filled(badge, 12, fill);
    painter.galley(badge.center() - galley.size() * 0.5, galley, theme::FG_SOFT());
    width + GAP
}

fn cloud_glyph(painter: &egui::Painter, center: Pos2, color: Color32) {
    // Overlapping filled lobes keep the small cloud mark legible at canvas zoom levels.
    painter.circle_filled(center + Vec2::new(-6.0, 1.0), 4.5, color);
    painter.circle_filled(center + Vec2::new(0.0, -3.0), 6.0, color);
    painter.circle_filled(center + Vec2::new(7.0, 1.0), 4.0, color);
    painter.rect_filled(
        Rect::from_min_size(center + Vec2::new(-6.0, 0.0), Vec2::new(13.0, 5.0)),
        2,
        color,
    );
}

fn empty_group(ctx: &egui::Context, group: &CloudGroup, rect: Rect, transform: egui::emath::TSTransform, clip: Rect) {
    egui::Area::new(Id::new(("cloud-empty", group.issue)))
        .order(Order::Middle)
        .fixed_pos(rect.min + Vec2::new(36.0, HEADER + 56.0))
        .constrain(false)
        .interactable(false)
        .show(ctx, |ui| {
            ui.ctx().set_transform_layer(ui.layer_id(), transform);
            ui.set_clip_rect(clip);
            ui.set_width(rect.width() - 72.0);
            ui.label(
                RichText::new("Your next task starts here")
                    .size(23.0)
                    .color(theme::FG_SOFT()),
            );
            ui.add_space(12.0);
            ui.label(
                RichText::new("Ctrl-double-click anywhere inside this cloud")
                    .size(15.0)
                    .color(theme::FG_SOFT()),
            );
            ui.label(
                RichText::new("to add an agent, browser or another panel.")
                    .size(15.0)
                    .color(theme::FG_DIM()),
            );
            ui.add_space(28.0);
            ui.label(
                RichText::new("One environment. All your tools.")
                    .size(13.0)
                    .color(theme::FG_DIM()),
            );
        });
}

fn frame_background(
    ctx: &egui::Context,
    issue: u32,
    rect: Rect,
    transform: egui::emath::TSTransform,
    clip: Rect,
    accent: Color32,
) {
    egui::Area::new(Id::new(("cloud-frame", issue)))
        .order(Order::Background)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(false)
        .show(ctx, |ui| {
            ui.ctx().set_transform_layer(ui.layer_id(), transform);
            ui.set_clip_rect(clip);
            ui.allocate_exact_size(rect.size(), Sense::hover());
            ui.painter().rect_filled(rect, 14, theme::PANEL_BG());
            ui.painter()
                .rect_stroke(rect, 14, Stroke::new(1.0, theme::alpha(accent, 85)), StrokeKind::Inside);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use crate::test_egui::DiscardTextures;

    /// Painted texts with their bounds and clip rectangles, and the width reserved for the cost badge.
    type Header = (Vec<(String, Rect, Rect)>, f32);

    fn title() -> String {
        "Synthetic cloud with a long title ".repeat(8)
    }

    fn header(cost: Option<&str>) -> Header {
        let group = CloudGroup::new(101, title(), "workspace".into(), "/synthetic".into(), [0.0, 0.0]);
        let mut reserved = f32::NAN;
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(900.0, HEADER), Sense::hover());
                reserved = paint_header(ui, &group, rect, cloud_accent(101), false, cost.map(str::to_owned));
            })
            .discard_textures();
        let texts = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.visual_bounding_rect(),
                    clipped.clip_rect,
                )),
                _ => None,
            })
            .collect();
        (texts, reserved)
    }

    fn find<'a>(texts: &'a [(String, Rect, Rect)], label: &str) -> &'a (String, Rect, Rect) {
        texts
            .iter()
            .find(|(text, ..)| text == label)
            .unwrap_or_else(|| panic!("{label} must be painted"))
    }

    #[test]
    fn cost_badge_sits_before_the_panel_badge_and_narrows_the_title() {
        let (plain, none) = header(None);
        assert!(none.abs() < f32::EPSILON);
        assert!(!plain.iter().any(|(text, ..)| text.starts_with('$')));
        let mut widths = Vec::new();
        for badge in ["$0.83 run", "$4.20 total", "$0.83 run · $4.20 total"] {
            let (costed, reserved) = header(Some(badge));
            let (_, cost, _) = find(&costed, badge);
            let (_, panels, _) = find(&costed, "Empty");
            assert!(cost.right() < panels.left(), "the cost precedes the panel count");
            let title_clip = |texts: &[(String, Rect, Rect)]| find(texts, &title()).2;
            assert!(
                title_clip(&costed).right() < cost.left(),
                "the title never runs under the cost"
            );
            assert!((title_clip(&plain).right() - title_clip(&costed).right() - reserved).abs() < 0.5);
            assert!(reserved > cost.width());
            widths.push(reserved);
        }
        assert!(widths[2] > widths[0].max(widths[1]), "both figures take more room");
    }

    #[test]
    fn removing_the_last_demo_cloud_releases_only_its_own_workspace() {
        let (temp, mut app) = test_app();
        let shared = app.board.create_workspace("Shared");
        let single = app.board.create_workspace("Single");
        for (issue, workspace) in [(101, shared), (102, shared), (103, single)] {
            let local = app.board.workspace(workspace).unwrap().local_id.clone();
            app.cloud_prototype.groups.0.push(CloudGroup::new(
                issue,
                "Demo".into(),
                local,
                temp.path().into(),
                [0.0, 0.0],
            ));
        }

        let ctx = egui::Context::default();
        // A new context asks for its first frames; settle it so only the release can ask.
        for _ in 0..4 {
            if !ctx.has_requested_repaint() {
                break;
            }
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();
        }
        app.cloud_action(Action::Remove(101), &ctx);
        assert!(
            !ctx.has_requested_repaint(),
            "cloud 102 still holds the shared workspace"
        );
        app.cloud_action(Action::Remove(103), &ctx);
        assert!(
            ctx.has_requested_repaint(),
            "the release schedules the frame that removes the workspace"
        );
        app.normalize_workspace_state(&ctx);

        assert!(app.board.workspace(shared).is_some(), "cloud 102 keeps its workspace");
        assert!(app.board.workspace(single).is_none());
    }
}
