//! Menu rows that say what they do: a title, a one-line description and an optional
//! shortcut. Used by the toolbar Menu.
use egui::{CornerRadius, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2};

use crate::theme;

const TEXT_HEIGHT: f32 = 34.0;

/// One row. Returns its response; disabled rows still answer hover for their reason.
pub(in crate::app) fn row(
    ui: &mut Ui,
    title: &str,
    detail: &str,
    shortcut: Option<&str>,
    enabled: bool,
) -> egui::Response {
    let height = TEXT_HEIGHT + 10.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), sense);
    let response = if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    };
    // A screen reader reads the whole row, as it is painted.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, spoken(title, detail, shortcut)));
    let hot = enabled && (response.hovered() || response.has_focus());
    if hot {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20),
        );
    }
    let left = rect.min.x + 10.0;
    let mut right = rect.max.x - 10.0;
    if let Some(shortcut) = shortcut {
        // A small key chip at the right; the text column ends before it.
        let galley = ui
            .painter()
            .layout_no_wrap(shortcut.to_owned(), FontId::monospace(10.0), theme::FG_SOFT());
        let chip = Rect::from_min_size(
            Pos2::new(
                right - galley.size().x - 12.0,
                rect.center().y - galley.size().y / 2.0 - 2.0,
            ),
            galley.size() + Vec2::new(12.0, 4.0),
        );
        ui.painter()
            .rect_filled(chip, CornerRadius::same(4), theme::BG_ELEVATED());
        ui.painter()
            .galley(chip.min + Vec2::new(6.0, 2.0), galley, theme::FG_SOFT());
        right = chip.min.x - 10.0;
    }
    let inner = Rect::from_min_max(Pos2::new(left, rect.min.y), Pos2::new(right.max(left), rect.max.y));
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        if !enabled {
            ui.disable();
        }
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space((height - TEXT_HEIGHT) / 2.0);
        ui.add(egui::Label::new(RichText::new(title).size(13.5).strong().color(theme::FG())).selectable(false));
        ui.add(
            egui::Label::new(RichText::new(detail).size(11.5).color(theme::FG_SOFT()))
                .truncate()
                .selectable(false),
        );
    });
    response
}

/// The row's text as one sentence list: each part ends with a full stop.
fn spoken(title: &str, detail: &str, shortcut: Option<&str>) -> String {
    let mut label = String::new();
    for part in [Some(title), Some(detail), shortcut].into_iter().flatten() {
        if !label.is_empty() {
            label.push(' ');
        }
        label.push_str(part);
        if !part.ends_with('.') {
            label.push('.');
        }
    }
    label
}

/// A thin divider between groups of rows.
pub(in crate::app) fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.min.x + 8.0, rect.center().y),
            Pos2::new(rect.max.x - 8.0, rect.center().y),
        ],
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE(), 180)),
    );
}

/// A short caption above a group of rows.
pub(in crate::app) fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(
                RichText::new(text.to_uppercase())
                    .size(10.5)
                    .strong()
                    .color(theme::FG_DIM()),
            )
            .selectable(false),
        );
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::{row, spoken};
    use crate::test_egui::DiscardTextures as _;

    #[test]
    fn a_row_reads_all_its_text() {
        assert_eq!(
            spoken("Quick Nav", "Jump to any workspace.", Some("Ctrl+K")),
            "Quick Nav. Jump to any workspace. Ctrl+K."
        );
        assert_eq!(
            spoken("Settings", "Theme and shortcuts", None),
            "Settings. Theme and shortcuts."
        );
    }

    #[test]
    fn a_screen_reader_gets_the_whole_row() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                let _ = row(ui, "Sessions", "Switch boards.", Some("Ctrl+P"), true);
            })
            .discard_textures();
        let update = output.platform_output.accesskit_update.expect("an AccessKit update");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Sessions. Switch boards. Ctrl+P.")),
            "{:?}",
            update.nodes
        );
    }
}
