//! Host-owned vertical scrollbar geometry shared by every browser backend.
//!
//! Native Chromium scrollbars are often too low-contrast to use, and some
//! `WebDriver` screenshots omit scrollbar pixels entirely. Horizon paints one
//! overlay from this geometry and routes gutter presses through engine-owned
//! `scrollTo` instead of backend chrome that CDP/`WebDriver` cannot operate.

use crate::frames::PageScrollState;

/// Smallest painted vertical track, in CSS pixels.
pub const MIN_TRACK_WIDTH: f32 = 12.0;
const MIN_THUMB_HEIGHT: f32 = 24.0;

/// CSS-pixel overlay matching the indicator Horizon paints over a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalScrollbarOverlay {
    pub track_x: f32,
    pub track_width: f32,
    pub thumb_y: f32,
    pub thumb_height: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum VerticalScrollbarPress {
    Track(f64),
    Drag(VerticalScrollbarDrag),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct VerticalScrollbarDrag {
    pointer_y: f64,
    scroll_y: f64,
    max_scroll: f64,
    scroll_per_pointer_pixel: f64,
}

impl VerticalScrollbarDrag {
    pub(crate) fn target_scroll_y(self, pointer_y: f64) -> f64 {
        (self.scroll_y + ((pointer_y - self.pointer_y) * self.scroll_per_pointer_pixel)).clamp(0.0, self.max_scroll)
    }
}

impl PageScrollState {
    #[must_use]
    pub fn is_vertically_scrollable(self) -> bool {
        self.is_valid() && self.content_height > self.client_height + f32::EPSILON
    }

    #[must_use]
    pub fn vertical_overlay(self) -> Option<VerticalScrollbarOverlay> {
        if !self.is_vertically_scrollable() {
            return None;
        }
        let track_width = self.vertical_track_width();
        let max_scroll = self.content_height - self.client_height;
        let visible_fraction = (self.client_height / self.content_height).clamp(0.0, 1.0);
        let natural_thumb = self.client_height * visible_fraction;
        let thumb_height = if self.client_height >= MIN_THUMB_HEIGHT {
            natural_thumb.clamp(MIN_THUMB_HEIGHT, self.client_height)
        } else {
            self.client_height
        };
        let progress = (self.scroll_y / max_scroll).clamp(0.0, 1.0);
        Some(VerticalScrollbarOverlay {
            track_x: (self.viewport_width - track_width).max(0.0),
            track_width,
            thumb_y: (self.client_height - thumb_height) * progress,
            thumb_height,
        })
    }

    pub(crate) fn from_chromium_layout_metrics(
        metrics: &serde_json::Value,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Option<Self> {
        let layout = metrics
            .get("cssLayoutViewport")
            .or_else(|| metrics.get("layoutViewport"))?;
        let content = metrics.get("cssContentSize").or_else(|| metrics.get("contentSize"))?;
        let candidate = Self {
            scroll_x: json_f32(layout.get("pageX")?)?,
            scroll_y: json_f32(layout.get("pageY")?)?,
            viewport_width: u32_to_f32(viewport_width)?,
            viewport_height: u32_to_f32(viewport_height)?,
            client_width: json_f32(layout.get("clientWidth")?)?,
            client_height: json_f32(layout.get("clientHeight")?)?,
            content_width: json_f32(content.get("width")?)?,
            content_height: json_f32(content.get("height")?)?,
        };
        candidate.is_valid().then_some(candidate)
    }

    pub(crate) fn with_scroll_y(self, scroll_y: f64) -> Self {
        let max_scroll = (self.content_height - self.client_height).max(0.0);
        Self {
            scroll_y: finite_f32(scroll_y).map_or(self.scroll_y, |value| value.clamp(0.0, max_scroll)),
            ..self
        }
    }

    pub(crate) fn vertical_press(self, x: f64, y: f64) -> Option<VerticalScrollbarPress> {
        let overlay = self.vertical_overlay()?;
        let viewport_width = f64::from(self.viewport_width);
        let client_height = f64::from(self.client_height);
        let track_left = f64::from(overlay.track_x);
        if x < track_left || x > viewport_width || y < 0.0 || y > client_height {
            return None;
        }

        let max_scroll = f64::from(self.content_height - self.client_height);
        let thumb_top = f64::from(overlay.thumb_y);
        let thumb_height = f64::from(overlay.thumb_height);
        let scroll_y = f64::from(self.scroll_y).clamp(0.0, max_scroll);
        if y >= thumb_top - 2.0 && y <= thumb_top + thumb_height + 2.0 {
            let thumb_travel = (client_height - thumb_height).max(1.0);
            return Some(VerticalScrollbarPress::Drag(VerticalScrollbarDrag {
                pointer_y: y,
                scroll_y,
                max_scroll,
                scroll_per_pointer_pixel: max_scroll / thumb_travel,
            }));
        }
        let direction = if y < thumb_top { -1.0 } else { 1.0 };
        Some(VerticalScrollbarPress::Track(
            (scroll_y + (direction * client_height)).clamp(0.0, max_scroll),
        ))
    }

    fn vertical_track_width(self) -> f32 {
        // Independent of `viewport - client`, which is aggregate reserved space
        // and can include a left gutter (`scrollbar-gutter: stable both-edges`).
        MIN_TRACK_WIDTH.min(self.viewport_width)
    }
}

fn json_f32(value: &serde_json::Value) -> Option<f32> {
    let number = value
        .as_f64()
        .or_else(|| value.as_u64().map(u64_to_f64))
        .or_else(|| value.as_i64().map(i64_to_f64))?;
    finite_f32(number)
}

fn u32_to_f32(value: u32) -> Option<f32> {
    finite_f32(f64::from(value))
}

fn u64_to_f64(value: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    {
        value as f64
    }
}

fn i64_to_f64(value: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    {
        value as f64
    }
}

fn finite_f32(value: f64) -> Option<f32> {
    #[allow(clippy::cast_possible_truncation)]
    let value = value as f32;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{VerticalScrollbarPress, json_f32};
    use crate::frames::PageScrollState;

    fn classic() -> PageScrollState {
        PageScrollState {
            scroll_x: 0.0,
            scroll_y: 1_200.0,
            viewport_width: 1_164.0,
            viewport_height: 608.0,
            client_width: 1_149.0,
            client_height: 608.0,
            content_width: 1_149.0,
            content_height: 3_000.0,
        }
    }

    fn overlay() -> PageScrollState {
        PageScrollState {
            client_width: 684.0,
            viewport_width: 684.0,
            viewport_height: 508.0,
            client_height: 508.0,
            content_width: 684.0,
            content_height: 2_618.0,
            scroll_x: 0.0,
            scroll_y: 640.0,
        }
    }

    #[test]
    fn classic_gutter_keeps_a_fixed_host_track_and_pages() {
        let state = classic();
        let overlay = state.vertical_overlay().expect("scrollable page");
        assert!((overlay.track_width - super::MIN_TRACK_WIDTH).abs() < f32::EPSILON);
        assert!((overlay.track_x - (1_164.0 - super::MIN_TRACK_WIDTH)).abs() < f32::EPSILON);

        let Some(VerticalScrollbarPress::Drag(drag)) = state.with_scroll_y(0.0).vertical_press(1_155.0, 72.0) else {
            panic!("visible scrollbar thumb should start a drag");
        };
        assert!((drag.target_scroll_y(361.0) - 1_426.0).abs() < 2.0);

        let Some(VerticalScrollbarPress::Track(target)) = state.with_scroll_y(0.0).vertical_press(1_155.0, 300.0)
        else {
            panic!("track should page");
        };
        assert!((target - 608.0).abs() < f64::EPSILON);
        assert!(state.vertical_press(1_148.0, 300.0).is_none());
    }

    #[test]
    fn aggregate_client_delta_does_not_widen_the_right_edge_track() {
        let mut state = classic();
        state.client_width = state.viewport_width - 30.0;
        state.content_width = state.client_width;
        let overlay = state.vertical_overlay().expect("scrollable both-edges page");
        assert!((overlay.track_width - super::MIN_TRACK_WIDTH).abs() < f32::EPSILON);
        assert!((overlay.track_x - (state.viewport_width - super::MIN_TRACK_WIDTH)).abs() < f32::EPSILON);
        assert!(state.vertical_press(f64::from(overlay.track_x - 1.0), 300.0).is_none());
        assert!(state.vertical_press(f64::from(overlay.track_x + 1.0), 300.0).is_some());
    }

    #[test]
    fn overlay_track_is_at_least_the_painted_minimum_and_owns_its_gutter() {
        let state = overlay();
        let paint = state.vertical_overlay().expect("scrollable overlay page");
        assert!((paint.track_width - super::MIN_TRACK_WIDTH).abs() < f32::EPSILON);
        assert!((paint.track_x - 672.0).abs() < f32::EPSILON);

        assert!(matches!(
            state.vertical_press(676.0, 128.0),
            Some(VerticalScrollbarPress::Drag(_))
        ));
        assert!(matches!(
            state.vertical_press(676.0, 300.0),
            Some(VerticalScrollbarPress::Track(_))
        ));
        assert!(state.vertical_press(671.0, 128.0).is_none());
    }

    #[test]
    fn chromium_integer_layout_metrics_populate_host_scroll_state() {
        let metrics = serde_json::json!({
            "cssLayoutViewport": {
                "pageX": 0,
                "pageY": 120,
                "clientWidth": 1149,
                "clientHeight": 608
            },
            "cssContentSize": { "width": 1149, "height": 3000 }
        });
        let state = PageScrollState::from_chromium_layout_metrics(&metrics, 1_164, 608).expect("metrics");
        assert!((state.scroll_y - 120.0).abs() < f32::EPSILON);
        assert!((state.viewport_width - 1_164.0).abs() < f32::EPSILON);
        assert!((state.client_width - 1_149.0).abs() < f32::EPSILON);
        assert!(state.is_vertically_scrollable());
        assert_eq!(json_f32(&serde_json::json!(1149)), Some(1_149.0));
    }

    #[test]
    fn slightly_narrower_content_width_still_paints_a_vertical_overlay() {
        let mut state = classic();
        state.content_width = state.client_width - 0.25;
        assert!(state.is_valid());
        assert!(state.vertical_overlay().is_some());
    }
}
