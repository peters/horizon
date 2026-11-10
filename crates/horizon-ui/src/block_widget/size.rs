//! The size a new card opens at.
//!
//! A card is as wide as the approved mockup and as tall as its content needs,
//! between a floor and a ceiling. The estimate uses the same metrics as the
//! widget, so a card that fits here is also drawn without a scrollbar. It is an
//! estimate, not a measurement: measuring the real text would need a font atlas
//! on the thread that creates the panel, and a card that is a little too tall is
//! better than one that is cut off.

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

/// The size a new card of `document` opens at: [`CARD_WIDTH`] wide and as tall
/// as its content needs, between [`CARD_MIN_HEIGHT`] and [`CARD_MAX_HEIGHT`].
pub(crate) fn default_card_size(document: &BlockDocument) -> [f32; 2] {
    let body = document
        .components
        .iter()
        .enumerate()
        .map(|(index, component)| component_height(component) + if index == 0 { 0.0 } else { COMPONENT_GAP })
        .sum::<f32>();
    let chrome = PANEL_TITLEBAR_HEIGHT + 2.0 * PANEL_PADDING + FOOTER_HEIGHT;
    let padding = f32::from(BODY_PAD_TOP) + f32::from(BODY_PAD_BOTTOM);
    let height = (chrome + padding + body).clamp(CARD_MIN_HEIGHT, CARD_MAX_HEIGHT);
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
