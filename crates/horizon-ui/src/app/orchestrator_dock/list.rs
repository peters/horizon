//! The list of the dock: the entries of one conversation, from the top of the
//! body to its bottom edge.
//!
//! The list scrolls between the header and the composer and it follows the
//! bottom while the person has not scrolled up (`ScrollArea::stick_to_bottom`).
//! One entry takes one row: a message of the person at the right, a text of the
//! agent at the left, one tool call, or a notice of Horizon centred over two
//! lines. A reference row of a card and a reference row of a work item take a row
//! that the person presses to bring that panel or that workspace into view, and
//! the proposal of the front desk takes the row of the two controls that start it
//! or leave it.
//!
//! Every text of the program is untrusted, so this module draws it as plain text
//! only: no markdown, no link and no path becomes active. One frame lays out at
//! most [`MAX_DRAWN_ENTRIES`] entries, which are the last ones of the list, and
//! one notice above them names the entries that it does not draw. The list
//! itself keeps [`horizon_core::agent_bridge::MAX_ENTRIES`] entries.

use egui::{Align, Label, Mesh, Rangef, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use egui::{Color32, CursorIcon};
use horizon_core::agent_bridge::{Entry, ProposalState, ToolState, WorkerAskState};

use crate::app::sidebar::rows::paint_workspace_tile;
use crate::loading_spinner;
use crate::theme;
use crate::typeface::Weight;
use horizon_core::Board;

use super::OrchestratorDock;
use super::markdown;
use super::proposal;
use super::text;
use super::worker_rows;

/// What a press on one row of the list asks the frame to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Press {
    /// Bring the panel of this reference row into view: a card of the program, or
    /// the panel of one of its workers.
    Card(String),
    /// Show the workspace of this work item, and make it the active one.
    WorkItem(String),
    /// Start the work items of this proposal, or leave them.
    Proposal { id: String, choice: proposal::Choice },
    /// Start one worker that a request asked for, or leave it.
    WorkerAsk { id: String, choice: worker_rows::Choice },
}

/// The facts that the reference rows of the list read of the board and of the
/// dock, gathered once for the frame that draws them.
#[derive(Default)]
pub(super) struct Refs {
    /// The reference row of each work item of the conversation.
    work_items: Vec<WorkItemRow>,
    /// The proposals and the requests for a worker that the conversation loaded
    /// from its file: the program that asked is gone, so they take no press.
    loaded: Vec<String>,
}

/// One reference row of a work item: its name, the state word of its workspace
/// while the board holds it, and the mark of that workspace.
#[derive(Clone, Debug)]
pub(super) struct WorkItemRow {
    work_item: String,
    name: String,
    /// Whether the board holds the workspace of this work item.
    held: bool,
    /// The state word of the work item, or `None` when the board holds no
    /// workspace of it, or when that workspace says no word.
    word: Option<&'static str>,
    accent: Color32,
}

impl Refs {
    /// The facts of the reference rows of the conversation that the dock holds of
    /// `local_id` on `board`.
    pub(super) fn build(board: &Board, dock: &OrchestratorDock, local_id: &str) -> Self {
        let mut refs = Self::default();
        let entries = dock.entries_of(local_id);
        let loaded = dock.loaded_entries(local_id);
        for (index, entry) in entries.entries.iter().enumerate() {
            match entry {
                Entry::WorkItem { work_item, name } => {
                    let workspace = board.workspace_id_by_local_id(work_item.as_str());
                    let accent = workspace
                        .and_then(|workspace| board.workspace(workspace))
                        .map_or_else(theme::FG_DIM, |workspace| theme::workspace_accent(workspace.color_idx));
                    refs.work_items.push(WorkItemRow {
                        work_item: work_item.clone(),
                        name: name.clone(),
                        held: workspace.is_some(),
                        word: workspace.and_then(|_| dock.work_item_word(work_item.as_str())),
                        accent,
                    });
                }
                Entry::Proposal { id, state, .. } if *state == ProposalState::Open && index < loaded => {
                    refs.loaded.push(id.clone());
                }
                Entry::WorkerAsk {
                    id,
                    state: WorkerAskState::Open,
                    ..
                } if index < loaded => {
                    refs.loaded.push(id.clone());
                }
                _ => {}
            }
        }
        refs
    }

    /// The reference row of `work_item`, when the conversation holds one.
    fn work_item(&self, work_item: &str) -> Option<&WorkItemRow> {
        self.work_items.iter().find(|row| row.work_item == work_item)
    }

    /// Whether the conversation loaded the proposal `id`, or the request `id` of a
    /// worker, from its file.
    fn loaded_open(&self, id: &str) -> bool {
        self.loaded.iter().any(|held| held == id)
    }
}

/// The padding of the list at each side, above the first entry and under the
/// last one.
pub(super) const LIST_PADDING: f32 = 16.0;
/// The air between two entries.
pub(super) const ENTRY_GAP: f32 = 16.0;
/// The air between two reference rows that follow each other: the rows of one
/// group belong together.
pub(super) const REF_GAP: f32 = 6.0;
/// The air between two tool rows that follow each other.
const TOOL_GAP: f32 = 4.0;
/// The entries that one frame lays out.
const MAX_DRAWN_ENTRIES: usize = 200;

/// The share of the list width that a bubble of the person takes at most.
const PERSON_SHARE: f32 = 0.86;
const BUBBLE_RADIUS: u8 = 12;
pub(super) const BUBBLE_PADDING_X: f32 = 12.0;
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

/// The air between a reference row of a card and the left edge of the list: the
/// indent of a tool row.
pub(super) const CARD_INDENT: f32 = TOOL_INDENT;
/// The share of the list width that a reference row of a card takes at most.
const CARD_SHARE: f32 = 0.96;
/// The height of one reference row.
pub(super) const CARD_HEIGHT: f32 = 34.0;
pub(super) const CARD_RADIUS: u8 = 10;
/// The alpha of the fill of a row whose card stands on the canvas.
const CARD_FILL_ALPHA: u8 = 18;
/// The alpha of its outline.
const CARD_OUTLINE_ALPHA: u8 = 115;
/// The padding of a row at its left and at its right.
pub(super) const CARD_PADDING_X: f32 = 10.0;
/// The size of the glyph of a card: a small card at the left of the row.
const CARD_GLYPH: [f32; 2] = [22.0, 16.0];
const CARD_GLYPH_RADIUS: u8 = 4;
const CARD_GLYPH_STROKE: f32 = 1.5;
/// How far under the top edge of the glyph its one line stands.
const CARD_GLYPH_LINE_DROP: f32 = 3.0;
/// The air between that line and the two sides of the glyph.
const CARD_GLYPH_LINE_INSET: f32 = 4.0;
/// The air between the glyph and the heading of the card.
pub(super) const CARD_GAP_AFTER_GLYPH: f32 = 10.0;
const CARD_TITLE_SIZE: f32 = 12.5;
const CARD_STATE_SIZE: f32 = 11.0;
/// The right end of a row whose card stands on the canvas.
pub(super) const CARD_STATE: &str = "on the canvas";
/// The right end of a row whose panel left the board.
pub(super) const CARD_STATE_CLOSED: &str = "closed";

/// The side of the mark of the workspace of a work item.
const WORK_ITEM_TILE: f32 = 20.0;
/// The size of the state word at the right end of a row of a work item.
const WORK_ITEM_STATE_SIZE: f32 = 11.0;
/// The caption above a message of the person that the front desk passed.
const VIA_FRONT_DESK: &str = "via the front desk";
const VIA_SIZE: f32 = 10.5;
/// The air between the caption of a passed message and its bubble.
const VIA_GAP: f32 = 2.0;

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
/// first text of the agent. `local_id` names the workspace of the conversation:
/// the place of the list belongs to that workspace, so the scroll of one
/// conversation never shows in another. `refs` holds the state of each work item
/// and the proposals that came from a file, and `card_present` says whether the
/// board still holds a card of a reference row.
///
/// It answers the row that the person pressed, so the frame acts on it.
pub(super) fn render(
    ui: &mut Ui,
    body: Rect,
    local_id: &str,
    entries: &[Entry],
    working: bool,
    refs: &Refs,
    card_present: &dyn Fn(&str) -> bool,
) -> Option<Press> {
    let width = (body.width() - 2.0 * LIST_PADDING).max(0.0);
    if width <= 0.0 || body.height() <= 0.0 {
        return None;
    }
    let area = Rect::from_min_max(
        pos2(body.min.x + LIST_PADDING, body.min.y),
        pos2(body.max.x - LIST_PADDING, body.max.y),
    );
    let mut area_ui = ui.new_child(UiBuilder::new().max_rect(area));
    area_ui.shrink_clip_rect(area);
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt(("orchestrator_dock_list", local_id))
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
                if let Some(press) = entry_row(ui, entry, width, refs, card_present) {
                    picked = Some(press);
                }
                previous = Some(entry);
            }
            if working {
                ui.add_space(if previous.is_some() { ENTRY_GAP } else { 0.0 });
                working_row(ui, width);
            }
            ui.add_space(LIST_PADDING);
        });
    picked
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
/// tool rows or two reference rows that follow each other.
fn gap(previous: Option<&Entry>, entry: &Entry) -> f32 {
    match previous {
        None => 0.0,
        Some(Entry::Tool { .. }) if matches!(entry, Entry::Tool { .. }) => TOOL_GAP,
        Some(previous) if is_reference(previous) && is_reference(entry) => REF_GAP,
        Some(_) => ENTRY_GAP,
    }
}

/// Whether `entry` takes the row of a reference to a card or to a work item.
fn is_reference(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Card { .. } | Entry::WorkItem { .. } | Entry::WorkerStarted { .. }
    )
}

/// The text of the notice of the entries that one frame does not draw.
fn skipped_notice(skipped: usize) -> String {
    format!("{skipped} earlier entries are not drawn")
}

/// Draws one entry of the list in one row of `width` points, and answers the row
/// that the person pressed.
fn entry_row(
    ui: &mut Ui,
    entry: &Entry,
    width: f32,
    refs: &Refs,
    card_present: &dyn Fn(&str) -> bool,
) -> Option<Press> {
    match entry {
        Entry::Person { text, via_front_desk } => {
            let caption = via_front_desk.then_some(VIA_FRONT_DESK);
            person_row(ui, text, caption, VIA_SIZE, width);
            None
        }
        Entry::Agent { text, .. } => {
            agent_row(ui, text, None, VIA_SIZE, width);
            None
        }
        Entry::Tool {
            name,
            summary,
            result,
            state,
            ..
        } => {
            tool_row(ui, name, summary, result.as_deref(), *state, width);
            None
        }
        Entry::Notice { text } => {
            notice_row(ui, text, width);
            None
        }
        Entry::Card { panel, title } => {
            let state = if card_present(panel) {
                CARD_STATE
            } else {
                CARD_STATE_CLOSED
            };
            reference_row(
                ui,
                ("orchestrator_dock_card", panel),
                title,
                state,
                width,
                card_present(panel),
            )
            .then(|| Press::Card(panel.clone()))
        }
        Entry::WorkItem { work_item, .. } => {
            if let Some(row) = refs.work_item(work_item) {
                work_item_row(ui, row, width).then(|| Press::WorkItem(work_item.clone()))
            } else {
                // A row of a work item that this frame gathered no fact of still
                // takes its height, so the list does not move under the person.
                missing_work_item_row(ui, width);
                None
            }
        }
        Entry::Proposal { id, items, state } => proposal::render(ui, id, items, *state, width, refs.loaded_open(id))
            .map(|choice| Press::Proposal { id: id.clone(), choice }),
        Entry::WorkerStarted { panel, program, .. } => {
            worker_rows::started_row(ui, panel, program, width, card_present(panel)).then(|| Press::Card(panel.clone()))
        }
        Entry::WorkerMessage { program, text, .. } => {
            worker_rows::message_row(ui, program, text, width);
            None
        }
        Entry::WorkerReport { program, text, .. } => {
            worker_rows::report_row(ui, program, text, width);
            None
        }
        Entry::WorkerAsk {
            id,
            program,
            task,
            state,
        } => worker_rows::ask_row(ui, id, program, task, *state, width, refs.loaded_open(id))
            .map(|choice| Press::WorkerAsk { id: id.clone(), choice }),
    }
}

/// One reference row of a work item that the person started from a proposal: the
/// mark of its workspace, its name and the state word of that work item. A work
/// item that the board no longer holds reads dim and takes no press, as the row of
/// a card that is gone does.
fn work_item_row(ui: &mut Ui, item: &WorkItemRow, width: f32) -> bool {
    let WorkItemRow {
        work_item,
        name,
        held,
        word,
        accent,
    } = item;
    let row_width = (width * CARD_SHARE).min(width - CARD_INDENT).max(1.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, row.height()));
    let present = *held;
    let sense = if present { Sense::click() } else { Sense::hover() };
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("orchestrator_dock_work_item", work_item)),
        sense,
    );
    let hovered = present && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let word_galley = word.map(|word| {
        text::line(
            ui,
            word,
            WORK_ITEM_STATE_SIZE,
            Weight::Regular,
            theme::FG_DIM(),
            row_width,
        )
    });
    let tile_left = rect.min.x + CARD_PADDING_X;
    let name_left = tile_left + WORK_ITEM_TILE + CARD_GAP_AFTER_GLYPH;
    let word_width = word_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + CARD_GAP_AFTER_GLYPH);
    let name_width = (rect.max.x - CARD_PADDING_X - word_width - name_left).max(1.0);
    let name_color = if present { theme::FG() } else { theme::FG_DIM() };
    let name_galley = text::line(ui, name, CARD_TITLE_SIZE, Weight::Medium, name_color, name_width);
    let (fill, outline) = if present {
        let outline = if hovered {
            theme::ACCENT()
        } else {
            theme::alpha(theme::ACCENT(), CARD_OUTLINE_ALPHA)
        };
        (theme::alpha(theme::ACCENT(), CARD_FILL_ALPHA), outline)
    } else {
        (Color32::TRANSPARENT, theme::BORDER_SUBTLE())
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CARD_RADIUS,
        fill,
        Stroke::new(1.0_f32, outline),
        StrokeKind::Inside,
    );
    let tile = Rect::from_min_size(
        pos2(tile_left, rect.center().y - WORK_ITEM_TILE / 2.0),
        vec2(WORK_ITEM_TILE, WORK_ITEM_TILE),
    );
    paint_workspace_tile(ui, tile, name, *accent, present);
    painter.galley(
        pos2(name_left, rect.center().y - name_galley.size().y / 2.0),
        name_galley,
        name_color,
    );
    if let Some(word_galley) = word_galley {
        painter.galley(
            pos2(
                rect.max.x - CARD_PADDING_X - word_galley.size().x,
                rect.center().y - word_galley.size().y / 2.0,
            ),
            word_galley,
            theme::FG_DIM(),
        );
    }
    present && response.clicked()
}

/// The empty row of a work item that the facts of this frame do not hold, so the
/// list keeps its height. It draws nothing and takes no press.
fn missing_work_item_row(ui: &mut Ui, width: f32) {
    let _ = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
}

/// One reference row of a panel that the program drew: the glyph of a panel, the
/// heading of the row and a state word at its right end, with a fill and an
/// outline of the accent. A panel that the board no longer holds reads dim and
/// takes no press. The row of a card and the row of a worker take this one form.
pub(super) fn reference_row(
    ui: &mut Ui,
    id: (&'static str, &str),
    title: &str,
    state_text: &str,
    width: f32,
    present: bool,
) -> bool {
    let row_width = (width * CARD_SHARE).min(width - CARD_INDENT).max(1.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, row.height()));
    let persistent_id = ui.make_persistent_id((id.0, id.1));
    let sense = if present { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, persistent_id, sense);
    let hovered = present && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let state_galley = text::line(
        ui,
        state_text,
        CARD_STATE_SIZE,
        Weight::Regular,
        theme::FG_DIM(),
        row_width,
    );
    let glyph_left = rect.min.x + CARD_PADDING_X;
    let title_left = glyph_left + CARD_GLYPH[0] + CARD_GAP_AFTER_GLYPH;
    let title_width =
        (rect.max.x - CARD_PADDING_X - state_galley.size().x - CARD_GAP_AFTER_GLYPH - title_left).max(1.0);
    let title_color = if present { theme::FG() } else { theme::FG_DIM() };
    let title_galley = text::line(ui, title, CARD_TITLE_SIZE, Weight::Medium, title_color, title_width);
    let (fill, outline) = if present {
        let outline = if hovered {
            theme::ACCENT()
        } else {
            theme::alpha(theme::ACCENT(), CARD_OUTLINE_ALPHA)
        };
        (theme::alpha(theme::ACCENT(), CARD_FILL_ALPHA), outline)
    } else {
        (Color32::TRANSPARENT, theme::BORDER_SUBTLE())
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CARD_RADIUS,
        fill,
        Stroke::new(1.0_f32, outline),
        StrokeKind::Inside,
    );
    let glyph = Rect::from_min_size(
        pos2(glyph_left, rect.center().y - CARD_GLYPH[1] / 2.0),
        vec2(CARD_GLYPH[0], CARD_GLYPH[1]),
    );
    let glyph_stroke = Stroke::new(CARD_GLYPH_STROKE, theme::ACCENT());
    painter.rect_stroke(glyph, CARD_GLYPH_RADIUS, glyph_stroke, StrokeKind::Inside);
    painter.hline(
        Rangef::new(glyph.min.x + CARD_GLYPH_LINE_INSET, glyph.max.x - CARD_GLYPH_LINE_INSET),
        glyph.min.y + CARD_GLYPH_LINE_DROP,
        glyph_stroke,
    );
    painter.galley(
        pos2(title_left, rect.center().y - title_galley.size().y / 2.0),
        title_galley,
        title_color,
    );
    painter.galley(
        pos2(
            rect.max.x - CARD_PADDING_X - state_galley.size().x,
            rect.center().y - state_galley.size().y / 2.0,
        ),
        state_galley,
        theme::FG_DIM(),
    );
    present && response.clicked()
}

/// One message of the person: a bubble at the right of the list, with the caption
/// of the front desk above it when the front desk passed the message.
pub(super) fn person_row(ui: &mut Ui, text: &str, caption: Option<&str>, caption_size: f32, width: f32) {
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
    let caption = caption.map(|caption| text::line(ui, caption, caption_size, Weight::Regular, theme::FG_DIM(), width));
    let caption_height = caption.as_ref().map_or(0.0, |caption| caption.size().y + VIA_GAP);
    let (row, _) = ui.allocate_exact_size(vec2(width, bubble.y + caption_height), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.max.x - bubble.x, row.min.y + caption_height), bubble);
    if let Some(caption) = caption {
        ui.painter()
            .galley(pos2(rect.max.x - caption.size().x, row.min.y), caption, theme::FG_DIM());
    }
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
pub(super) fn agent_row(ui: &mut Ui, text: &str, caption: Option<&str>, caption_size: f32, width: f32) {
    let indent = AGENT_BAR_WIDTH + AGENT_GAP;
    let text_width = (width * AGENT_SHARE - indent).max(1.0);
    // The marks of Markdown are drawn, not shown: an agent writes them every day.
    let job = markdown::job(text, BODY_SIZE, AGENT_LINE_HEIGHT, theme::FG(), text_width);
    let text_height = ui.painter().layout_job(job.clone()).size().y;
    // A caption stands above the text of the agent, at the left edge of the list.
    let caption = caption.map(|caption| text::line(ui, caption, caption_size, Weight::Regular, theme::FG_DIM(), width));
    let caption_height = caption.as_ref().map_or(0.0, |caption| caption.size().y + VIA_GAP);
    let height = text_height + caption_height;
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if let Some(caption) = caption {
        ui.painter().galley(row.min, caption, theme::FG_DIM());
    }
    let top = row.min.y + caption_height;
    agent_bar(
        ui.painter(),
        Rect::from_min_size(pos2(row.min.x, top), vec2(AGENT_BAR_WIDTH, text_height)),
    );
    let text_rect = Rect::from_min_size(pos2(row.min.x + indent, top), vec2(text_width, text_height));
    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
    );
    text_ui.add(Label::new(job).selectable(true).wrap());
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

/// The name that the row of a tool shows. A tool of a server carries the name of
/// its server in front (`mcp__<server>__<tool>`), which fills the row and hides
/// the tool, so the row shows the part after the last `__`.
pub(super) fn tool_label(name: &str) -> &str {
    if !name.starts_with("mcp__") {
        return name;
    }
    match name.rsplit_once("__") {
        Some((_, tool)) if !tool.is_empty() => tool,
        _ => name,
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
/// takes the free width. A notice that is longer than one row wraps, centred,
/// with no line at its sides: a notice that is cut hides what Horizon says.
fn notice_row(ui: &mut Ui, text: &str, width: f32) {
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

    use super::{MAX_DRAWN_ENTRIES, skipped_notice, tool_label, waits_for_text};

    #[test]
    fn the_row_of_a_tool_of_a_server_shows_the_tool_and_not_the_server() {
        assert_eq!(
            tool_label("mcp__plugin_horizon_horizon-browser__block_panel"),
            "block_panel"
        );
        assert_eq!(tool_label("Bash"), "Bash");
        assert_eq!(tool_label("my__tool"), "my__tool", "only a tool of a server is cut");
        assert_eq!(
            tool_label("mcp__server__"),
            "mcp__server__",
            "a name with no tool stays whole"
        );
    }

    fn person(text: &str) -> Entry {
        Entry::Person {
            text: text.to_owned(),
            via_front_desk: false,
        }
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
