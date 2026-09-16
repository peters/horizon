//! The mapping between a browser panel's layout size and the emulated CSS
//! viewport the render path drives.
//!
//! The render path sends the browser backend whatever rect the panel body has
//! left after the titlebar, the body padding, the browser chrome row, and the
//! item spacing before the body (`synchronize_viewport` on
//! `render::show_body`'s `available_rect_before_wrap`). [`VIEWPORT_CHROME`]
//! is the exact complement of that layout, built from the same constants the
//! layout uses, so a size requested through `browser_create` or
//! `browser_resize` can be converted to the panel size that renders it — and
//! back, for the viewport stamp the host writes on the manifest.
//!
//! `rendered_viewport_matches_the_chrome_offset` re-renders the panel body
//! through the real chrome with the app's embedded fonts and fails if the
//! offset ever drifts from the render path.

use super::{PANEL_PADDING, PANEL_TITLEBAR_HEIGHT};
use crate::browser_widget::chrome::CHROME_ROW_HEIGHT;
use crate::theme::ITEM_SPACING;

/// Panel chrome between the panel rect and the emulated viewport: the two
/// body gutters on the width axis; the titlebar, the two gutters, the browser
/// chrome row, and one item spacing on the height axis.
pub(super) const VIEWPORT_CHROME: [f32; 2] = [
    2.0 * PANEL_PADDING,
    PANEL_TITLEBAR_HEIGHT + 2.0 * PANEL_PADDING + CHROME_ROW_HEIGHT + ITEM_SPACING.y,
];

/// The panel layout size whose rendered browser viewport is `viewport`
/// (CSS pixels per axis).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "requested viewports are bounded to 8000 CSS pixels by the MCP controller, below f32's exact integer range"
)]
pub(super) fn panel_size_for_viewport(viewport: [u32; 2]) -> [f32; 2] {
    [
        viewport[0] as f32 + VIEWPORT_CHROME[0],
        viewport[1] as f32 + VIEWPORT_CHROME[1],
    ]
}

/// The emulated CSS viewport a panel at layout `size` renders.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "board layout sizes are non-negative CSS pixels, far below u32::MAX"
)]
pub(super) fn viewport_size_from_panel_size(size: [f32; 2]) -> [u32; 2] {
    [
        (size[0] - VIEWPORT_CHROME[0]).max(0.0).round() as u32,
        (size[1] - VIEWPORT_CHROME[1]).max(0.0).round() as u32,
    ]
}

#[cfg(test)]
mod tests {
    use super::super::panels::PanelFrame;
    use super::*;
    use crate::browser_widget::BrowserUiState;
    use crate::test_egui::DiscardTextures;
    use egui::{Align, Layout, Rect, UiBuilder, vec2};
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{AppearanceTheme, PanelId};

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "16.0 and 92.0 are exactly representable, so an equality pin is the point"
    )]
    fn the_viewport_chrome_is_the_measured_panel_chrome() {
        // Width: the two body gutters. Height: titlebar, gutters, chrome row,
        // item spacing. The render test below proves the render path consumes
        // exactly this.
        assert_eq!(VIEWPORT_CHROME, [16.0, 92.0]);
    }

    #[test]
    fn panel_and_viewport_sizes_are_exact_inverses() {
        for viewport in [[320, 320], [375, 812], [1280, 800], [1920, 1080], [8000, 8000]] {
            assert_eq!(
                viewport_size_from_panel_size(panel_size_for_viewport(viewport)),
                viewport,
                "sizing a panel for {viewport:?} must render exactly that viewport"
            );
        }
        assert_eq!(viewport_size_from_panel_size([10.0, 10.0]), [0, 0]);
        assert_eq!(viewport_size_from_panel_size([1295.6, 891.6]), [1280, 800]);
    }

    /// Drift guard: render the real panel body (`PanelFrame` body rect, body
    /// scope, real chrome strip) with the app's embedded fonts and theme, and
    /// check that the rect left for the page body is exactly the viewport the
    /// geometry predicts. Any chrome layout change that moves this fails here
    /// instead of silently shifting every agent-reported viewport.
    #[test]
    fn rendered_viewport_matches_the_chrome_offset() {
        let ctx = egui::Context::default();
        ctx.set_fonts(super::super::bootstrap::configure_fonts());
        crate::theme::apply(&ctx, AppearanceTheme::Dark);
        let mut browser = BrowserPanelState::inert();
        let mut ui_state = BrowserUiState::default();
        let panel_size = [1280.0, 800.0];
        let panel_rect = Rect::from_min_size(egui::Pos2::ZERO, vec2(panel_size[0], panel_size[1]));
        let body = PanelFrame::new(panel_rect).body;
        let mut measured_size = egui::vec2(f32::NAN, f32::NAN);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.scope_builder(
                UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)),
                |body_ui| {
                    crate::browser_widget::chrome::show(body_ui, PanelId(1), &mut browser, &mut ui_state, true);
                    measured_size = body_ui.available_rect_before_wrap().size();
                },
            );
        });
        let _ = output.discard_textures();
        let expected = [panel_size[0] - VIEWPORT_CHROME[0], panel_size[1] - VIEWPORT_CHROME[1]];
        assert!(
            (measured_size.x - expected[0]).abs() <= 0.5 && (measured_size.y - expected[1]).abs() <= 0.5,
            "rendered viewport {measured_size:?} drifted from the predicted {expected:?}: update VIEWPORT_CHROME and the chrome constants together"
        );
    }
}
