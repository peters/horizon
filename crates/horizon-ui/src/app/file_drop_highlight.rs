use egui::{Color32, Context, CornerRadius, Order, Stroke, StrokeKind};

use crate::theme;

use super::HorizonApp;
use super::file_drop::FileDropHighlight;

impl HorizonApp {
    pub(super) fn render_file_drop_highlight(&self, ctx: &Context) {
        let Some(highlight) = self.file_drop_highlight else {
            return;
        };

        let Some((rect, accent, corner_radius)) = (match highlight {
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
                let stroke = Stroke::new(2.5, theme::alpha(accent, 180));
                painter.rect_filled(local_rect, corner_radius, fill);
                painter.rect_stroke(local_rect, corner_radius, stroke, StrokeKind::Inside);
            });
    }
}

fn workspace_accent(board: &horizon_core::Board, workspace_id: horizon_core::WorkspaceId) -> Color32 {
    board
        .workspaces
        .iter()
        .find(|ws| ws.id == workspace_id)
        .map_or(theme::ACCENT(), |ws| {
            let (r, g, b) = ws.accent();
            Color32::from_rgb(r, g, b)
        })
}
