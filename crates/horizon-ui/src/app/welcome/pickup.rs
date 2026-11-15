//! The section **Pick up where you left off** of the first start page.
//!
//! The section stands under the four tiles and above the row of shortcuts, and it
//! shows the newest session of each of at most six folders. It shows nothing when
//! this computer keeps no session that Horizon can resume, so the page then looks
//! as it does without it.
//!
//! A row of the section is the row of the window **Recent agent sessions**: the
//! same title, the same words under it and the same press. The control `Show all`
//! stands on the line of the section title and opens the full list.

use egui::{Button, CornerRadius, FontId, Pos2, Rect, Stroke, Ui, Vec2};
use horizon_core::PickupFolder;

use crate::app::session_pickup::rows::{PickupPress, ROW_GAP, ROW_HEIGHT, paint_row};

pub(in crate::app) use crate::app::session_pickup::rows::PickupPress as PickupSectionPress;
use crate::app::session_pickup::{SECTION_TITLE, SHOW_ALL};
use crate::theme;
use crate::typeface::{self, Weight};

/// The space between the tiles and the section, in whole points.
pub(in crate::app) const SECTION_GAP: f32 = 22.0;
/// The space between the section title and its first row.
const TITLE_GAP: f32 = 10.0;
/// The font of the section title.
const TITLE_FONT: f32 = 16.0;
/// The line height of the section title.
const TITLE_LINE_HEIGHT: f32 = 20.0;
/// The font of the control that opens the full list.
const SHOW_ALL_FONT: f32 = 12.0;
/// The width of that control.
const SHOW_ALL_WIDTH: f32 = 74.0;
/// The height of that control.
const SHOW_ALL_HEIGHT: f32 = 22.0;

/// The height of the section that shows `rows` rows, without the gap above it.
#[must_use]
pub(in crate::app) fn section_height(rows: usize) -> f32 {
    if rows == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let count = rows as f32;
    TITLE_LINE_HEIGHT + TITLE_GAP + count * ROW_HEIGHT + (count - 1.0) * ROW_GAP
}

/// Paints the section inside `content`, its top at `top`, and answers the press of
/// the person.
pub(in crate::app) fn paint_section(
    ui: &mut Ui,
    content: Rect,
    top: f32,
    folders: &[PickupFolder],
    confirming: impl Fn(&horizon_core::PickupRow) -> bool,
) -> Option<PickupPress> {
    if folders.is_empty() {
        return None;
    }
    paint_section_title(ui, content, top, folders.len());
    let mut press = paint_show_all(ui, content, top);
    let mut y = top + TITLE_LINE_HEIGHT + TITLE_GAP;
    for (folder_index, folder) in folders.iter().enumerate() {
        for (row_index, row) in folder.rows.iter().enumerate() {
            let rect = Rect::from_min_size(Pos2::new(content.min.x, y), Vec2::new(content.width(), ROW_HEIGHT));
            if press.is_none() {
                press = paint_row(ui, rect, folder_index, row_index, row, Some(folder), confirming(row));
            }
            y += ROW_HEIGHT + ROW_GAP;
        }
    }
    press
}

/// Paints the title of the section and the count of the sessions under it.
fn paint_section_title(ui: &Ui, content: Rect, top: f32, folders: usize) {
    let painter = ui.painter();
    let weight = typeface::format(&FontId::proportional(TITLE_FONT), theme::FG(), Weight::Semibold);
    let mut job = egui::text::LayoutJob::single_section(SECTION_TITLE.to_owned(), weight);
    job.wrap.max_rows = 1;
    job.wrap.max_width = (content.width() - SHOW_ALL_WIDTH - 12.0).max(1.0);
    let title = painter.layout_job(job);
    painter.galley(Pos2::new(content.min.x, top), title.clone(), theme::FG());
    let count = if folders == 1 {
        "1 folder".to_string()
    } else {
        format!("{folders} folders")
    };
    let count = painter.layout_no_wrap(count, FontId::proportional(11.5), theme::FG_SOFT());
    painter.galley(
        Pos2::new(content.min.x + title.size().x + 10.0, top + 3.0),
        count,
        theme::FG_SOFT(),
    );
}

/// Paints the control that opens the full list, and answers its press.
fn paint_show_all(ui: &mut Ui, content: Rect, top: f32) -> Option<PickupPress> {
    let rect = Rect::from_min_size(
        Pos2::new(content.max.x - SHOW_ALL_WIDTH, top),
        Vec2::new(SHOW_ALL_WIDTH, SHOW_ALL_HEIGHT),
    );
    let button = ui
        .put(
            rect,
            Button::new(egui::RichText::new(SHOW_ALL).size(SHOW_ALL_FONT).color(theme::FG()))
                .fill(theme::alpha(theme::ACCENT(), 22))
                .stroke(Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), 110)))
                .corner_radius(CornerRadius::same(8)),
        )
        .on_hover_text("Open every session of this computer in one list.");
    if button.clicked() {
        return Some(PickupPress::ShowAll);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ROW_GAP, ROW_HEIGHT, section_height};

    #[test]
    fn the_section_takes_no_room_without_a_row() {
        assert_eq!(
            section_height(0),
            0.0,
            "the page then looks as it does without the section"
        );
        assert!(section_height(1) > ROW_HEIGHT, "a title and one row");
        assert!(
            section_height(6) > section_height(1),
            "six rows need more room than one"
        );
    }

    #[test]
    fn every_row_after_the_first_adds_its_height_and_one_gap() {
        let one = section_height(1);
        let two = section_height(2);
        let six = section_height(6);

        assert!((two - one - ROW_HEIGHT - ROW_GAP).abs() < f32::EPSILON);
        assert!((six - one - 5.0 * (ROW_HEIGHT + ROW_GAP)).abs() < 0.01);
    }
}
