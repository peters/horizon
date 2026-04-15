use egui::{CornerRadius, Pos2, Rect, StrokeKind, Vec2};

use crate::theme;

use super::layout::{f32_to_usize, quantize_visible_rows, usize_to_f32};

const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 18.0;

pub(super) fn render_scrollbar(
    ui: &egui::Ui,
    rect: Rect,
    scrollback: usize,
    visible_rows: usize,
    scrollback_limit: usize,
    highlighted: bool,
) {
    let painter = ui.painter_at(rect.expand2(Vec2::new(2.0, 0.0)));
    let track_fill = if highlighted {
        theme::alpha(theme::PANEL_BG_ALT(), 220)
    } else {
        theme::alpha(theme::PANEL_BG_ALT(), 170)
    };
    painter.rect_filled(rect, CornerRadius::same(u8::MAX), track_fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(u8::MAX),
        egui::Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 180)),
        StrokeKind::Outside,
    );

    let thumb_height = scrollbar_thumb_height(rect.height(), quantize_visible_rows(visible_rows), scrollback_limit);
    let thumb_rect = scrollbar_thumb_rect(rect, thumb_height, scrollback, scrollback_limit);
    painter.rect_filled(
        thumb_rect,
        CornerRadius::same(u8::MAX),
        if scrollback > 0 || highlighted {
            theme::alpha(theme::ACCENT(), 210)
        } else {
            theme::alpha(theme::FG_DIM(), 140)
        },
    );
}

pub(super) fn scrollbar_thumb_height(track_height: f32, visible_rows: u16, scrollback_limit: usize) -> f32 {
    if track_height <= SCROLLBAR_MIN_THUMB_HEIGHT {
        return track_height.max(0.0);
    }

    let visible_rows = f32::from(visible_rows.max(1));
    let total_rows = visible_rows + usize_to_f32(scrollback_limit.max(1));
    (track_height * (visible_rows / total_rows)).clamp(SCROLLBAR_MIN_THUMB_HEIGHT, track_height)
}

fn scrollbar_thumb_rect(track_rect: Rect, thumb_height: f32, scrollback: usize, scrollback_limit: usize) -> Rect {
    let max_scrollback = usize_to_f32(scrollback_limit.max(1));
    let scroll_ratio = (usize_to_f32(scrollback).min(max_scrollback) / max_scrollback).clamp(0.0, 1.0);
    let travel = (track_rect.height() - thumb_height).max(0.0);
    let thumb_top = track_rect.max.y - thumb_height - (travel * scroll_ratio);

    Rect::from_min_size(
        Pos2::new(track_rect.min.x + 1.0, thumb_top),
        Vec2::new((track_rect.width() - 2.0).max(4.0), thumb_height),
    )
}

pub(super) fn scrollbar_pointer_to_scrollback(
    pointer_position: Pos2,
    track_rect: Rect,
    thumb_height: f32,
    scrollback_limit: usize,
) -> usize {
    let clamped_y = pointer_position.y.clamp(track_rect.min.y, track_rect.max.y);
    let travel = (track_rect.height() - thumb_height).max(1.0);
    let relative = (track_rect.max.y - thumb_height - clamped_y).clamp(0.0, travel);
    let ratio = (relative / travel).clamp(0.0, 1.0);
    f32_to_usize((ratio * usize_to_f32(scrollback_limit.max(1))).round())
}

#[cfg(test)]
mod tests {
    use super::{scrollbar_pointer_to_scrollback, scrollbar_thumb_height};
    use egui::{Pos2, Rect, Vec2};

    #[test]
    fn thumb_height_stays_within_track_bounds() {
        assert!((scrollbar_thumb_height(12.0, 50, 0) - 12.0).abs() <= f32::EPSILON);

        let thumb_height = scrollbar_thumb_height(120.0, 24, 240);

        assert!(thumb_height >= 18.0);
        assert!(thumb_height <= 120.0);
    }

    #[test]
    fn pointer_position_maps_to_expected_scrollback_extremes() {
        let track_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(12.0, 100.0));
        let thumb_height = 20.0;

        assert_eq!(
            scrollbar_pointer_to_scrollback(Pos2::new(16.0, track_rect.max.y), track_rect, thumb_height, 200),
            0
        );
        assert_eq!(
            scrollbar_pointer_to_scrollback(Pos2::new(16.0, track_rect.min.y), track_rect, thumb_height, 200),
            200
        );
    }
}
