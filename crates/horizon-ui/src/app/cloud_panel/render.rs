use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use horizon_core::cloud_panel::CloudGroup;

use super::super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::app::{RenameEditAction, panel_chrome::show_inline_rename_editor};
use crate::theme;

#[derive(Clone, Copy)]
enum Action {
    Rename(u32),
    Collapse(u32),
    Remove(u32),
    Close(u32),
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
            let (min, max) = group.runtime_bounds_while(self.cloud_prototype.production.closing(group.issue));
            let drawer = if fixture_mode {
                None
            } else {
                self.cloud_drawer_rect(group)
            };
            (transform * Rect::from_min_max(Pos2::from(min), Pos2::from(max))).contains(position)
                || drawer.is_some_and(|drawer| (transform * drawer).contains(position))
        });
        let layer = ctx.layer_id_at(position);
        candidates
            .clone()
            .find(|group| {
                layer.is_some_and(|layer| {
                    layer.id == Id::new(("cloud-drawer", group.issue))
                        || layer.id == Id::new(("cloud-runtime", group.issue))
                })
            })
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
        let mut strip_actions = Vec::new();
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
            // A production cloud shows its spend in the status strip instead.
            let cost = group
                .remote
                .is_none()
                .then(|| self.cloud_prototype.production.runtimes.get(&group.issue))
                .flatten()
                .and_then(|runtime| runtime.cost_badge(now));
            let response = egui::Area::new(Id::new(("cloud-header", group.issue)))
                .order(Order::Middle)
                .fixed_pos(rect.min)
                .constrain(false)
                .show(ctx, |ui| {
                    ui.ctx().set_transform_layer(ui.layer_id(), transform);
                    ui.set_clip_rect(clip);
                    let (header, _) =
                        ui.allocate_exact_size(Vec2::new(rect.width(), group.header_chrome_height()), Sense::hover());
                    paint_header_base(ui, header, accent);
                    let drag_rect =
                        Rect::from_min_max(header.min, Pos2::new(close_rect(header).left(), header.bottom()));
                    // Production clouds register the drag first so the strip's controls sit above it.
                    let early_drag = group.remote.is_some().then(|| drag_area(ui, drag_rect, editing));
                    let trailing = trailing(
                        &mut self.cloud_prototype.production,
                        ui,
                        header,
                        group,
                        &self.board,
                        cost,
                        &mut strip_actions,
                    );
                    let cost_width = paint_header(ui, group, header, accent, editing, trailing);
                    if group.remote.is_some() && close_button(ui, header) {
                        action = Some(Action::Close(group.issue));
                    }
                    if editing {
                        title_action =
                            rename_field(ui, header, cost_width, &mut self.cloud_prototype.title_draft, transform);
                    }
                    let drag = early_drag.unwrap_or_else(|| drag_area(ui, drag_rect, editing));
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
            // A production cloud shows its steps and output there instead.
            if group.panels.is_empty() && !group.collapsed && group.remote.is_none() {
                let ready = self.cloud_prototype.production.accepts_panels(group);
                empty_group(ctx, group, rect, transform, clip, ready);
            }
        }
        if moved {
            self.save_cloud_prototype();
        }
        for (id, chosen) in strip_actions {
            self.apply_strip_action(id, chosen, ctx);
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
            Action::Rename(i) | Action::Collapse(i) | Action::Remove(i) | Action::Close(i) => i,
        };
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|g| g.issue == issue) else {
            return;
        };
        let mut removed_from = None;
        match action {
            Action::Close(_) => self.request_cloud_close(issue),
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
                    super::production::cards::forget_log_heights(ctx, issue);
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
        self.render_cloud_error(ctx);
        self.render_cloud_close_confirmation(ctx);
    }

    fn render_cloud_error(&mut self, ctx: &egui::Context) {
        if self.cloud_prototype.creation_open() {
            return;
        }
        let Some(error) = &self.cloud_prototype.error else {
            return;
        };
        let mut dismissed = false;
        egui::Area::new(Id::new("cloud-error"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 48.0).clamp(120.0, 560.0));
                    ui.colored_label(Color32::LIGHT_RED, error);
                    dismissed = ui.button("Dismiss").clicked();
                });
            });
        if dismissed {
            self.cloud_prototype.error = None;
        }
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

fn close_button(ui: &egui::Ui, header: Rect) -> bool {
    let close = close_rect(header);
    let response = ui.interact(close, ui.id().with("close"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Close cloud"));
    ui.painter().text(
        close.center(),
        Align2::CENTER_CENTER,
        "×",
        FontId::proportional(23.0),
        if response.hovered() {
            theme::BTN_CLOSE()
        } else {
            theme::FG_DIM()
        },
    );
    response.on_hover_text("Close cloud…").clicked()
}

/// The title editor over the title; a press outside it commits the edit.
fn rename_field(
    ui: &mut egui::Ui,
    header: Rect,
    reserved: f32,
    draft: &mut String,
    transform: egui::emath::TSTransform,
) -> RenameEditAction {
    let field = Rect::from_min_size(
        header.min + Vec2::new(64.0, 16.0),
        Vec2::new((header.width() - 64.0 - reserved).max(120.0), 32.0),
    );
    let edit = show_inline_rename_editor(ui, field, draft, FontId::proportional(21.0));
    let pressed_outside = ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|p| !(transform * field).contains(p))
    });
    if pressed_outside {
        RenameEditAction::Commit
    } else {
        edit
    }
}

fn drag_area(ui: &egui::Ui, rect: Rect, editing: bool) -> egui::Response {
    ui.interact(
        rect,
        ui.id().with("drag"),
        if editing {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        },
    )
}

/// A production cloud's status strip, or a local cloud's badges.
fn trailing(
    production: &mut super::production::Production,
    ui: &mut egui::Ui,
    header: Rect,
    group: &CloudGroup,
    board: &horizon_core::Board,
    cost: Option<String>,
    actions: &mut Vec<(u32, super::production::cards::strip::StripAction)>,
) -> Trailing {
    if group.remote.is_none() {
        return Trailing::Badges { cost };
    }
    let strip = production.header_strip(ui, header, group, board);
    if let Some(chosen) = strip.action {
        actions.push((group.issue, chosen));
    }
    Trailing::Strip {
        reserved: strip.reserved,
        subtitle: strip.subtitle,
    }
}

/// What the header shows right of the title.
enum Trailing {
    /// A local or design cloud: its panel count and optional cost badge.
    Badges { cost: Option<String> },
    /// A production cloud: the status strip already painted, and its subtitle.
    Strip { reserved: f32, subtitle: String },
}

fn close_rect(header: Rect) -> Rect {
    Rect::from_center_size(header.right_top() + Vec2::new(-24.0, 35.0), Vec2::splat(28.0))
}

/// The header's fill, bottom rule and cloud mark, under everything else in it.
fn paint_header_base(ui: &egui::Ui, rect: Rect, accent: Color32) {
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
}

/// Title, subtitle and what sits right of them. Returns the width taken from the
/// title there: the badges or the status strip.
fn paint_header(
    ui: &egui::Ui,
    group: &CloudGroup,
    rect: Rect,
    accent: Color32,
    editing: bool,
    trailing: Trailing,
) -> f32 {
    let painter = ui.painter();
    let badge = Rect::from_min_size(rect.right_top() + Vec2::new(-150.0, 23.0), Vec2::new(90.0, 25.0));
    let fill = theme::blend(theme::PANEL_BG(), accent, 0.09);
    let (reserved, subtitle) = match trailing {
        Trailing::Badges { cost } => (
            cost.map_or(0.0, |cost| cost_badge(painter, badge, cost, fill)) + 154.0,
            None,
        ),
        Trailing::Strip { reserved, subtitle } => (reserved, Some(subtitle)),
    };
    if !editing {
        let title_rect = Rect::from_min_size(
            rect.min + Vec2::new(64.0, 16.0),
            Vec2::new((rect.width() - 64.0 - reserved).max(0.0), 30.0),
        );
        painter.with_clip_rect(title_rect).text(
            title_rect.left_center(),
            Align2::LEFT_CENTER,
            &group.title,
            FontId::proportional(21.0),
            theme::FG(),
        );
    }
    let subtitle = subtitle.unwrap_or_else(|| {
        let provider = group.environment.provider.as_deref().unwrap_or("Local");
        // Providers Horizon deploys on are named by their description; the rest are the
        // prototype's design fixtures.
        let provider = match horizon_core::cloud_runtime::provider::by_id(provider) {
            Some(described) => described.label,
            None => match provider {
                "daytona" => "Daytona",
                "fly" => "Fly.io",
                "local" => "Local",
                other => other,
            },
        };
        let profile = group.environment.profile.as_deref().unwrap_or("Development");
        format!("{provider}  /  {profile}")
    });
    let subtitle_rect = Rect::from_min_max(
        rect.min + Vec2::new(64.0, 48.0),
        Pos2::new((rect.right() - reserved).max(rect.left() + 64.0), rect.top() + 70.0),
    );
    // Too long for the room left beside the strip: end it with an ellipsis, not a cut.
    let mut job = egui::text::LayoutJob::simple_singleline(subtitle, FontId::proportional(13.0), theme::FG_SOFT());
    job.wrap = egui::text::TextWrapping::truncate_at_width(subtitle_rect.width());
    let galley = painter.layout_job(job);
    painter.galley(
        Pos2::new(subtitle_rect.left(), rect.top() + 59.0 - galley.size().y / 2.0),
        galley,
        theme::FG_SOFT(),
    );
    if group.remote.is_none() {
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
    }
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

fn empty_group(
    ctx: &egui::Context,
    group: &CloudGroup,
    rect: Rect,
    transform: egui::emath::TSTransform,
    clip: Rect,
    ready: bool,
) {
    egui::Area::new(Id::new(("cloud-empty", group.issue)))
        .order(Order::Middle)
        .fixed_pos(rect.min + Vec2::new(36.0, group.header_height() + 56.0))
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
                RichText::new(if ready {
                    "Ctrl-double-click anywhere inside this cloud"
                } else {
                    "This cloud is not ready to accept panels yet."
                })
                .size(15.0)
                .color(theme::FG_SOFT()),
            );
            ui.label(
                RichText::new(if ready {
                    "to add an agent, browser or another panel."
                } else {
                    "Deploy or reconnect it, then add panels when it is ready."
                })
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

    #[test]
    fn cloud_error_is_visible_without_fullscreen_and_can_be_dismissed() {
        use egui::{Event, PointerButton};
        let (_temp, mut app) = test_app();
        app.cloud_prototype.error = Some("Agent is disabled by this cloud profile".into());
        assert!(app.cloud_prototype.fullscreen.is_none());
        let ctx = egui::Context::default();
        let mut frame = |events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                    events,
                    ..Default::default()
                },
                |ui| app.render_cloud_error(ui.ctx()),
            )
            .discard_textures()
        };
        frame(vec![]);
        let output = frame(vec![]);
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text.galley.text().contains("Agent is disabled"))
        );
        let dismiss = texts.iter().find(|text| text.galley.text() == "Dismiss").unwrap();
        let pos = dismiss.pos + dismiss.galley.rect.center().to_vec2();
        frame(vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]);
        frame(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }]);
        assert!(app.cloud_prototype.error.is_none());
    }

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
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(900.0, horizon_core::cloud_panel::HEADER), Sense::hover());
                reserved = paint_header(
                    ui,
                    &group,
                    rect,
                    cloud_accent(101),
                    false,
                    Trailing::Badges {
                        cost: cost.map(str::to_owned),
                    },
                );
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
        assert!((none - 154.0).abs() < f32::EPSILON, "the panel badge keeps its place");
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
            assert!((title_clip(&plain).right() - title_clip(&costed).right() - (reserved - none)).abs() < 0.5);
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

    #[test]
    fn removing_an_empty_card_drops_only_its_row_height_cache() {
        let (temp, mut app) = test_app();
        let workspace = app.board.create_workspace("Demo");
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        for issue in [11, 12] {
            app.cloud_prototype.groups.0.push(CloudGroup::new(
                issue,
                "Demo".into(),
                local.clone(),
                temp.path().into(),
                [0.0, 0.0],
            ));
        }
        let ctx = egui::Context::default();
        super::super::production::cards::remember_log_height_cache(&ctx, 11);
        super::super::production::cards::remember_log_height_cache(&ctx, 12);

        app.cloud_action(Action::Remove(11), &ctx);

        assert!(app.cloud_prototype.groups.0.iter().all(|group| group.issue != 11));
        assert!(!super::super::production::cards::log_height_cache_present(&ctx, 11));
        assert!(super::super::production::cards::log_height_cache_present(&ctx, 12));
    }
}
