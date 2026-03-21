use egui::{Pos2, Rect, Vec2};

use crate::app::util::usize_to_f32;

pub(super) const INPUT_HEIGHT: f32 = 44.0;
pub(super) const HEADER_ROW_HEIGHT: f32 = 26.0;
pub(super) const ROW_HEIGHT: f32 = 28.0;

const OVERLAY_WIDTH: f32 = 1100.0;
const MAX_VISIBLE_ROWS: usize = 20;

pub(super) struct OverlayLayout {
    pub(super) screen: Rect,
    pub(super) card: Rect,
    pub(super) inner: Rect,
    pub(super) results_height: f32,
}

pub(super) struct Columns {
    pub(super) alias: f32,
    pub(super) ipv4: f32,
    pub(super) tags: f32,
    pub(super) hostname: f32,
    pub(super) status: f32,
    pub(super) last_seen: f32,
}

pub(super) fn current_epoch_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

pub(super) fn overlay_layout(screen: Rect) -> OverlayLayout {
    let width = OVERLAY_WIDTH.min(screen.width() * 0.92);
    let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
    let card_height = INPUT_HEIGHT + 16.0 + HEADER_ROW_HEIGHT + results_height + 56.0;
    let card_min = Pos2::new((screen.width() - width) * 0.5, (screen.height() - card_height) * 0.22);
    let card = Rect::from_min_size(card_min, Vec2::new(width, card_height));

    OverlayLayout {
        screen,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        results_height,
    }
}

pub(super) fn columns(content_width: f32) -> Columns {
    // Proportional column layout that adapts to overlay width.
    let alias_frac = 0.23;
    let ipv4_frac = 0.13;
    let tags_frac = 0.31;
    let hostname_frac = 0.12;
    let status_frac = 0.05;
    // last_seen gets the remainder.

    let x0 = 16.0;
    Columns {
        alias: x0,
        ipv4: x0 + content_width * alias_frac,
        tags: x0 + content_width * (alias_frac + ipv4_frac),
        hostname: x0 + content_width * (alias_frac + ipv4_frac + tags_frac),
        status: x0 + content_width * (alias_frac + ipv4_frac + tags_frac + hostname_frac),
        last_seen: x0 + content_width * (alias_frac + ipv4_frac + tags_frac + hostname_frac + status_frac),
    }
}
