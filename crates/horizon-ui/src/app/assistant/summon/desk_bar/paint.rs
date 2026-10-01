//! Painting for the bar's tiles and small widgets: tile frames, the panels drawn inside
//! them, labels, the scripted click and keycaps, and a few shared helpers.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use horizon_core::PanelKind;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::Tile;
use crate::theme;

/// Ctrl, Alt and an arrow, lighting up as if pressed.
pub(super) fn paint_keycaps(ui: &mut Ui, right: bool, progress: f32) {
    let lit = progress < 0.75;
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 232.0).max(0.0) / 2.0);
            for (label, width) in [("Ctrl", 62.0), ("Alt", 54.0), (if right { "→" } else { "←" }, 44.0)] {
                let (rect, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
                let fill = if lit {
                    theme::ACCENT().gamma_multiply(0.35)
                } else {
                    theme::PANEL_BG_ALT()
                };
                ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(7),
                    Stroke::new(1.2, if lit { theme::ACCENT() } else { theme::BORDER_STRONG() }),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(13.0),
                    if lit { theme::FG() } else { theme::FG_SOFT() },
                );
                ui.add_space(6.0);
            }
        });
    });
    ui.ctx().request_repaint();
}

pub(super) fn paint_tile_frame(ui: &Ui, preview: Rect, active: bool, hovered: bool) {
    let accent = theme::ACCENT();
    let fill = if active {
        theme::blend(theme::BG(), accent, 0.12)
    } else {
        theme::BG()
    };
    ui.painter().rect_filled(preview, CornerRadius::same(10), fill);
    let stroke = if active {
        Stroke::new(1.5, accent)
    } else if hovered {
        Stroke::new(1.0, theme::BORDER_STRONG())
    } else {
        Stroke::new(1.0, theme::BORDER_SUBTLE())
    };
    ui.painter()
        .rect_stroke(preview, CornerRadius::same(10), stroke, StrokeKind::Inside);
}

/// The panels of a workspace as small windows, with faint lines for their content
/// and a dot for each agent's state.
pub(super) fn paint_tile_panels(ui: &Ui, preview: Rect, tile: &Tile) {
    let painter = ui.painter();
    let inner = preview.shrink2(vec2(10.0, 10.0));
    let now = super::super::super::num::seconds(ui);
    for panel in &tile.panels {
        let mini = Rect::from_min_max(
            pos2(
                inner.left() + panel.at[0] * inner.width(),
                inner.top() + panel.at[1] * inner.height(),
            ),
            pos2(
                inner.left() + panel.at[2] * inner.width(),
                inner.top() + panel.at[3] * inner.height(),
            ),
        )
        .shrink(1.5);
        painter.rect_filled(
            mini,
            CornerRadius::same(3),
            theme::blend(theme::PANEL_BG_ALT(), panel.color, 0.2),
        );
        painter.rect_stroke(
            mini,
            CornerRadius::same(3),
            Stroke::new(1.0, panel.color.gamma_multiply(0.7)),
            StrokeKind::Inside,
        );
        let strip = Rect::from_min_size(mini.min, vec2(mini.width(), 3.0_f32.min(mini.height())));
        painter.rect_filled(strip, CornerRadius::same(2), panel.color);
        let mut y = mini.top() + 9.0;
        for share in [0.62, 0.84, 0.48, 0.74, 0.9, 0.55, 0.7, 0.4, 0.8] {
            if y + 3.0 >= mini.bottom() - 3.0 {
                break;
            }
            let width = mini.width() * share - 8.0;
            painter.rect_filled(
                Rect::from_min_size(pos2(mini.left() + 5.0, y), vec2(width.max(4.0), 2.0)),
                CornerRadius::same(1),
                panel.color.gamma_multiply(0.35),
            );
            y += 5.0;
        }
        if let Some(state) = panel.state {
            let (color, pulse) = state_color(state);
            let radius = if pulse {
                3.0 + 1.2 * (now * 5.0).sin().abs()
            } else {
                3.0
            };
            painter.circle_filled(mini.right_top() + vec2(-6.0, 7.0), radius, color);
        }
    }
}

/// Under a tile: its number, its name and a summary of what its agents are doing.
pub(super) fn paint_tile_label(ui: &Ui, rect: Rect, index: usize, tile: &Tile, active: bool) {
    let painter = ui.painter();
    if rect.width() < 110.0 {
        paint_compact_label(ui, rect, index, tile, active);
        return;
    }
    let label_y = rect.bottom() - 11.0;
    let name_color = if active { theme::FG() } else { theme::FG_SOFT() };
    let number = painter.text(
        pos2(rect.left() + 4.0, label_y),
        Align2::LEFT_CENTER,
        format!("{}", index + 1),
        FontId::monospace(11.0),
        theme::ACCENT(),
    );
    painter.text(
        pos2(number.right() + 7.0, label_y),
        Align2::LEFT_CENTER,
        elide(
            &tile.name,
            super::super::super::num::index((rect.width() - 80.0) / 6.5).max(6),
        ),
        FontId::proportional(12.0),
        name_color,
    );
    let summary = if tile.needs_you > 0 {
        Some((format!("{} needs you", tile.needs_you), theme::PALETTE_RED()))
    } else if tile.working > 0 {
        Some((format!("{} working", tile.working), theme::PALETTE_YELLOW()))
    } else if tile.agents > 0 {
        Some(("idle".to_string(), theme::PALETTE_GREEN()))
    } else {
        None
    };
    if let Some((text, color)) = summary {
        painter.text(
            pos2(rect.right() - 4.0, label_y),
            Align2::RIGHT_CENTER,
            text,
            FontId::proportional(10.5),
            color,
        );
    }
}

/// A narrow tile: its number and a dot for what its agents are doing.
pub(super) fn paint_compact_label(ui: &Ui, rect: Rect, index: usize, tile: &Tile, active: bool) {
    let painter = ui.painter();
    let y = rect.bottom() - 11.0;
    painter.text(
        pos2(rect.left() + 4.0, y),
        Align2::LEFT_CENTER,
        format!("{}", index + 1),
        FontId::monospace(11.0),
        if active { theme::FG() } else { theme::ACCENT() },
    );
    let dot = if tile.needs_you > 0 {
        Some(theme::PALETTE_RED())
    } else if tile.working > 0 {
        Some(theme::PALETTE_YELLOW())
    } else if tile.agents > 0 {
        Some(theme::PALETTE_GREEN())
    } else {
        None
    };
    if let Some(color) = dot {
        painter.circle_filled(pos2(rect.right() - 8.0, y), 3.5, color);
    }
}

/// A scripted click: a ripple spreading from the tile's centre and a cursor settling on it.
pub(in crate::app::assistant::summon) fn paint_click(ui: &Ui, centre: egui::Pos2, progress: f32) {
    let ease = 1.0 - (1.0 - progress).powi(3);
    ui.painter().circle_stroke(
        centre,
        8.0 + 46.0 * ease,
        Stroke::new(2.0, theme::ACCENT().gamma_multiply(1.0 - progress)),
    );
    let cursor = centre + vec2(10.0, 12.0) * (1.0 - ease) + vec2(4.0, 4.0);
    let tip = [
        cursor,
        cursor + vec2(0.0, 18.0),
        cursor + vec2(5.0, 14.0),
        cursor + vec2(11.0, 14.0),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        tip.to_vec(),
        Color32::WHITE,
        Stroke::new(1.0, Color32::BLACK),
    ));
    ui.ctx().request_repaint();
}

pub(super) fn scope_row(ui: &mut Ui, label: &str, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), theme::ACCENT().gamma_multiply(0.1));
    }
    let box_rect = Rect::from_center_size(rect.left_center() + vec2(14.0, 0.0), vec2(14.0, 14.0));
    ui.painter().rect_stroke(
        box_rect,
        CornerRadius::same(4),
        Stroke::new(1.3, if on { theme::ACCENT() } else { theme::BORDER_STRONG() }),
        StrokeKind::Inside,
    );
    if on {
        ui.painter()
            .rect_filled(box_rect.shrink(3.0), CornerRadius::same(2), theme::ACCENT());
    }
    ui.painter().text(
        rect.left_center() + vec2(30.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        if on { theme::FG() } else { theme::FG_SOFT() },
    );
    response
}

pub(in crate::app::assistant::summon) fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(10.5)
            .extra_letter_spacing(0.9)
            .color(theme::FG_DIM()),
    );
}

pub(super) fn kind_color(kind: PanelKind) -> Color32 {
    if kind.is_agent() {
        theme::ACCENT()
    } else {
        match kind {
            PanelKind::Browser => theme::PALETTE_GREEN(),
            PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => theme::PALETTE_YELLOW(),
            PanelKind::Device => theme::PALETTE_CYAN(),
            _ => theme::FG_DIM(),
        }
    }
}

/// Colour of an agent's state dot, and whether it pulses.
pub(in crate::app::assistant::summon) fn state_color(state: AgentState) -> (Color32, bool) {
    match state {
        AgentState::Working => (theme::PALETTE_YELLOW(), true),
        AgentState::NeedsInput => (theme::PALETTE_RED(), true),
        AgentState::Idle => (theme::PALETTE_GREEN(), false),
        AgentState::Starting => (theme::FG_DIM(), true),
        AgentState::Exited => (theme::BORDER_STRONG(), false),
    }
}

pub(in crate::app::assistant::summon) fn elide(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}
