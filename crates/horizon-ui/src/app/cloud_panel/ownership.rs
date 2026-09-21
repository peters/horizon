use super::HorizonApp;
use crate::app::view::canvas_scene_transform;
use crate::theme;
use egui::{Id, Order, Pos2, RichText, Vec2};
use horizon_core::{PanelKind, browser_actor};

impl HorizonApp {
    pub(in crate::app) fn render_cloud_ownership(&self, ctx: &egui::Context) {
        if !self.cloud_prototype.ready {
            return;
        }
        let canvas = self.canvas_rect(ctx);
        let transform = canvas_scene_transform(canvas, self.canvas_view);
        let clip = transform.inverse() * canvas;
        for panel in &self.board.panels {
            if !self.cloud_panel_is_in_view(&panel.local_id) {
                continue;
            }
            if !panel.visible || !matches!(panel.kind, PanelKind::Browser | PanelKind::Device) {
                continue;
            }
            if !self
                .cloud_prototype
                .groups
                .0
                .iter()
                .any(|g| g.panels.contains(&panel.local_id))
            {
                continue;
            }
            let Some(text) = self.cloud_ownership_label(panel.id) else {
                continue;
            };
            egui::Area::new(Id::new(("cloud-owner", panel.id.0)))
                .order(Order::Middle)
                .fixed_pos(Pos2::from(panel.layout.position) - Vec2::new(0.0, 22.0))
                .constrain(false)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.ctx().set_transform_layer(ui.layer_id(), transform);
                    ui.set_clip_rect(clip);
                    ui.set_width(panel.layout.size[0]);
                    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(theme::PALETTE_CYAN())).truncate());
                });
        }
    }
    pub(in crate::app) fn cloud_ownership_label(&self, id: horizon_core::PanelId) -> Option<String> {
        let panel = self.board.panel(id)?;
        if !matches!(panel.kind, PanelKind::Browser | PanelKind::Device)
            || !self
                .cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.panels.contains(&panel.local_id))
        {
            return None;
        }
        let owner = if panel.kind == PanelKind::Browser {
            panel.browser().and_then(|b| b.owner.as_deref())
        } else {
            self.panel_render_caches
                .device_ui_state
                .get(&panel.id)
                .and_then(|s| s.owner.as_deref())
        };
        let label = owner.map_or_else(
            || "No active agent".to_string(),
            |actor| {
                let agent = self.board.panels.iter().find(|p| {
                    p.kind.is_agent()
                        && (browser_actor(&p.local_id) == actor || format!("horizon:cloud-{}", p.local_id) == actor)
                });
                agent.map_or_else(
                    || "External agent".into(),
                    |p| format!("{} · panel {}", p.kind.display_name(), p.id.0),
                )
            },
        );
        let text = if panel.kind == PanelKind::Device {
            let runtime = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .find(|g| g.panels.contains(&panel.local_id))
                .and_then(|g| self.cloud_prototype.production.runtimes.get(&g.issue));
            if let Some(runtime) = runtime {
                let actor = runtime
                    .desktop_controller
                    .as_deref()
                    .or(runtime.desktop_last_input.as_deref());
                let controller = actor
                    .and_then(|actor| {
                        self.board
                            .panels
                            .iter()
                            .find(|p| format!("horizon:cloud-{}", p.local_id) == actor)
                    })
                    .map_or_else(
                        || "None".into(),
                        |p| format!("{} · panel {}", p.kind.display_name(), p.id.0),
                    );
                format!(
                    "Read-only viewer · {}: {controller}",
                    if runtime.desktop_controller.is_some() {
                        "Input controller"
                    } else {
                        "Last input"
                    }
                )
            } else {
                format!("Viewer: {label} · input control not tracked")
            }
        } else {
            format!("Browser controller: {label}")
        };
        Some(text)
    }
}
