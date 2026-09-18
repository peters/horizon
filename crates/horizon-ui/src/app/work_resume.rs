use egui::Context;
use horizon_core::agent_work::AskReason;
use horizon_core::{PanelId, PanelKind};

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn render_work_resume_controls(&mut self, ui: &mut egui::Ui, panel_id: PanelId) {
        let Some(panel) = self.board.panel(panel_id) else {
            return;
        };
        if (!panel.kind.supports_session_binding() && panel.kind != PanelKind::KiloCode)
            || panel.remote_workspace().is_some()
        {
            return;
        }
        let mut enabled = panel.work_resume.enabled;
        if ui.checkbox(&mut enabled, "Continue work after restart")
            .on_hover_text("Opt in for this panel. Verified clean shutdowns can continue automatically; other cases ask first. Takes effect on the next panel launch.").changed() {
            if let Some(panel) = self.board.panel_mut(panel_id) { panel.set_work_resume_enabled(enabled); }
            self.mark_runtime_dirty();
        }
        if let Some(panel) = self.board.panel_mut(panel_id) {
            let mut hours = panel.work_resume.max_downtime_seconds / 3600;
            if enabled
                && ui
                    .horizontal(|ui| {
                        ui.label("Ask after downtime (hours)");
                        ui.add(egui::DragValue::new(&mut hours).range(0..=168)).changed()
                    })
                    .inner
            {
                panel.work_resume.max_downtime_seconds = hours.saturating_mul(3600);
                self.mark_runtime_dirty();
            }
        }
        let pending = self
            .board
            .panel(panel_id)
            .and_then(|panel| panel.terminal())
            .and_then(horizon_core::Terminal::pending_work_resume);
        if pending.is_some() {
            let refusal = self
                .board
                .panel(panel_id)
                .and_then(|panel| panel.check_work_resume().err());
            if resume_button(ui, refusal.as_ref()).clicked() {
                self.confirm_work_resume(panel_id);
                ui.close();
            }
        }
    }

    pub(super) fn work_resume_overlay_rect(&self, ctx: &Context) -> Option<egui::Rect> {
        if !self.fixed_overlays_visible()
            || !self.board.panels.iter().any(|panel| {
                panel.work_resume.enabled
                    && panel
                        .terminal()
                        .is_some_and(|terminal| terminal.pending_work_resume().is_some())
            })
        {
            return None;
        }
        let canvas = self.canvas_rect(ctx).shrink(8.0);
        Some(egui::Rect::from_min_size(
            canvas.min,
            egui::vec2(canvas.width().clamp(0.0, 620.0), canvas.height().clamp(0.0, 160.0)),
        ))
    }

    pub(super) fn render_work_resume_banner(&mut self, ctx: &Context) {
        let Some(rect) = self.work_resume_overlay_rect(ctx) else {
            return;
        };
        let mut resume = None;
        let mut reveal = None;
        egui::Area::new(egui::Id::new("work_resume_banner"))
            .fixed_pos(rect.min)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(crate::theme::BG_ELEVATED())
                    .inner_margin(8)
                    .corner_radius(8)
                    .show(ui, |ui| {
                        ui.set_width((rect.width() - 16.0).max(0.0));
                        ui.set_height((rect.height() - 16.0).max(0.0));
                        let count = self
                            .board
                            .panels
                            .iter()
                            .filter(|panel| {
                                panel.work_resume.enabled
                                    && panel
                                        .terminal()
                                        .is_some_and(|terminal| terminal.pending_work_resume().is_some())
                            })
                            .count();
                        ui.strong(format!("Review work before continuing ({count})"));
                        egui::ScrollArea::vertical()
                            .max_height((rect.height() - 42.0).max(0.0))
                            .show(ui, |ui| {
                                for panel in &self.board.panels {
                                    if !panel.work_resume.enabled {
                                        continue;
                                    }
                                    let Some(reason) =
                                        panel.terminal().and_then(horizon_core::Terminal::pending_work_resume)
                                    else {
                                        continue;
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.link(&panel.title).clicked() {
                                            reveal = Some(panel.id);
                                        }
                                        ui.label(reason_text(reason));
                                        let refusal = panel.check_work_resume().err();
                                        if resume_button(ui, refusal.as_ref()).clicked() {
                                            resume = Some(panel.id);
                                        }
                                    });
                                }
                            });
                    });
            });
        if let Some(panel_id) = reveal {
            self.reveal_selected_panel(ctx, panel_id);
        }
        if let Some(panel_id) = resume {
            self.confirm_work_resume(panel_id);
        }
    }

    fn confirm_work_resume(&mut self, panel_id: PanelId) {
        let result = self
            .board
            .panel_mut(panel_id)
            .map(horizon_core::Panel::request_work_resume);
        match result {
            Some(Ok(())) => self.queue_panel_restart(panel_id),
            Some(Err(error)) => {
                if let Some(workspace) = self.board.panel_workspace_id(panel_id) {
                    self.board.create_attention(
                        workspace,
                        Some(panel_id),
                        "resume",
                        error.to_string(),
                        horizon_core::AttentionSeverity::High,
                    );
                }
            }
            None => {}
        }
    }
}

fn reason_text(reason: AskReason) -> &'static str {
    match reason {
        AskReason::UncleanShutdown => "The previous session did not close cleanly.",
        AskReason::MissingEvidence => "Automatic continuation is not verified for this session.",
        AskReason::WaitingForUser => "A permission or question needs your review.",
        AskReason::Downtime => "The session was away longer than allowed.",
        AskReason::RepositoryChanged => "The working tree changed or could not be checked.",
        AskReason::BatchLimit => "The automatic continuation limit was reached.",
    }
}

fn resume_button(ui: &mut egui::Ui, refusal: Option<&horizon_core::Error>) -> egui::Response {
    let response = ui.add_enabled(refusal.is_none(), egui::Button::new("Resume work"));
    if let Some(error) = refusal {
        response.on_disabled_hover_text(error.to_string())
    } else {
        response.on_hover_text("Restart this conversation and continue the latest authorized request. Existing permission requirements still apply.")
    }
}
