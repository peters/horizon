use serde::{Deserialize, Serialize};

pub const DEFAULT_CANVAS_ZOOM: f32 = 1.0;
pub const MIN_CANVAS_ZOOM: f32 = 0.25;
pub const MAX_CANVAS_ZOOM: f32 = 4.0;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct CanvasViewState {
    pub pan_offset: [f32; 2],
    pub zoom: f32,
}

impl CanvasViewState {
    #[must_use]
    pub fn new(pan_offset: [f32; 2], zoom: f32) -> Self {
        Self {
            pan_offset,
            zoom: clamp_canvas_zoom(zoom),
        }
    }

    #[must_use]
    pub fn from_legacy_pan_offset(pan_offset: [f32; 2]) -> Self {
        Self::new(pan_offset, DEFAULT_CANVAS_ZOOM)
    }

    #[must_use]
    pub fn clamped(self) -> Self {
        Self::new(self.pan_offset, self.zoom)
    }

    #[must_use]
    pub fn is_identity(self) -> bool {
        (self.pan_offset[0]).abs() <= f32::EPSILON
            && (self.pan_offset[1]).abs() <= f32::EPSILON
            && (self.zoom - DEFAULT_CANVAS_ZOOM).abs() <= f32::EPSILON
    }

    #[must_use]
    pub fn canvas_to_screen(self, canvas_origin: [f32; 2], canvas_point: [f32; 2]) -> [f32; 2] {
        [
            canvas_origin[0] + self.pan_offset[0] + canvas_point[0] * self.zoom,
            canvas_origin[1] + self.pan_offset[1] + canvas_point[1] * self.zoom,
        ]
    }

    #[must_use]
    pub fn screen_to_canvas(self, canvas_origin: [f32; 2], screen_point: [f32; 2]) -> [f32; 2] {
        [
            (screen_point[0] - canvas_origin[0] - self.pan_offset[0]) / self.zoom,
            (screen_point[1] - canvas_origin[1] - self.pan_offset[1]) / self.zoom,
        ]
    }

    #[must_use]
    pub fn canvas_size_to_screen(self, canvas_size: [f32; 2]) -> [f32; 2] {
        [canvas_size[0] * self.zoom, canvas_size[1] * self.zoom]
    }

    #[must_use]
    pub fn screen_delta_to_canvas(self, screen_delta: [f32; 2]) -> [f32; 2] {
        [screen_delta[0] / self.zoom, screen_delta[1] / self.zoom]
    }

    pub fn set_pan_offset(&mut self, pan_offset: [f32; 2]) {
        self.pan_offset = pan_offset;
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = clamp_canvas_zoom(zoom);
    }

    pub fn align_canvas_point_to_screen(
        &mut self,
        canvas_origin: [f32; 2],
        canvas_point: [f32; 2],
        screen_anchor: [f32; 2],
    ) {
        self.pan_offset = [
            screen_anchor[0] - canvas_origin[0] - canvas_point[0] * self.zoom,
            screen_anchor[1] - canvas_origin[1] - canvas_point[1] * self.zoom,
        ];
    }

    pub fn zoom_about_screen_anchor(&mut self, canvas_origin: [f32; 2], screen_anchor: [f32; 2], zoom: f32) {
        let canvas_point = self.screen_to_canvas(canvas_origin, screen_anchor);
        self.zoom = clamp_canvas_zoom(zoom);
        self.align_canvas_point_to_screen(canvas_origin, canvas_point, screen_anchor);
    }
}

impl Default for CanvasViewState {
    fn default() -> Self {
        Self {
            pan_offset: [0.0, 0.0],
            zoom: DEFAULT_CANVAS_ZOOM,
        }
    }
}

#[must_use]
pub fn clamp_canvas_zoom(zoom: f32) -> f32 {
    if !zoom.is_finite() {
        return DEFAULT_CANVAS_ZOOM;
    }

    zoom.clamp(MIN_CANVAS_ZOOM, MAX_CANVAS_ZOOM)
}

#[cfg(test)]
mod tests {
    use super::{CanvasViewState, DEFAULT_CANVAS_ZOOM, MAX_CANVAS_ZOOM, MIN_CANVAS_ZOOM, clamp_canvas_zoom};

    #[test]
    fn clamps_zoom_range_and_non_finite_values() {
        assert!((clamp_canvas_zoom(f32::NAN) - DEFAULT_CANVAS_ZOOM).abs() <= f32::EPSILON);
        assert!((clamp_canvas_zoom(f32::INFINITY) - DEFAULT_CANVAS_ZOOM).abs() <= f32::EPSILON);
        assert!((clamp_canvas_zoom(0.01) - MIN_CANVAS_ZOOM).abs() <= f32::EPSILON);
        assert!((clamp_canvas_zoom(10.0) - MAX_CANVAS_ZOOM).abs() <= f32::EPSILON);
    }

    #[test]
    fn screen_and_canvas_points_round_trip() {
        let view = CanvasViewState::new([36.0, -24.0], 1.75);
        let canvas_origin = [210.0, 46.0];
        let canvas_point = [480.0, 320.0];

        let screen_point = view.canvas_to_screen(canvas_origin, canvas_point);
        let round_trip = view.screen_to_canvas(canvas_origin, screen_point);

        assert!((round_trip[0] - canvas_point[0]).abs() <= f32::EPSILON);
        assert!((round_trip[1] - canvas_point[1]).abs() <= f32::EPSILON);
    }

    #[test]
    fn zoom_about_anchor_preserves_canvas_point_under_cursor() {
        let mut view = CanvasViewState::new([120.0, -36.0], 1.25);
        let canvas_origin = [210.0, 46.0];
        let anchor = [860.0, 420.0];
        let anchored_point = view.screen_to_canvas(canvas_origin, anchor);

        view.zoom_about_screen_anchor(canvas_origin, anchor, 2.5);

        let screen_after = view.canvas_to_screen(canvas_origin, anchored_point);
        assert!((screen_after[0] - anchor[0]).abs() <= f32::EPSILON);
        assert!((screen_after[1] - anchor[1]).abs() <= f32::EPSILON);
    }

    #[test]
    fn align_canvas_point_moves_point_to_requested_screen_anchor() {
        let mut view = CanvasViewState::new([0.0, 0.0], 2.0);
        let canvas_origin = [210.0, 46.0];
        let canvas_point = [320.0, 180.0];
        let screen_anchor = [420.0, 260.0];

        view.align_canvas_point_to_screen(canvas_origin, canvas_point, screen_anchor);

        let mapped = view.canvas_to_screen(canvas_origin, canvas_point);
        assert!((mapped[0] - screen_anchor[0]).abs() <= f32::EPSILON);
        assert!((mapped[1] - screen_anchor[1]).abs() <= f32::EPSILON);
    }

    #[test]
    fn screen_delta_scales_back_to_canvas_delta() {
        let view = CanvasViewState::new([0.0, 0.0], 2.5);
        let canvas_delta = view.screen_delta_to_canvas([50.0, -25.0]);

        assert!((canvas_delta[0] - 20.0).abs() <= f32::EPSILON);
        assert!((canvas_delta[1] + 10.0).abs() <= f32::EPSILON);
    }
}
