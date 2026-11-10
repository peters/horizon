//! The heading of a card: one strong line, an optional second line and an
//! optional badge on the same row, right aligned.

use egui::{FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::Badge;

use crate::theme;

use super::badge;
use super::metrics::{BADGE_HEIGHT, HEADING_BADGE_GAP, HEADING_FONT, HEADING_SUBTITLE_FONT, HEADING_SUBTITLE_GAP};
use super::text;

/// Draw the heading of component `index` and return the height it took.
pub(super) fn show(
    ui: &mut Ui,
    index: usize,
    text: &str,
    subtitle: Option<&str>,
    badge: Option<&Badge>,
    agent_present: bool,
) -> f32 {
    let width = ui.available_width();
    let pill = badge.map(|badge| badge::heading_pill(ui, badge, agent_present));
    let pill_width = pill.map_or(0.0, |(width, _, _)| width);
    let text_width = (width
        - if pill.is_some() {
            pill_width + HEADING_BADGE_GAP
        } else {
            0.0
        })
    .max(0.0);

    let title = text::line(
        ui.painter(),
        text,
        &FontId::proportional(HEADING_FONT),
        theme::FG(),
        text_width,
    );
    let title_height = title.size().y;
    let subtitle_galley = subtitle.map(|subtitle| {
        text::line(
            ui.painter(),
            subtitle,
            &FontId::proportional(HEADING_SUBTITLE_FONT),
            theme::FG_SOFT(),
            text_width,
        )
    });
    let subtitle_height = subtitle_galley.as_ref().map_or(0.0, |galley| galley.size().y);
    let mut height = title_height;
    if subtitle.is_some() {
        height += HEADING_SUBTITLE_GAP + subtitle_height;
    }
    if pill.is_some() {
        height = height.max(BADGE_HEIGHT);
    }

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));

    let painter = ui.painter();
    painter.galley(rect.min, title, theme::FG());
    if let Some(galley) = subtitle_galley {
        let top = rect.min.y + title_height + HEADING_SUBTITLE_GAP;
        painter.galley(Pos2::new(rect.min.x, top), galley, theme::FG_SOFT());
        label_node(ui, index, rect, top, subtitle);
    }
    if let (Some(badge), Some((pill_width, tone, prefix))) = (badge, pill) {
        let pill_rect = Rect::from_min_size(
            Pos2::new(rect.max.x - pill_width, rect.min.y),
            egui::vec2(pill_width, BADGE_HEIGHT),
        );
        badge::paint(painter, pill_rect, &badge.text, prefix, tone);
        badge::node(
            ui,
            ui.make_persistent_id(("block_heading_badge", index)),
            pill_rect,
            &badge.text,
            prefix,
        );
    }
    height
}

/// Registers the second line as its own node, so a reader hears both lines and
/// a test can find the text that was painted.
fn label_node(ui: &Ui, index: usize, row: Rect, top: f32, subtitle: Option<&str>) {
    let Some(subtitle) = subtitle else { return };
    let rect = Rect::from_min_max(
        Pos2::new(row.min.x, top),
        Pos2::new(row.max.x, top + HEADING_SUBTITLE_FONT),
    );
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_heading_subtitle", index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, subtitle));
}
