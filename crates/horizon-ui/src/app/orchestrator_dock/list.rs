//! The list of the dock: the entries of one conversation, from the top of the
//! body to its bottom edge.
//!
//! The list scrolls between the header and the composer and it follows the
//! bottom while the person has not scrolled up (`ScrollArea::stick_to_bottom`).
//! One entry takes one row: a message of the person at the right, a text of the
//! agent at the left, one tool call, or a notice of Horizon centred over two
//! lines.
//!
//! Every text of the program is untrusted, so this module draws it as plain text
//! only: no markdown, no link and no path becomes active. One frame lays out at
//! most [`MAX_DRAWN_ENTRIES`] entries, which are the last ones of the list, and
//! one notice above them names the entries that it does not draw. The list
//! itself keeps [`horizon_core::agent_bridge::MAX_ENTRIES`] entries.

use egui::{
    Align, FontId, Label, Mesh, Rangef, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextWrapMode, Ui, UiBuilder,
    pos2, vec2,
};
use horizon_core::agent_bridge::{Entry, ToolState};

use crate::loading_spinner;
use crate::theme;
use crate::typeface::Weight;

use super::text;

/// The padding of the list at each side, above the first entry and under the
/// last one.
const LIST_PADDING: f32 = 16.0;
/// The air between two entries.
const ENTRY_GAP: f32 = 16.0;
/// The air between two tool rows that follow each other.
const TOOL_GAP: f32 = 4.0;
/// The entries that one frame lays out.
const MAX_DRAWN_ENTRIES: usize = 200;

/// The share of the list width that a bubble of the person takes at most.
const PERSON_SHARE: f32 = 0.86;
const BUBBLE_RADIUS: u8 = 12;
const BUBBLE_PADDING_X: f32 = 12.0;
const BUBBLE_PADDING_Y: f32 = 9.0;

/// The share of the list width that a text of the agent takes at most.
const AGENT_SHARE: f32 = 0.96;
const AGENT_BAR_WIDTH: f32 = 3.0;
const AGENT_BAR_RADIUS: u8 = 2;
const AGENT_GAP: f32 = 10.0;

const BODY_SIZE: f32 = 13.5;
const PERSON_LINE_HEIGHT: f32 = 1.5;
const AGENT_LINE_HEIGHT: f32 = 1.55;

const TOOL_INDENT: f32 = 13.0;
const TOOL_DOT: f32 = 6.0;
const TOOL_FONT_SIZE: f32 = 12.0;
/// The font of the line of a result under a tool row that failed.
const TOOL_RESULT_FONT_SIZE: f32 = 11.5;
/// The air between a tool row and the line of its result.
const TOOL_RESULT_GAP: f32 = 2.0;
const TOOL_GAP_AFTER_DOT: f32 = 8.0;
const TOOL_GAP_AFTER_NAME: f32 = 8.0;
/// The mauve of the workspace accents, which the bar of an agent text ends at.
const AGENT_BAR_END_ACCENT: usize = 6;

const NOTICE_FONT_SIZE: f32 = 10.5;
const NOTICE_GAP: f32 = 8.0;
/// The least length of a line of a notice at one side of its text.
const NOTICE_MIN_LINE: f32 = 8.0;

/// The height of the row of the spinner, which is the diameter of its arc.
const WORKING_HEIGHT: f32 = loading_spinner::ARC_SPAN;
const WORKING_GAP: f32 = 8.0;
const WORKING_SIZE: f32 = 12.0;

/// Draws the entries of `entries` in `body`, between the header and the
/// composer, with the row of `working` under them while a turn waits for the
/// first text of the agent.
pub(super) fn render(ui: &mut Ui, body: Rect, entries: &[Entry], working: bool) {
    let width = (body.width() - 2.0 * LIST_PADDING).max(0.0);
    if width <= 0.0 || body.height() <= 0.0 {
        return;
    }
    let area = Rect::from_min_max(
        pos2(body.min.x + LIST_PADDING, body.min.y),
        pos2(body.max.x - LIST_PADDING, body.max.y),
    );
    let mut area_ui = ui.new_child(UiBuilder::new().max_rect(area));
    area_ui.shrink_clip_rect(area);
    egui::ScrollArea::vertical()
        .id_salt("orchestrator_dock_list")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(&mut area_ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            let skipped = entries.len().saturating_sub(MAX_DRAWN_ENTRIES);
            ui.add_space(LIST_PADDING);
            if skipped > 0 {
                notice_row(ui, &skipped_notice(skipped), width);
            }
            let mut previous: Option<&Entry> = None;
            for entry in &entries[skipped..] {
                ui.add_space(gap(previous, entry));
                entry_row(ui, entry, width);
                previous = Some(entry);
            }
            if working {
                ui.add_space(if previous.is_some() { ENTRY_GAP } else { 0.0 });
                working_row(ui, width);
            }
            ui.add_space(LIST_PADDING);
        });
}

/// Whether a turn of this conversation waits for the first text of the agent:
/// it is busy, and no text of the agent came after the last message of the
/// person.
pub(super) fn waits_for_text(entries: &[Entry], busy: bool) -> bool {
    busy && !entries
        .iter()
        .rev()
        .take_while(|entry| !matches!(entry, Entry::Person { .. }))
        .any(|entry| matches!(entry, Entry::Agent { .. }))
}

/// The air above `entry`, which is wider between two entries than between two
/// tool rows that follow each other.
fn gap(previous: Option<&Entry>, entry: &Entry) -> f32 {
    match previous {
        None => 0.0,
        Some(Entry::Tool { .. }) if matches!(entry, Entry::Tool { .. }) => TOOL_GAP,
        Some(_) => ENTRY_GAP,
    }
}

/// The text of the notice of the entries that one frame does not draw.
fn skipped_notice(skipped: usize) -> String {
    format!("{skipped} earlier entries are not drawn")
}

/// Draws one entry of the list in one row of `width` points.
fn entry_row(ui: &mut Ui, entry: &Entry, width: f32) {
    match entry {
        Entry::Person { text } => person_row(ui, text, width),
        Entry::Agent { text, .. } => agent_row(ui, text, width),
        Entry::Tool {
            name,
            summary,
            result,
            state,
            ..
        } => tool_row(ui, name, summary, result.as_deref(), *state, width),
        Entry::Notice { text } => notice_row(ui, text, width),
    }
}

/// One message of the person: a bubble at the right of the list.
fn person_row(ui: &mut Ui, text: &str, width: f32) {
    let text_width = (width * PERSON_SHARE - 2.0 * BUBBLE_PADDING_X).max(1.0);
    let galley = text::paragraph(
        ui,
        text,
        BODY_SIZE,
        Weight::Regular,
        theme::FG(),
        text_width,
        PERSON_LINE_HEIGHT,
    );
    let bubble = vec2(
        galley.size().x + 2.0 * BUBBLE_PADDING_X,
        galley.size().y + 2.0 * BUBBLE_PADDING_Y,
    );
    let (row, _) = ui.allocate_exact_size(vec2(width, bubble.y), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.max.x - bubble.x, row.min.y), bubble);
    let painter = ui.painter();
    painter.rect(
        rect,
        BUBBLE_RADIUS,
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.min.x + BUBBLE_PADDING_X, rect.min.y + BUBBLE_PADDING_Y),
        galley,
        theme::FG(),
    );
}

/// One text of the agent: the accent bar at the left of the list, and the text
/// beside it, selectable, as plain text. The bar runs the full height of the text
/// and steps from the accent at its top to the mauve of the workspace accents at
/// its bottom.
fn agent_row(ui: &mut Ui, text: &str, width: f32) {
    let indent = AGENT_BAR_WIDTH + AGENT_GAP;
    let text_width = (width * AGENT_SHARE - indent).max(1.0);
    let rich = RichText::new(text)
        .font(FontId::proportional(BODY_SIZE))
        .color(theme::FG())
        .line_height(Some(BODY_SIZE * AGENT_LINE_HEIGHT));
    let galley = egui::WidgetText::from(rich.clone()).into_galley(
        ui,
        Some(TextWrapMode::Wrap),
        text_width,
        FontId::proportional(BODY_SIZE),
    );
    let height = galley.size().y;
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    agent_bar(
        ui.painter(),
        Rect::from_min_size(row.min, vec2(AGENT_BAR_WIDTH, height)),
    );
    let text_rect = Rect::from_min_size(pos2(row.min.x + indent, row.min.y), vec2(text_width, height));
    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
    );
    text_ui.add(Label::new(rich).selectable(true).wrap());
}

/// The bar of one agent text: a mesh of two colours that runs the full height of
/// the text, with a round cap at each end for its radius.
fn agent_bar(painter: &egui::Painter, rect: Rect) {
    let top = theme::ACCENT();
    let bottom = theme::workspace_accent(AGENT_BAR_END_ACCENT);
    let cap = (f32::from(AGENT_BAR_RADIUS)).min(AGENT_BAR_WIDTH / 2.0);
    let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + cap), pos2(rect.max.x, rect.max.y - cap));
    let mut mesh = Mesh::default();
    mesh.colored_vertex(body.left_top(), top);
    mesh.colored_vertex(body.right_top(), top);
    mesh.colored_vertex(body.left_bottom(), bottom);
    mesh.colored_vertex(body.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
    painter.circle_filled(pos2(rect.center().x, body.min.y), cap, top);
    painter.circle_filled(pos2(rect.center().x, body.max.y), cap, bottom);
}

/// One tool call: the dot of its state, its name and what it was asked to do on
/// one line, and, for a call that failed, the first line of its result under
/// them.
fn tool_row(ui: &mut Ui, name: &str, summary: &str, result: Option<&str>, state: ToolState, width: f32) {
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
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
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

/// One notice of Horizon: the text in the middle, and one line at each side that
/// takes the free width.
fn notice_row(ui: &mut Ui, text: &str, width: f32) {
    let max_text = (width - 2.0 * NOTICE_GAP - 2.0 * NOTICE_MIN_LINE).max(1.0);
    let galley = text::mono_line(ui, text, NOTICE_FONT_SIZE, theme::FG_DIM(), max_text);
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
fn working_row(ui: &mut Ui, width: f32) {
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

#[cfg(test)]
mod tests {
    use horizon_core::agent_bridge::Entry;

    use super::{MAX_DRAWN_ENTRIES, skipped_notice, waits_for_text};

    fn person(text: &str) -> Entry {
        Entry::Person { text: text.to_owned() }
    }

    fn agent(text: &str) -> Entry {
        Entry::Agent {
            message_id: "msg-1".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_row_of_work_shows_until_the_first_text_of_this_turn() {
        assert!(!waits_for_text(&[], false), "a quiet conversation shows no row of work");
        assert!(waits_for_text(&[person("alpha")], true));
        assert!(
            !waits_for_text(&[person("alpha"), agent("bravo")], true),
            "a text of this turn answers the row of work"
        );
        assert!(
            waits_for_text(&[agent("bravo"), person("alpha")], true),
            "the text of an earlier turn does not answer this one"
        );
        assert!(!waits_for_text(&[person("alpha"), agent("bravo")], false));
    }

    #[test]
    fn one_frame_draws_the_last_two_hundred_entries_and_names_the_others() {
        assert_eq!(MAX_DRAWN_ENTRIES, 200);
        assert_eq!(skipped_notice(3), "3 earlier entries are not drawn");
    }
}
