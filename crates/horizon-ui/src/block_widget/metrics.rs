//! The sizes and spacing one card is drawn at.
//!
//! Every value here comes from the approved card mockups. The widget uses them
//! directly and [`super::size_of`] uses the same numbers to estimate the height
//! a new card needs, so a card opens at the size it is drawn.

/// Space above the first component of the body, in whole points.
pub(super) const BODY_PAD_TOP: i8 = 16;
/// Space on both sides of the body, in whole points.
pub(super) const BODY_PAD_SIDE: i8 = 16;
/// Space below the last component of the body, in whole points.
pub(super) const BODY_PAD_BOTTOM: i8 = 16;
/// Space between two components.
pub(super) const COMPONENT_GAP: f32 = 16.0;
/// Height of one line of the proportional face, as a multiple of its size.
pub(super) const LINE_FACTOR: f32 = 1.35;

// One set of corner radii. Every rounded part of a card takes one of these four
// values, so a frame, a control and a small part of two cards are the same
// shape.

/// Corner radius of a frame inside a card: the board, the callout, the box of a
/// `kv`.
pub(super) const RADIUS_FRAME: u8 = 12;
/// Corner radius of a control: a row, a pill, an item of a board, a button, a
/// badge, the state chip of a titlebar and a status frame.
pub(super) const RADIUS_CONTROL: u8 = 8;
/// Corner radius of a small part: the square of a wave tag and the outline of
/// the tag of an item.
pub(super) const RADIUS_TAG: u8 = 4;
/// Corner radius of the bar of a progress, which is as round as it is high.
pub(super) const RADIUS_BAR: u8 = 2;

/// Height of the footer strip.
pub(super) const FOOTER_HEIGHT: f32 = 30.0;
/// Font of the footer texts.
pub(super) const FOOTER_FONT: f32 = 11.0;
/// Space between a footer text and the card edge.
pub(super) const FOOTER_PAD_SIDE: f32 = 16.0;

/// Font of the one heading of a card.
pub(super) const HEADING_FONT: f32 = 18.0;
/// Font of the second line under a heading.
pub(super) const HEADING_SUBTITLE_FONT: f32 = 12.5;
/// Line height of the second line under a heading, as a multiple of its font
/// size.
pub(super) const HEADING_SUBTITLE_LINE_HEIGHT: f32 = 1.35;
/// Space between a heading and its second line.
pub(super) const HEADING_SUBTITLE_GAP: f32 = 4.0;
/// Space between the heading text and a badge on the same row.
pub(super) const HEADING_BADGE_GAP: f32 = 12.0;
/// Space between the second line of a heading and a badge that takes a row of
/// its own under it.
pub(super) const HEADING_BADGE_ROW_GAP: f32 = 8.0;

/// Height of a badge.
pub(super) const BADGE_HEIGHT: f32 = 24.0;
/// Space on both sides inside a badge.
pub(super) const BADGE_PAD_SIDE: f32 = 12.0;
/// Font of a badge text.
pub(super) const BADGE_FONT: f32 = 11.5;
/// Diameter of the dot of a badge.
pub(super) const BADGE_DOT: f32 = 6.0;
/// Space between the dot and the text of a badge.
pub(super) const BADGE_DOT_GAP: f32 = 4.0;
/// Alpha of the fill of a badge in a state colour.
pub(super) const BADGE_FILL_ALPHA: u8 = 28;
/// Alpha of the outline of a badge in a state colour.
pub(super) const BADGE_OUTLINE_ALPHA: u8 = 90;

/// Space on both sides inside the inset box.
pub(super) const KV_PAD_SIDE: f32 = 12.0;
/// Height of one pair that holds one line of text.
pub(super) const KV_ROW_HEIGHT: f32 = 34.0;
/// Space above and below the text of a pair. A pair of one line keeps
/// [`KV_ROW_HEIGHT`]; a pair whose label or value needs more lines grows by the
/// height of those lines.
pub(super) const KV_ROW_PAD_Y: f32 = 8.0;
/// Thickness of the line between two pairs.
pub(super) const KV_SEPARATOR: f32 = 0.5;
/// Alpha of the line between two pairs.
pub(super) const KV_SEPARATOR_ALPHA: u8 = 179;
/// Font of a label.
pub(super) const KV_LABEL_FONT: f32 = 12.5;
/// Font of a value.
pub(super) const KV_VALUE_FONT: f32 = 12.5;
/// Font of a monospace value.
pub(super) const KV_MONO_FONT: f32 = 12.0;
/// Least space between a label and its value.
pub(super) const KV_COLUMN_GAP: f32 = 12.0;

/// Font of body text.
pub(super) const TEXT_FONT: f32 = 12.5;
/// Line height of body text, as a multiple of its font size.
pub(super) const TEXT_LINE_HEIGHT: f32 = 1.5;

/// Space between two rows of a list.
pub(super) const LIST_ROW_GAP: f32 = 8.0;
/// Space on both sides inside a row.
pub(super) const LIST_ROW_PAD_SIDE: f32 = 12.0;
/// Space above and below the text of a row.
pub(super) const LIST_ROW_PAD_Y: f32 = 12.0;
/// Outline width of a row that is not selected.
pub(super) const LIST_ROW_OUTLINE: f32 = 1.0;
/// Outline width of the selected row.
pub(super) const LIST_ROW_OUTLINE_SELECTED: f32 = 2.0;
/// Diameter of the radio mark of a row.
pub(super) const LIST_MARK_SIZE: f32 = 14.0;
/// Ring width of an unselected radio mark.
pub(super) const LIST_MARK_RING: f32 = 1.5;
/// Ring width of the selected radio mark.
pub(super) const LIST_MARK_RING_SELECTED: f32 = 4.0;
/// Space between the radio mark and the text of a row.
pub(super) const LIST_MARK_GAP: f32 = 12.0;
/// Font of the title of a row.
pub(super) const LIST_TITLE_FONT: f32 = 13.0;
/// Font of the second line of a row.
pub(super) const LIST_SUBTITLE_FONT: f32 = 11.5;
/// Space between the title and the second line of a row.
pub(super) const LIST_LINE_GAP: f32 = 2.0;
/// Font of the short value at the right edge of a row.
pub(super) const LIST_VALUE_FONT: f32 = 12.0;
/// Least space between the text of a row and its value or its badge.
pub(super) const LIST_VALUE_GAP: f32 = 12.0;

/// Height of one pill of a list that draws pills.
pub(super) const PILL_HEIGHT: f32 = 30.0;
/// Space on both sides inside a pill.
pub(super) const PILL_PAD_SIDE: f32 = 12.0;
/// Font of a pill.
pub(super) const PILL_FONT: f32 = 12.5;
/// Space between two pills of one line.
pub(super) const PILL_GAP: f32 = 8.0;
/// Space between two lines of pills.
pub(super) const PILL_LINE_GAP: f32 = 8.0;

/// Font of a caption.
pub(super) const CAPTION_FONT: f32 = 10.5;
/// How far the letters of a caption are spread, in points.
pub(super) const CAPTION_TRACKING: f32 = 0.7;

/// Diameter of the mark of a step.
pub(super) const STEP_MARK: f32 = 16.0;
/// Ring width of the mark of a step.
pub(super) const STEP_MARK_RING: f32 = 1.5;
/// Space between the mark of a step and its text.
pub(super) const STEP_MARK_GAP: f32 = 12.0;
/// Space between two steps.
pub(super) const STEP_GAP: f32 = 12.0;
/// Font of the line of a step.
pub(super) const STEP_FONT: f32 = 12.5;
/// Font of the time at the right edge of a step.
pub(super) const STEP_TIME_FONT: f32 = 11.5;
/// Least space between the line of a step and its time.
pub(super) const STEP_TIME_GAP: f32 = 12.0;

/// Font of the label row of a progress bar.
pub(super) const PROGRESS_LABEL_FONT: f32 = 11.5;
/// Space between the label row and the bar.
pub(super) const PROGRESS_LABEL_GAP: f32 = 8.0;
/// Height of the bar.
pub(super) const PROGRESS_BAR_HEIGHT: f32 = 4.0;

/// Height of a button.
pub(super) const BUTTON_HEIGHT: f32 = 36.0;
/// Space between two buttons.
pub(super) const BUTTON_GAP: f32 = 8.0;
/// Font of a button label.
pub(super) const BUTTON_FONT: f32 = 13.0;
/// Space on both sides inside a primary button.
pub(super) const BUTTON_PAD_PRIMARY: f32 = 16.0;
/// Space on both sides inside every other button.
pub(super) const BUTTON_PAD: f32 = 12.0;

/// Font of the label of a link.
pub(super) const LINK_FONT: f32 = 13.0;
/// Font of the host under the label of a link.
pub(super) const LINK_HOST_FONT: f32 = 11.5;
/// Space between the label of a link and the host under it.
pub(super) const LINK_HOST_GAP: f32 = 2.0;
/// Thickness of the line under the label of a link.
pub(super) const LINK_UNDERLINE: f32 = 1.0;

/// Space on both sides inside a status frame.
pub(super) const STATUS_PAD_SIDE: f32 = 12.0;
/// Space above and below the text of a status frame.
pub(super) const STATUS_PAD_Y: f32 = 12.0;
/// Font of the strong line of a status frame.
pub(super) const STATUS_TITLE_FONT: f32 = 12.5;
/// Font of the second line of a status frame.
pub(super) const STATUS_LINE_FONT: f32 = 12.0;
/// Space between the two lines of a status frame.
pub(super) const STATUS_LINE_GAP: f32 = 2.0;

/// Height of the chip in the titlebar of a card.
pub(super) const CHIP_HEIGHT: f32 = 20.0;
/// Space on both sides inside the chip.
pub(super) const CHIP_PAD_SIDE: f32 = 8.0;
/// Font of the chip text.
pub(super) const CHIP_FONT: f32 = 10.5;
/// Diameter of the dot of the chip.
pub(super) const CHIP_DOT: f32 = 6.0;
/// Space between the dot and the text of the chip.
pub(super) const CHIP_DOT_GAP: f32 = 4.0;
/// Space between the chip and the title text.
pub(super) const CHIP_TITLE_GAP: f32 = 8.0;

/// A count of components or rows as a length. A card holds at most a few dozen
/// of them, so every conversion is exact.
pub(super) fn count(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Width of a new card.
pub(super) const CARD_WIDTH: f32 = 420.0;
/// The least width a wide card opens at.
pub(super) const WIDE_CARD_MIN_WIDTH: f32 = 1100.0;
/// The largest width a wide card opens at.
pub(super) const WIDE_CARD_MAX_WIDTH: f32 = 2400.0;
/// The least body width that draws the wide form of every component.
pub(super) const WIDE_FORM_MIN_WIDTH: f32 = 1000.0;
/// The width of the box that a progress bar takes beside the heading of a wide
/// card.
pub(super) const WIDE_PROGRESS_WIDTH: f32 = 320.0;
/// Space between the heading text and the progress bar beside it.
pub(super) const WIDE_PROGRESS_GAP: f32 = 28.0;
/// Font of the heading of a wide card that stands beside a progress bar.
pub(super) const WIDE_HEADING_FONT: f32 = 26.0;
/// Smallest height of a card that fits its content.
///
/// It keeps the titlebar, one line of the body and the footer on screen, so a
/// card with a single line of content ends right after that content instead of
/// leaving an empty band above its footer.
pub(super) const CARD_MIN_HEIGHT: f32 = 140.0;
/// Largest height of a card that fits its content.
pub(super) const CARD_MAX_HEIGHT: f32 = 900.0;
/// Largest height of a wide card that fits its content.
pub(super) const WIDE_CARD_MAX_HEIGHT: f32 = 1200.0;

// The board of a card: the grid of a wide card and the grouped list of a narrow
// one, from the approved board mockup.

/// Height of the head row of the grid, which carries the name of every wave.
pub(super) const BOARD_HEAD_HEIGHT: f32 = 44.0;
/// Least height of one body row of the grid; a row grows with its fullest cell.
pub(super) const BOARD_ROW_MIN_HEIGHT: f32 = 104.0;
/// Width of the first column of the grid, which carries the workers.
pub(super) const BOARD_HEAD_COLUMN: f32 = 230.0;
/// The least width of one column of the grid, with the items of its wave.
pub(super) const BOARD_COLUMN_MIN_WIDTH: f32 = 260.0;
/// The largest width of one column of the grid. A grid that needs less than the
/// body stays at the left edge.
pub(super) const BOARD_COLUMN_MAX_WIDTH: f32 = 480.0;
/// The largest width of the list of a board inside a card that draws the wide
/// form.
pub(super) const BOARD_LIST_MAX_WIDTH: f32 = 560.0;
/// Space on both sides inside a head cell.
pub(super) const BOARD_HEAD_PAD_SIDE: f32 = 12.0;
/// Space between two parts of a head cell.
pub(super) const BOARD_HEAD_GAP: f32 = 8.0;
/// Space between the parts of a worker cell.
pub(super) const BOARD_ROW_GAP: f32 = 2.0;
/// Space inside a cell, on every side.
pub(super) const BOARD_CELL_PAD: f32 = 8.0;
/// Space between two items of one cell.
pub(super) const BOARD_CELL_GAP: f32 = 8.0;
/// Side of the square that carries the tag of a wave.
pub(super) const BOARD_TAG_SIZE: f32 = 20.0;
/// Font of the tag of a wave.
pub(super) const BOARD_TAG_FONT: f32 = 11.0;
/// Font of the name of a wave.
pub(super) const BOARD_COLUMN_FONT: f32 = 12.5;
/// Font of the note of a wave, in the monospace family.
pub(super) const BOARD_NOTE_FONT: f32 = 10.5;
/// Font of the value of a wave, in the monospace family.
pub(super) const BOARD_VALUE_FONT: f32 = 11.5;
/// Font of the line of a worker.
pub(super) const BOARD_ROW_LABEL_FONT: f32 = 14.0;
/// Font of the second line of a worker.
pub(super) const BOARD_ROW_SUBTITLE_FONT: f32 = 11.5;
/// How much of the accent the cell of the wave that runs now blends into its
/// fill.
pub(super) const BOARD_CURRENT_TINT: f32 = 0.045;
/// How much of the border colour the hair lines between two cells carry.
pub(super) const BOARD_HAIR_TINT: f32 = 0.45;
/// Width of the accent line under the head of the wave that runs now.
pub(super) const BOARD_CURRENT_LINE: f32 = 2.0;
/// Space between two groups of the narrow form.
pub(super) const BOARD_GROUP_GAP: f32 = 16.0;
/// Space between a group caption and its first item.
pub(super) const BOARD_GROUP_HEAD_GAP: f32 = 8.0;
/// Font of the value of a group caption of the narrow form, in the monospace
/// family.
pub(super) const BOARD_GROUP_VALUE_FONT: f32 = 11.5;

/// Height of one item, from its two lines and its padding.
pub(super) const ITEM_BORDER: f32 = 1.0;
/// Space above and below the lines of an item.
pub(super) const ITEM_PAD_Y: f32 = 12.0;
/// Space on both sides inside an item.
pub(super) const ITEM_PAD_SIDE: f32 = 12.0;
/// Diameter of the mark of an item.
pub(super) const ITEM_MARK: f32 = 18.0;
/// Ring width of the mark of an item.
pub(super) const ITEM_MARK_RING: f32 = 1.5;
/// Width of the halo around the mark of the item that runs now.
pub(super) const ITEM_HALO: f32 = 4.0;
/// How much of the accent or the warn colour the halo carries.
pub(super) const ITEM_HALO_ALPHA: u8 = 46;
/// Space between the mark and the title of an item.
pub(super) const ITEM_MARK_GAP: f32 = 12.0;
/// Space between the two lines of an item.
pub(super) const ITEM_LINE_GAP: f32 = 2.0;
/// Font of the title of an item.
pub(super) const ITEM_TITLE_FONT: f32 = 13.5;
/// Font of the value of an item, in the monospace family.
pub(super) const ITEM_VALUE_FONT: f32 = 12.0;
/// Font of the evidence of an item, in the monospace family.
pub(super) const ITEM_NOTE_FONT: f32 = 11.5;
/// Line height of the evidence of an item, in points.
pub(super) const ITEM_NOTE_LINE_HEIGHT: f32 = 15.5;
/// Font of the tag of an item.
pub(super) const ITEM_TAG_FONT: f32 = 10.0;
/// Space between the title and the tag of an item.
pub(super) const ITEM_TAG_GAP: f32 = 8.0;
/// Space on both sides inside the outline of the tag of an item.
pub(super) const ITEM_TAG_PAD: f32 = 4.0;
/// Least space between the title of an item and its value.
pub(super) const ITEM_VALUE_GAP: f32 = 8.0;
/// How much of the ok colour the evidence of a finished item carries.
pub(super) const ITEM_DONE_NOTE_TINT: f32 = 0.75;

// The callout: one strip that asks the person something.

/// Space on both sides inside a callout.
pub(super) const CALLOUT_PAD_SIDE: f32 = 16.0;
/// Space above and below the content of a callout.
pub(super) const CALLOUT_PAD_Y: f32 = 16.0;
/// Space between the two parts of the wide callout row.
pub(super) const CALLOUT_ROW_GAP: f32 = 20.0;
/// Space between the caption, the text and the buttons of a callout.
pub(super) const CALLOUT_STACK_GAP: f32 = 12.0;
/// Space between the caption and the text of a callout.
pub(super) const CALLOUT_CAPTION_GAP: f32 = 2.0;
/// Font of the text of a wide callout.
pub(super) const CALLOUT_TEXT_FONT: f32 = 16.0;
/// Font of the text of a narrow callout.
pub(super) const CALLOUT_STACK_TEXT_FONT: f32 = 14.5;

// The log: the events of the work.

/// Font of the time of a log entry, in the monospace family.
pub(super) const LOG_TIME_FONT: f32 = 11.5;
/// Font of the text of a log entry.
pub(super) const LOG_TEXT_FONT: f32 = 12.5;
/// Space between the time and the text of an entry.
pub(super) const LOG_TIME_GAP: f32 = 12.0;
/// Least width of one column of a wide log.
pub(super) const LOG_COLUMN_MIN_WIDTH: f32 = 330.0;
/// Space between two columns of a wide log.
pub(super) const LOG_COLUMN_GAP: f32 = 28.0;
/// Space between two lines of a log.
pub(super) const LOG_ROW_GAP: f32 = 4.0;
