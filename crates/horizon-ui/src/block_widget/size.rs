//! The height a card takes: the size a new card opens at, and the height its
//! content needs once the body measured it.
//!
//! A card is as wide as the approved mockup and as tall as its content needs,
//! between a floor and a ceiling. The estimate below sizes a card that has not
//! been drawn yet, because the text can only be measured with a font atlas on
//! the thread that draws a frame. The measurement of the first pass is the
//! truth: the host gives the card the height that fits, once per document.

use horizon_browser_control::block::{BlockDocument, Component};

use super::metrics::{
    BADGE_HEIGHT, BODY_PAD_BOTTOM, BODY_PAD_TOP, BUTTON_HEIGHT, CARD_MAX_HEIGHT, CARD_MIN_HEIGHT, CARD_WIDTH,
    COMPONENT_GAP, FOOTER_HEIGHT, HEADING_FONT, HEADING_SUBTITLE_FONT, HEADING_SUBTITLE_GAP, KV_ROW_HEIGHT,
    LIST_LINE_GAP, LIST_ROW_GAP, LIST_ROW_PAD_Y, LIST_SUBTITLE_FONT, LIST_TITLE_FONT, PROGRESS_BAR_HEIGHT,
    PROGRESS_LABEL_FONT, PROGRESS_LABEL_GAP, TEXT_FONT, TEXT_LINE_HEIGHT, count,
};
use crate::app::{PANEL_PADDING, PANEL_TITLEBAR_HEIGHT};

/// Characters of body text that fit on one line at the card width.
const TEXT_CHARS_PER_LINE: usize = 58;
/// Height of one line of the proportional face, as a multiple of its size.
const LINE_FACTOR: f32 = 1.35;

/// The height a card takes between the panel edge and the body content: the
/// titlebar, the panel padding above and below the body, and the footer.
pub(crate) const CARD_CHROME_HEIGHT: f32 = PANEL_TITLEBAR_HEIGHT + 2.0 * PANEL_PADDING + FOOTER_HEIGHT;

/// The bounds of a card height that fits its content.
pub(crate) const CARD_HEIGHT_LIMITS: [f32; 2] = [CARD_MIN_HEIGHT, CARD_MAX_HEIGHT];

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

/// The size a new card of `document` opens at: [`CARD_WIDTH`] wide and the
/// height its content is estimated to need, between the card limits.
///
/// The estimate is provisional: the first pass measures the real content and
/// [`fitted_card_height`] takes over from it.
pub(crate) fn default_card_size(document: &BlockDocument) -> [f32; 2] {
    let body = document
        .components
        .iter()
        .enumerate()
        .map(|(index, component)| component_height(component) + if index == 0 { 0.0 } else { COMPONENT_GAP })
        .sum::<f32>();
    let padding = f32::from(BODY_PAD_TOP) + f32::from(BODY_PAD_BOTTOM);
    let height = (CARD_CHROME_HEIGHT + padding + body).clamp(CARD_MIN_HEIGHT, CARD_MAX_HEIGHT);
    [CARD_WIDTH, height]
}

/// The height one component needs.
fn component_height(component: &Component) -> f32 {
    match component {
        Component::Heading { subtitle, badge, .. } => {
            let mut height = HEADING_FONT * LINE_FACTOR;
            if subtitle.is_some() {
                height += HEADING_SUBTITLE_GAP + HEADING_SUBTITLE_FONT * LINE_FACTOR;
            }
            if badge.is_some() {
                height = height.max(BADGE_HEIGHT);
            }
            height
        }
        Component::Text { text } => {
            let lines = text
                .lines()
                .map(|line| line.chars().count().div_ceil(TEXT_CHARS_PER_LINE).max(1));
            let lines = lines.sum::<usize>().max(1);
            count(lines) * TEXT_FONT * TEXT_LINE_HEIGHT
        }
        Component::Kv { rows } => count(rows.len()) * KV_ROW_HEIGHT,
        Component::Badge { .. } => BADGE_HEIGHT,
        Component::Progress { .. } => PROGRESS_LABEL_FONT * LINE_FACTOR + PROGRESS_LABEL_GAP + PROGRESS_BAR_HEIGHT,
        Component::Divider => 1.0,
        Component::List { rows, .. } => {
            let heights: f32 = rows
                .iter()
                .map(|row| {
                    let mut height = 2.0 * LIST_ROW_PAD_Y + LIST_TITLE_FONT * LINE_FACTOR;
                    if row.subtitle.is_some() {
                        height += LIST_LINE_GAP + LIST_SUBTITLE_FONT * LINE_FACTOR;
                    }
                    height
                })
                .sum();
            heights + count(rows.len().saturating_sub(1)) * LIST_ROW_GAP
        }
        Component::Buttons { items } => {
            if items.is_empty() {
                0.0
            } else {
                BUTTON_HEIGHT
            }
        }
    }
}
