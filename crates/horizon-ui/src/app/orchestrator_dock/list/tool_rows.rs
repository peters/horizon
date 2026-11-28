//! The row of one tool call: the dot of its state, the name of the tool, what it was
//! asked to do, and the line of a result that failed.

use egui::{Rect, Sense, Ui, pos2, vec2};

use horizon_core::agent_bridge::ToolState;

use crate::theme;

use super::super::text;
use super::tool_label;
use super::{
    TOOL_DOT, TOOL_FONT_SIZE, TOOL_GAP_AFTER_DOT, TOOL_GAP_AFTER_NAME, TOOL_INDENT, TOOL_RESULT_FONT_SIZE,
    TOOL_RESULT_GAP,
};

/// One tool call: the dot of its state, its name and what it was asked to do on
/// one line, and, for a call that failed, the first line of its result under
/// them.
pub(super) fn tool_row(ui: &mut Ui, name: &str, summary: &str, result: Option<&str>, state: ToolState, width: f32) {
    let full_name = name;
    let name = tool_label(name);
    // A tool whose summary only repeats its name shows the name once.
    let summary = if summary.eq_ignore_ascii_case(name) || summary.eq_ignore_ascii_case(full_name) {
        ""
    } else {
        summary
    };
    let name_left = TOOL_INDENT + TOOL_DOT + TOOL_GAP_AFTER_DOT;
    let name_galley = text::mono_line(ui, name, TOOL_FONT_SIZE, theme::FG_SOFT(), (width - name_left).max(1.0));
    let summary_left = name_left + name_galley.size().x + TOOL_GAP_AFTER_NAME;
    let summary_width = width - summary_left;
    let summary_galley =
        (summary_width > 1.0).then(|| text::mono_line(ui, summary, TOOL_FONT_SIZE, theme::FG_DIM(), summary_width));
    let result_galley = match (state, result) {
        (ToolState::Failed, Some(result)) if summary_width > 1.0 => Some(text::mono_line(
            ui,
            result,
            TOOL_RESULT_FONT_SIZE,
            theme::PALETTE_RED(),
            summary_width,
        )),
        _ => None,
    };
    let line_height = name_galley.size().y.max(TOOL_DOT);
    let height = match &result_galley {
        Some(result) => line_height + TOOL_RESULT_GAP + result.size().y,
        None => line_height,
    };
    let (row, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if name.len() != full_name.len() {
        // The short name of a tool of a server; the whole name is one hover away.
        response.on_hover_text(full_name);
    }
    let painter = ui.painter();
    painter.circle_filled(
        pos2(row.min.x + TOOL_INDENT + TOOL_DOT / 2.0, row.min.y + line_height / 2.0),
        TOOL_DOT / 2.0,
        dot_color(state),
    );
    let top = row.min.y + (line_height - name_galley.size().y) / 2.0;
    let name_rect = Rect::from_min_size(pos2(row.min.x + name_left, top), name_galley.size());
    painter.galley(name_rect.min, name_galley, theme::FG_SOFT());
    if let Some(galley) = summary_galley {
        painter.galley(pos2(row.min.x + summary_left, top), galley, theme::FG_DIM());
    }
    if let Some(galley) = result_galley {
        painter.galley(
            pos2(row.min.x + summary_left, row.min.y + line_height + TOOL_RESULT_GAP),
            galley,
            theme::PALETTE_RED(),
        );
    }
}
/// The colour of the dot of one tool call.
fn dot_color(state: ToolState) -> egui::Color32 {
    match state {
        ToolState::Running => theme::ACCENT(),
        ToolState::Done => theme::PALETTE_GREEN(),
        ToolState::Failed => theme::PALETTE_RED(),
    }
}
