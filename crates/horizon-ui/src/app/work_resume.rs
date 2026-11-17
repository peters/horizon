use egui::Context;
use horizon_core::agent_work::AskReason;
use horizon_core::{PanelId, PanelKind};

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn render_work_resume_controls(&mut self, ui: &mut egui::Ui, panel_id: PanelId) {
        let Some(panel) = self.board.panel(panel_id) else {
            return;
        };
        if !panel.kind.supports_session_binding() && panel.kind != PanelKind::KiloCode {
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
        if !self.fixed_overlays_visible() {
            return None;
        }
        let rows: f32 = self
            .board
            .panels
            .iter()
            .filter(|panel| waits_for_review(panel).is_some())
            .map(|_| BANNER_ROW)
            .sum();
        if rows <= 0.0 {
            return None;
        }
        // The banner takes the height of its head and of one row for each session.
        let height = (BANNER_HEAD + rows).min(BANNER_MAX_HEIGHT);
        let canvas = self.canvas_rect(ctx).shrink(8.0);
        Some(egui::Rect::from_min_size(
            canvas.min,
            egui::vec2(canvas.width().clamp(0.0, 620.0), canvas.height().clamp(0.0, height)),
        ))
    }

    pub(super) fn render_work_resume_banner(&mut self, ctx: &Context) {
        let Some(rect) = self.work_resume_overlay_rect(ctx) else {
            return;
        };
        let mut resume = None;
        let mut reveal = None;
        egui::Area::new(egui::Id::unique("work_resume_banner"))
            .fixed_pos(rect.min)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(crate::theme::BG_ELEVATED())
                    .inner_margin(8)
                    .corner_radius(8)
                    .show(ui, |ui| {
                        ui.set_width((rect.width() - 16.0).max(0.0));
                        let panels = &self.board.panels;
                        let count = panels.iter().filter(|panel| waits_for_review(panel).is_some()).count();
                        ui.strong(format!("Review work before continuing ({count})"));
                        egui::ScrollArea::vertical()
                            .max_height(BANNER_MAX_HEIGHT - 42.0)
                            .show(ui, |ui| {
                                for panel in panels {
                                    let Some(reason) = waits_for_review(panel) else {
                                        continue;
                                    };
                                    let (revealed, resumed) = review_row(ui, panel, reason);
                                    if revealed {
                                        reveal = Some(panel.id);
                                    }
                                    if resumed {
                                        resume = Some(panel.id);
                                    }
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
                tracing::error!(panel_id = panel_id.0, %error, "failed to request work resume");
            }
            None => {}
        }
    }
}

/// The largest height of the banner, which scrolls its rows above it.
const BANNER_MAX_HEIGHT: f32 = 160.0;
/// The height of the banner without its rows: its two margins, its head and the
/// gap under the head.
const BANNER_HEAD: f32 = 35.0;
/// The height of one row of the banner, with the gap to the next row.
const BANNER_ROW: f32 = 23.0;

/// Why the session of `panel` waits for a review of the person, or `None` for a
/// panel that does not continue its work or that waits for nothing.
fn waits_for_review(panel: &horizon_core::Panel) -> Option<AskReason> {
    if !panel.work_resume.enabled {
        return None;
    }
    panel.terminal().and_then(horizon_core::Terminal::pending_work_resume)
}

/// The row of one session that waits: the name of its panel, the reason and the
/// control, on one line at every width of the banner. Answers whether the person
/// pressed the name and whether they pressed the control.
///
/// The control keeps the end of the row, and the name and the reason take the
/// room that is left of it. A text that this room cuts ends with an ellipsis and
/// shows whole as its hover text.
fn review_row(ui: &mut egui::Ui, panel: &horizon_core::Panel, reason: AskReason) -> (bool, bool) {
    let (mut revealed, mut resumed) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let refusal = panel.check_work_resume().err();
            resumed = resume_button(ui, refusal.as_ref()).clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                revealed = ui.link(&panel.title).clicked();
                ui.label(reason_text(reason));
            });
        });
    });
    (revealed, resumed)
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
