//! The sizes and spacing one card is drawn at.
//!
//! Every value here comes from the approved card mockups. The widget uses them
//! directly and [`super::size_of`] uses the same numbers to estimate the height
//! a new card needs, so a card opens at the size it is drawn.

/// Space above the first component of the body, in whole points.
pub(super) const BODY_PAD_TOP: i8 = 18;
/// Space on both sides of the body, in whole points.
pub(super) const BODY_PAD_SIDE: i8 = 16;
/// Space below the last component of the body, in whole points.
pub(super) const BODY_PAD_BOTTOM: i8 = 14;
/// Space between two components.
pub(super) const COMPONENT_GAP: f32 = 16.0;

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
/// Space between a heading and its second line.
pub(super) const HEADING_SUBTITLE_GAP: f32 = 4.0;
/// Space between the heading text and a badge on the same row.
pub(super) const HEADING_BADGE_GAP: f32 = 12.0;

/// Height of a badge.
pub(super) const BADGE_HEIGHT: f32 = 24.0;
/// Corner radius of a badge.
pub(super) const BADGE_RADIUS: u8 = 8;
/// Space on both sides inside a badge.
pub(super) const BADGE_PAD_SIDE: f32 = 10.0;
/// Font of a badge text.
pub(super) const BADGE_FONT: f32 = 11.5;
/// Diameter of the dot of a badge.
pub(super) const BADGE_DOT: f32 = 6.0;
/// Space between the dot and the text of a badge.
pub(super) const BADGE_DOT_GAP: f32 = 6.0;
/// Alpha of the fill of a badge in a state colour.
pub(super) const BADGE_FILL_ALPHA: u8 = 28;
/// Alpha of the outline of a badge in a state colour.
pub(super) const BADGE_OUTLINE_ALPHA: u8 = 90;

/// Corner radius of the inset box of a `kv`.
pub(super) const KV_RADIUS: u8 = 10;
/// Space on both sides inside the inset box.
pub(super) const KV_PAD_SIDE: f32 = 12.0;
/// Height of one pair.
pub(super) const KV_ROW_HEIGHT: f32 = 34.0;
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
pub(super) const LIST_ROW_GAP: f32 = 6.0;
/// Corner radius of a row.
pub(super) const LIST_ROW_RADIUS: u8 = 8;
/// Space on both sides inside a row.
pub(super) const LIST_ROW_PAD_SIDE: f32 = 12.0;
/// Space above and below the text of a row.
pub(super) const LIST_ROW_PAD_Y: f32 = 10.0;
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

/// Font of the label row of a progress bar.
pub(super) const PROGRESS_LABEL_FONT: f32 = 11.5;
/// Space between the label row and the bar.
pub(super) const PROGRESS_LABEL_GAP: f32 = 6.0;
/// Height of the bar.
pub(super) const PROGRESS_BAR_HEIGHT: f32 = 4.0;
/// Corner radius of the bar.
pub(super) const PROGRESS_RADIUS: u8 = 2;

/// Height of a button.
pub(super) const BUTTON_HEIGHT: f32 = 36.0;
/// Corner radius of a button.
pub(super) const BUTTON_RADIUS: u8 = 10;
/// Space between two buttons.
pub(super) const BUTTON_GAP: f32 = 8.0;
/// Font of a button label.
pub(super) const BUTTON_FONT: f32 = 13.0;
/// Space on both sides inside a primary button.
pub(super) const BUTTON_PAD_PRIMARY: f32 = 16.0;
/// Space on both sides inside every other button.
pub(super) const BUTTON_PAD: f32 = 14.0;

/// Corner radius of a status frame.
pub(super) const STATUS_RADIUS: u8 = 8;
/// Space on both sides inside a status frame.
pub(super) const STATUS_PAD_SIDE: f32 = 12.0;
/// Space above and below the text of a status frame.
pub(super) const STATUS_PAD_Y: f32 = 10.0;
/// Font of the strong line of a status frame.
pub(super) const STATUS_TITLE_FONT: f32 = 12.5;
/// Font of the second line of a status frame.
pub(super) const STATUS_LINE_FONT: f32 = 12.0;
/// Space between the two lines of a status frame.
pub(super) const STATUS_LINE_GAP: f32 = 2.0;

/// Height of the chip in the titlebar of a card.
pub(super) const CHIP_HEIGHT: f32 = 20.0;
/// Corner radius of the chip.
pub(super) const CHIP_RADIUS: u8 = 8;
/// Space on both sides inside the chip.
pub(super) const CHIP_PAD_SIDE: f32 = 8.0;
/// Font of the chip text.
pub(super) const CHIP_FONT: f32 = 10.5;
/// Diameter of the dot of the chip.
pub(super) const CHIP_DOT: f32 = 6.0;
/// Space between the dot and the text of the chip.
pub(super) const CHIP_DOT_GAP: f32 = 6.0;
/// Space between the chip and the title text.
pub(super) const CHIP_TITLE_GAP: f32 = 8.0;

/// A count of components or rows as a length. A card holds at most a few dozen
/// of them, so every conversion is exact.
pub(super) fn count(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Width of a new card.
pub(super) const CARD_WIDTH: f32 = 420.0;
/// Smallest height of a card that fits its content.
pub(super) const CARD_MIN_HEIGHT: f32 = 220.0;
/// Largest height of a card that fits its content.
pub(super) const CARD_MAX_HEIGHT: f32 = 900.0;
