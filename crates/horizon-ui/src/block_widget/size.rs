//! The height a card takes: the size a new card opens at, and the height its
//! content needs once the body measured it.
//!
//! A card is as wide as the approved mockup and as tall as its content needs,
//! between a floor and a ceiling. The estimate below sizes a card that has not
//! been drawn yet, because the text can only be measured with a font atlas on
//! the thread that draws a frame. The measurement of the first pass is the
//! truth: the host gives the card the height that fits, once per document.

use horizon_browser_control::block::{BlockDocument, BoardItem, BoardRow, Component, ListLayout, Width};

use super::board::{NOTE_LINES, cell_height, chip_height};
use super::form::Form;
use super::metrics::{
    BADGE_HEIGHT, BOARD_CELL_GAP, BOARD_COLUMN_MAX_WIDTH, BOARD_GROUP_GAP, BOARD_GROUP_HEAD_GAP, BOARD_HEAD_COLUMN,
    BOARD_HEAD_HEIGHT, BOARD_ROW_MIN_HEIGHT, BODY_PAD_BOTTOM, BODY_PAD_SIDE, BODY_PAD_TOP, BUTTON_GAP, BUTTON_HEIGHT,
    CALLOUT_CAPTION_GAP, CALLOUT_PAD_Y, CALLOUT_STACK_GAP, CALLOUT_STACK_TEXT_FONT, CALLOUT_TEXT_FONT, CAPTION_FONT,
    CAPTION_TRACKING, CARD_MAX_HEIGHT, CARD_MIN_HEIGHT, CARD_WIDTH, COMPONENT_GAP, FOOTER_HEIGHT, HEADING_FONT,
    HEADING_SUBTITLE_FONT, HEADING_SUBTITLE_GAP, HEADING_SUBTITLE_LINE_HEIGHT, KV_ROW_HEIGHT, KV_ROW_PAD_Y,
    KV_VALUE_FONT, LINE_FACTOR, LIST_LINE_GAP, LIST_ROW_GAP, LIST_ROW_PAD_Y, LIST_SUBTITLE_FONT, LIST_TITLE_FONT,
    LOG_ROW_GAP, LOG_TEXT_FONT, PILL_HEIGHT, PILL_LINE_GAP, STEP_FONT, STEP_GAP, STEP_MARK, STEP_TIME_FONT, TEXT_FONT,
    TEXT_LINE_HEIGHT, WIDE_CARD_MAX_HEIGHT, WIDE_CARD_MAX_WIDTH, WIDE_CARD_MIN_WIDTH, WIDE_HEADING_FONT, count,
};
use crate::app::{PANEL_PADDING, PANEL_TITLEBAR_HEIGHT};

/// Characters of body text that fit on one line at the card width.
const TEXT_CHARS_PER_LINE: usize = 58;
/// Characters of the second line under a heading that fit on one line at the
/// card width. The line is drawn in the size of the body text, so it holds the
/// same number of characters.
const SUBTITLE_CHARS_PER_LINE: usize = TEXT_CHARS_PER_LINE;
/// Characters of pill titles that fit on one line of pills at the card width.
const PILL_CHARS_PER_LINE: usize = 34;
/// The characters of padding that one pill spends around its title.
const PILL_CHARS_OF_PADDING: usize = 3;
/// Characters of the title of a list row that fit on one line. A row keeps the
/// lines that its name needs, and the first measured pass corrects the estimate.
const LIST_TITLE_CHARS_PER_LINE: usize = 38;
/// Characters of the second line of a list row that fit on one line.
const LIST_SUBTITLE_CHARS_PER_LINE: usize = 42;
/// Characters of the value of a key and value row that fit on one line. The
/// value takes the room that the column of labels leaves, which is about this
/// many characters of the proportional face; the first measured pass corrects
/// the estimate.
const KV_VALUE_CHARS_PER_LINE: usize = 45;
/// Characters of the evidence of a board item that fit on one line of the
/// narrow form, with the place of the item in front of it.
const NOTE_CHARS_PER_LINE: usize = 40;
/// Characters of the question of a callout that fit on one line of the narrow
/// form.
const CALLOUT_CHARS_PER_LINE: usize = 34;

/// The height a card takes between the panel edge and the body content: the
/// titlebar, the panel padding above and below the body, and the footer.
pub(crate) const CARD_CHROME_HEIGHT: f32 = PANEL_TITLEBAR_HEIGHT + 2.0 * PANEL_PADDING + FOOTER_HEIGHT;

/// The bounds of a card height that fits its content.
pub(crate) const CARD_HEIGHT_LIMITS: [f32; 2] = [CARD_MIN_HEIGHT, CARD_MAX_HEIGHT];

/// Whether the document asks for the wide card.
pub(crate) fn asks_wide(document: &BlockDocument) -> bool {
    document.width == Some(Width::Wide)
}

/// The bounds of the height of a card that draws `document`: the wide card may
/// grow taller, because a board grows with its waves.
pub(crate) fn card_height_limits(document: &BlockDocument) -> [f32; 2] {
    let ceiling = if asks_wide(document) {
        WIDE_CARD_MAX_HEIGHT
    } else {
        CARD_MAX_HEIGHT
    };
    [CARD_MIN_HEIGHT, ceiling]
}

/// The width that a wide card of `document` opens at.
///
/// A document with a board opens as wide as its board needs and the canvas view
/// of that moment allows: the half of the view, between the body that its waves
/// need at the least and the body that they can fill at the most, never more
/// than the largest width of a wide card. A view that cannot give the least
/// width opens the card as a narrow card. A document without a board keeps the
/// width of the wide card of round 40: the half of the view, between the bounds
/// of the wide card.
pub(crate) fn wide_card_width(document: &BlockDocument, canvas_width: f32) -> f32 {
    let Some(columns) = board_columns(document) else {
        return (canvas_width * 0.5).clamp(WIDE_CARD_MIN_WIDTH, WIDE_CARD_MAX_WIDTH);
    };
    let least = super::board::grid_min_width(columns) + 2.0 * f32::from(BODY_PAD_SIDE);
    if canvas_width < least {
        return CARD_WIDTH;
    }
    let most = BOARD_HEAD_COLUMN + count(columns) * BOARD_COLUMN_MAX_WIDTH + 2.0 * f32::from(BODY_PAD_SIDE);
    (canvas_width * 0.5)
        .clamp(least, most)
        .min(WIDE_CARD_MAX_WIDTH)
        .min(canvas_width)
}

/// The number of waves of the board of `document`, while it has one.
fn board_columns(document: &BlockDocument) -> Option<usize> {
    document.components.iter().find_map(|component| match component {
        Component::Board { columns, .. } => Some(columns.len()),
        _ => None,
    })
}

/// The height from the bottom edge of a card to the top of its footer strip:
/// the panel padding below the body and the strip itself.
///
/// The footer is the part of a card that holds no control, so the corner grip
/// of the panel stops above this line and every button keeps its press at every
/// canvas zoom.
pub(crate) const CARD_FOOT_SPAN: f32 = PANEL_PADDING + FOOTER_HEIGHT;

/// The height a card panel takes for the content height that its last pass
/// measured, between `limits`.
///
/// The person outranks the content: a card that was resized by hand keeps
/// `current_height`, so a later document never undoes that size. Otherwise the
/// content decides, and a card that is still too short for it scrolls its body
/// above a footer that stays put.
pub(crate) fn fitted_card_height(
    measured_content_height: f32,
    hand_resized: bool,
    current_height: f32,
    limits: [f32; 2],
) -> f32 {
    if hand_resized {
        return current_height;
    }
    (measured_content_height + CARD_CHROME_HEIGHT)
        .max(0.0)
        .clamp(limits[0], limits[1])
}

/// The size a new card of `document` opens at: the width that the document asks
/// for and the height its content is estimated to need, between the card
/// limits. A wide card opens at half of the canvas view of `canvas_width`.
///
/// The estimate is provisional: the first pass measures the real content and
/// [`fitted_card_height`] takes over from it.
pub(crate) fn default_card_size(document: &BlockDocument, canvas_width: f32) -> [f32; 2] {
    let width = if asks_wide(document) {
        wide_card_width(document, canvas_width)
    } else {
        CARD_WIDTH
    };
    let body_width = width - 2.0 * f32::from(BODY_PAD_SIDE);
    let form = Form::of(body_width);
    let body = components_height(document, form, body_width);
    let padding = f32::from(BODY_PAD_TOP) + f32::from(BODY_PAD_BOTTOM);
    let limits = card_height_limits(document);
    let height = (CARD_CHROME_HEIGHT + padding + body).clamp(limits[0], limits[1]);
    [width, height]
}

/// The height of every component of a document, with the gaps between them.
fn components_height(document: &BlockDocument, form: Form, width: f32) -> f32 {
    let mut height = 0.0;
    let mut index = 0;
    while index < document.components.len() {
        if index > 0 {
            height += COMPONENT_GAP;
        }
        let component = &document.components[index];
        // The wide form of a heading that a progress bar follows draws both in
        // one row, and that row is as tall as the taller part.
        if let (Form::Wide, Component::Heading { text, subtitle, badge }, Some(Component::Progress { detail, .. })) =
            (form, component, document.components.get(index + 1))
        {
            let heading = heading_height(text, subtitle.as_deref(), badge.is_some(), WIDE_HEADING_FONT, width);
            let bar = super::progress::height(super::progress::wide_layout(detail.as_deref()));
            height += heading.max(bar);
            index += 2;
            continue;
        }
        height += component_height(component, form, width);
        index += 1;
    }
    height
}

/// The height of one heading, at `font` and for `width` points of room.
fn heading_height(text: &str, subtitle: Option<&str>, badge: bool, font: f32, width: f32) -> f32 {
    let chars_per_line = (width / (font * 0.5)).floor().max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let chars_per_line = chars_per_line as usize;
    let lines = text.chars().count().div_ceil(chars_per_line).max(1);
    let mut height = count(lines) * font * LINE_FACTOR;
    if let Some(subtitle) = subtitle {
        let lines = subtitle.chars().count().div_ceil(SUBTITLE_CHARS_PER_LINE).max(1);
        height += HEADING_SUBTITLE_GAP + count(lines) * HEADING_SUBTITLE_FONT * HEADING_SUBTITLE_LINE_HEIGHT;
    }
    if badge {
        height = height.max(BADGE_HEIGHT);
    }
    height
}

fn progress_height(layout: super::progress::Layout) -> f32 {
    super::progress::height(layout)
}

/// The height one component needs.
fn component_height(component: &Component, form: Form, width: f32) -> f32 {
    match component {
        Component::Heading { text, subtitle, badge } => {
            // A heading names the card, so it takes every line that its words need
            // and the estimate counts them; the badge keeps the top right of the
            // first line.
            heading_height(text, subtitle.as_deref(), badge.is_some(), HEADING_FONT, width)
        }
        Component::Text { text } => {
            let lines = text
                .lines()
                .map(|line| line.chars().count().div_ceil(TEXT_CHARS_PER_LINE).max(1));
            let lines = lines.sum::<usize>().max(1);
            count(lines) * TEXT_FONT * TEXT_LINE_HEIGHT
        }
        Component::Kv { rows } => rows
            .iter()
            .map(|row| {
                // A value keeps the lines that its words need, and the pair grows
                // with them; a label is short by design and stays on one line.
                let lines = row.value.chars().count().div_ceil(KV_VALUE_CHARS_PER_LINE).max(1);
                let value = count(lines) * KV_VALUE_FONT * LINE_FACTOR;
                (value + 2.0 * KV_ROW_PAD_Y).max(KV_ROW_HEIGHT)
            })
            .sum::<f32>(),
        Component::Badge { .. } => BADGE_HEIGHT,
        Component::Progress { .. } => progress_height(super::progress::Layout::Inline),
        Component::Divider => 1.0,
        Component::List {
            rows,
            layout: ListLayout::Pills,
            ..
        } => {
            // A pill takes its title and its padding; the lines wrap where the
            // card width ends. The first pass measures the real content.
            let characters = rows
                .iter()
                .map(|row| row.title.chars().count() + PILL_CHARS_OF_PADDING)
                .sum::<usize>();
            let lines = characters
                .div_ceil(PILL_CHARS_PER_LINE)
                .max(usize::from(!rows.is_empty()));
            count(lines) * PILL_HEIGHT + count(lines.saturating_sub(1)) * PILL_LINE_GAP
        }
        Component::List { rows, .. } => {
            // Every row of one list takes the height of its tallest row, so the
            // values of the rows keep one right edge.
            let tallest = rows
                .iter()
                .map(|row| {
                    let title = row.title.chars().count().div_ceil(LIST_TITLE_CHARS_PER_LINE).max(1);
                    let mut height = 2.0 * LIST_ROW_PAD_Y + count(title) * LIST_TITLE_FONT * LINE_FACTOR;
                    if let Some(subtitle) = row.subtitle.as_ref() {
                        let lines = subtitle.chars().count().div_ceil(LIST_SUBTITLE_CHARS_PER_LINE).max(1);
                        height += LIST_LINE_GAP + count(lines) * LIST_SUBTITLE_FONT * LINE_FACTOR;
                    }
                    height
                })
                .fold(0.0, f32::max);
            count(rows.len()) * tallest + count(rows.len().saturating_sub(1)) * LIST_ROW_GAP
        }
        Component::Buttons { items } => {
            if items.is_empty() {
                0.0
            } else {
                BUTTON_HEIGHT
            }
        }
        Component::Link { .. } => super::link::height(),
        Component::Caption { .. } => CAPTION_FONT + CAPTION_TRACKING,
        Component::Board { columns, rows, items } => match form {
            Form::Wide => board_grid_height(columns.len(), rows.len(), items),
            Form::Narrow => board_list_height(columns.len(), rows, items),
        },
        Component::Callout {
            caption, text, buttons, ..
        } => callout_height(caption.is_some(), text, buttons.len(), form),
        Component::Log { entries } => log_height(entries.len(), form, width),
        Component::Steps { steps } => {
            let heights: f32 = steps
                .iter()
                .map(|_| {
                    (STEP_MARK)
                        .max(STEP_FONT * LINE_FACTOR)
                        .max(STEP_TIME_FONT * LINE_FACTOR)
                })
                .sum();
            heights + count(steps.len().saturating_sub(1)) * STEP_GAP
        }
    }
}

/// The height of the grid of a board: the head row and every body row, which is
/// as tall as its fullest cell.
fn board_grid_height(columns: usize, rows: usize, items: &[BoardItem]) -> f32 {
    let mut height = BOARD_HEAD_HEIGHT;
    for row in 0..rows {
        let tallest = (0..columns)
            .map(|column| {
                items
                    .iter()
                    .filter(|item| item.column == column && item.row == row)
                    .count()
            })
            .max()
            .unwrap_or(0);
        height += cell_height(tallest).max(BOARD_ROW_MIN_HEIGHT);
    }
    height
}

/// The height of the list of a board: the caption of every group and its items,
/// each with the lines that its evidence needs.
fn board_list_height(columns: usize, rows: &[BoardRow], items: &[BoardItem]) -> f32 {
    let mut height = 0.0;
    let mut first = true;
    for column in 0..columns {
        let group: Vec<&BoardItem> = items.iter().filter(|item| item.column == column).collect();
        if group.is_empty() {
            continue;
        }
        if !first {
            height += BOARD_GROUP_GAP;
        }
        first = false;
        height += CAPTION_FONT + CAPTION_TRACKING + BOARD_GROUP_HEAD_GAP;
        for item in &group {
            let place = rows.get(item.row).map_or(0, |row| row.label.chars().count());
            let note = item.note.as_ref().map_or(0, |note| note.chars().count() + 3);
            height += chip_height((place + note).div_ceil(NOTE_CHARS_PER_LINE).clamp(1, NOTE_LINES));
        }
        height += count(group.len().saturating_sub(1)) * BOARD_CELL_GAP;
    }
    height
}

/// The height of a callout: one row in the wide form, a stack in the narrow one.
fn callout_height(caption: bool, text: &str, buttons: usize, form: Form) -> f32 {
    let head = if caption {
        CAPTION_FONT + CAPTION_TRACKING + CALLOUT_CAPTION_GAP
    } else {
        0.0
    };
    let lines = match form {
        Form::Wide => 1,
        Form::Narrow => text.chars().count().div_ceil(CALLOUT_CHARS_PER_LINE).max(1),
    };
    let body = count(lines) * body_font(form) * LINE_FACTOR;
    let rows = buttons.max(1);
    let buttons = match (buttons, form) {
        (0, _) => 0.0,
        (_, Form::Wide) => BUTTON_HEIGHT,
        (_, Form::Narrow) => count(rows) * BUTTON_HEIGHT + count(rows - 1) * BUTTON_GAP,
    };
    let content = match form {
        Form::Wide => (head + body).max(buttons),
        Form::Narrow => {
            head + body
                + if buttons > 0.0 {
                    CALLOUT_STACK_GAP + buttons
                } else {
                    0.0
                }
        }
    };
    content + 2.0 * CALLOUT_PAD_Y
}

fn body_font(form: Form) -> f32 {
    match form {
        Form::Wide => CALLOUT_TEXT_FONT,
        Form::Narrow => CALLOUT_STACK_TEXT_FONT,
    }
}

/// The height of a log: as many lines as the columns of the form need.
fn log_height(entries: usize, form: Form, width: f32) -> f32 {
    let rows = entries.div_ceil(super::log::columns(width, form));
    let line = LOG_TEXT_FONT * LINE_FACTOR;
    count(rows) * line + count(rows.saturating_sub(1)) * LOG_ROW_GAP
}
