//! The rows of the list that are no entry of a conversation: one notice of Horizon
//! between two lines, and the row of a turn that waits for the first text.

use egui::{Rangef, Sense, Stroke, Ui, pos2, vec2};

use crate::loading_spinner;
use crate::theme;
use crate::typeface::Weight;

use super::super::text;
use super::{NOTICE_FONT_SIZE, NOTICE_GAP, NOTICE_MIN_LINE, WORKING_GAP, WORKING_HEIGHT, WORKING_SIZE};

/// One notice of Horizon: the text in the middle, and one line at each side that
/// takes the free width. A notice that is longer than one row wraps, centred,
/// with no line at its sides: a notice that is cut hides what Horizon says.
pub(super) fn notice_row(ui: &mut Ui, text: &str, width: f32) {
    let max_text = (width - 2.0 * NOTICE_GAP - 2.0 * NOTICE_MIN_LINE).max(1.0);
    let galley = text::mono_line(ui, text, NOTICE_FONT_SIZE, theme::FG_DIM(), max_text);
    if galley.elided {
        let wrapped = text::mono_rows_centred(ui, text, NOTICE_FONT_SIZE, theme::FG_DIM(), max_text);
        let (row, _) = ui.allocate_exact_size(vec2(width, wrapped.size().y), Sense::hover());
        ui.painter()
            .galley(pos2(row.center().x, row.min.y), wrapped, theme::FG_DIM());
        return;
    }
    let height = galley.size().y;
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let text_width = galley.size().x;
    let left = row.center().x - text_width / 2.0;
    let painter = ui.painter();
    let stroke = Stroke::new(1.0_f32, theme::BORDER_SUBTLE());
    let right_line_from = (left + text_width + NOTICE_GAP).min(row.max.x);
    painter.hline(
        Rangef::new(row.min.x, (left - NOTICE_GAP).max(row.min.x)),
        row.center().y,
        stroke,
    );
    painter.hline(Rangef::new(right_line_from, row.max.x), row.center().y, stroke);
    painter.galley(pos2(left, row.min.y), galley, theme::FG_DIM());
}

/// The row of a turn that waits for the first text of the agent: the spinner of
/// the interface and `working` beside it. It asks for the frame of the next step
/// of the spinner, and only a visible conversation draws this row.
pub(super) fn working_row(ui: &mut Ui, width: f32) {
    let (row, _) = ui.allocate_exact_size(vec2(width, WORKING_HEIGHT), Sense::hover());
    loading_spinner::paint_arc_at(
        ui,
        pos2(row.min.x + WORKING_HEIGHT / 2.0, row.center().y),
        theme::ACCENT(),
    );
    let galley = text::line(
        ui,
        "working",
        WORKING_SIZE,
        Weight::Regular,
        theme::FG_DIM(),
        (width - WORKING_HEIGHT - WORKING_GAP).max(1.0),
    );
    let top = row.center().y - galley.size().y / 2.0;
    ui.painter().galley(
        pos2(row.min.x + WORKING_HEIGHT + WORKING_GAP, top),
        galley,
        theme::FG_DIM(),
    );
}
