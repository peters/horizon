//! The log: the events of the work, each with its time.
//!
//! One line for each entry: the time in the monospace family, dim, in a column
//! of one width, then the words of the event. The wide card flows the entries
//! into as many columns of at least [`LOG_COLUMN_MIN_WIDTH`] points as fit, row
//! by row; the narrow card keeps one column. An entry takes no press.

use egui::{FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::LogEntry;

use crate::theme;

use super::form::Form;
use super::metrics::{
    LOG_COLUMN_GAP, LOG_COLUMN_MIN_WIDTH, LOG_ROW_GAP, LOG_TEXT_FONT, LOG_TIME_FONT, LOG_TIME_GAP, count,
};
use super::text;

/// The columns of a log that is `width` points wide: as many as fit at the least
/// width of a column, and one in the narrow form.
pub(super) fn columns(width: f32, form: Form) -> usize {
    if form == Form::Narrow {
        return 1;
    }
    let mut columns = 1;
    while count(columns + 1) * LOG_COLUMN_MIN_WIDTH + count(columns) * LOG_COLUMN_GAP <= width {
        columns += 1;
    }
    columns
}

/// Draw one `log` component.
pub(super) fn show(ui: &mut Ui, index: usize, entries: &[LogEntry], form: Form) {
    if entries.is_empty() {
        return;
    }
    let width = ui.available_width();
    let columns = columns(width, form);
    let column_width = ((width + LOG_COLUMN_GAP) / count(columns)) - LOG_COLUMN_GAP;
    let painter = ui.painter().clone();
    let times: Vec<_> = entries
        .iter()
        .map(|entry| {
            text::line(
                &painter,
                &entry.at,
                &FontId::monospace(LOG_TIME_FONT),
                theme::FG_DIM(),
                f32::INFINITY,
            )
        })
        .collect();
    let time_width = times.iter().map(|galley| galley.size().x).fold(0.0, f32::max);
    let texts: Vec<_> = entries
        .iter()
        .map(|entry| {
            text::line(
                &painter,
                &entry.text,
                &FontId::proportional(LOG_TEXT_FONT),
                theme::FG_SOFT(),
                (column_width - time_width - LOG_TIME_GAP).max(0.0),
            )
        })
        .collect();
    let line_height = times
        .iter()
        .chain(texts.iter())
        .map(|galley| galley.size().y)
        .fold(0.0, f32::max);
    let rows = entries.len().div_ceil(columns);
    let height = count(rows) * line_height + count(rows.saturating_sub(1)) * LOG_ROW_GAP;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    for (at, entry) in entries.iter().enumerate() {
        let row = at / columns;
        let column = at % columns;
        let left = rect.min.x + count(column) * (column_width + LOG_COLUMN_GAP);
        let top = rect.min.y + count(row) * (line_height + LOG_ROW_GAP);
        let origin = Pos2::new(left, top + (line_height - times[at].size().y) * 0.5);
        painter.galley(origin, times[at].clone(), theme::FG_DIM());
        painter.galley(
            Pos2::new(
                left + time_width + LOG_TIME_GAP,
                top + (line_height - texts[at].size().y) * 0.5,
            ),
            texts[at].clone(),
            theme::FG_SOFT(),
        );
        let line = Rect::from_min_size(Pos2::new(left, top), Vec2::new(column_width, line_height));
        ui.interact(
            line,
            ui.make_persistent_id(("block_log_entry", index, at)),
            Sense::hover(),
        )
        .widget_info(|| WidgetInfo::labeled(Role::Label, true, &entry.text));
    }
}
