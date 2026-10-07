use egui::{Color32, Context, CornerRadius, Order, Stroke, StrokeKind};

use crate::theme;

use super::HorizonApp;
use super::file_drop::FileDropHighlight;

impl HorizonApp {
    pub(super) fn render_file_drop_highlight(&mut self, ctx: &Context) {
        if super::panels::session_picker_panel(ctx).is_some_and(|panel| self.board.panel(panel).is_some()) {
            self.clear_file_drop_state(ctx);
            return;
        }
        let Some(highlight) = self.file_drop_highlight else {
            return;
        };

        let Some((rect, accent, corner_radius)) = (match highlight {
            FileDropHighlight::Browser(panel_id) => self
                .panel_render_caches
                .browser_ui_state
                .get(&panel_id)
                .and_then(|state| state.drop_geometry)
                .map(|(rect, _)| (rect, theme::ACCENT(), CornerRadius::same(12))),
            FileDropHighlight::Panel(panel_id) => self
                .panel_screen_rects
                .get(&panel_id)
                .copied()
                .map(|rect| (rect, theme::ACCENT(), CornerRadius::same(16))),
            FileDropHighlight::Workspace(workspace_id) => self
                .workspace_screen_rects
                .iter()
                .find(|(id, _)| *id == workspace_id)
                .map(|(_, rect)| {
                    let accent = workspace_accent(&self.board, workspace_id);
                    (*rect, accent, CornerRadius::same(20))
                }),
        }) else {
            return;
        };

        egui::Area::new(egui::Id::new("file_drop_highlight"))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                let (_, painter) = ui.allocate_painter(rect.size(), egui::Sense::hover());
                let local_rect = painter.clip_rect();
                let fill = theme::alpha(theme::blend(theme::PANEL_BG(), accent, 0.22), 48);
                let stroke = Stroke::new(2.5_f32, theme::alpha(accent, 180));
                painter.rect_filled(local_rect, corner_radius, fill);
                painter.rect_stroke(local_rect, corner_radius, stroke, StrokeKind::Inside);
                if matches!(highlight, FileDropHighlight::Browser(_)) {
                    let text = painter.layout_no_wrap(
                        "Drop files to upload".into(),
                        egui::FontId::proportional(16.0),
                        theme::FG(),
                    );
                    let card = egui::Rect::from_center_size(local_rect.center(), text.size() + egui::vec2(36.0, 24.0));
                    painter.rect_filled(card, 12, theme::PANEL_BG());
                    painter.rect_stroke(card, 12, Stroke::new(1.0, theme::ACCENT()), StrokeKind::Inside);
                    painter.galley(card.center() - text.size() * 0.5, text, theme::FG());
                }
            });
    }
}

fn workspace_accent(board: &horizon_core::Board, workspace_id: horizon_core::WorkspaceId) -> Color32 {
    board
        .workspaces
        .iter()
        .find(|ws| ws.id == workspace_id)
        .map_or(theme::ACCENT(), |ws| theme::workspace_accent(ws.color_idx))
}
