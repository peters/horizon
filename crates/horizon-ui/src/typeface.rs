//! The weights of the text of the interface.
//!
//! The bundled Inter is one variable face: a single file carries every weight,
//! and the rasteriser draws a weight only when the layout asks for its `wght`
//! coordinate. [`Weight`] is the one place that names the four weights of the
//! mockups and the coordinate of each, so no call site writes a number.
//!
//! `egui::RichText::strong` is not a weight in this toolkit version: it paints
//! the same glyphs in the strong colour of the visuals. Text that must look
//! heavier goes through [`format`] or [`rich`] instead.

use egui::epaint::text::VariationCoords;
use egui::text::TextFormat;
use egui::{Color32, FontId, RichText};

/// A weight of the interface, and the `wght` coordinate that draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weight {
    /// Body text and secondary lines.
    Regular,
    /// A label that stands a little above its neighbours.
    Medium,
    /// A row title, a primary button label and a focused title bar.
    Semibold,
    /// The heading of a card and the title of a tile.
    Bold,
    /// The heading of the first start page, and a caption.
    Black,
}

impl Weight {
    /// The `wght` axis value of this weight.
    pub(crate) const fn wght(self) -> f32 {
        match self {
            Self::Regular => 400.0,
            Self::Medium => 500.0,
            Self::Semibold => 600.0,
            Self::Bold => 650.0,
            Self::Black => 700.0,
        }
    }

    /// The variation coordinates that ask a variable face for this weight.
    pub(crate) fn coords(self) -> VariationCoords {
        VariationCoords::new([(b"wght", self.wght())])
    }
}

/// The text format of `font` in `color` at `weight`.
pub(crate) fn format(font: &FontId, color: Color32, weight: Weight) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color,
        coords: weight.coords(),
        ..TextFormat::default()
    }
}

/// A stock-widget text in `color` at `weight`, for the widgets that take a
/// [`RichText`] instead of a layout job.
pub(crate) fn rich(text: impl Into<String>, size: f32, color: Color32, weight: Weight) -> RichText {
    RichText::new(text)
        .size(size)
        .color(color)
        .variation(b"wght", weight.wght())
}

#[cfg(test)]
mod tests {
    use egui::text::LayoutJob;
    use egui::{Color32, FontId};

    use super::{Weight, format};
    use crate::app::bootstrap::configure_fonts;
    use crate::test_egui::DiscardTextures as _;

    /// Lay `text` out at `weight` in the bundled Inter.
    fn layout(ctx: &egui::Context, text: &str, weight: Weight) -> std::sync::Arc<egui::Galley> {
        let job = LayoutJob::single_section(
            text.to_owned(),
            format(&FontId::proportional(16.0), Color32::WHITE, weight),
        );
        let mut galley = None;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                galley = Some(ui.painter().layout_job(job.clone()));
            })
            .discard_textures();
        galley.expect("one frame")
    }

    #[test]
    fn every_weight_has_its_own_wght_coordinate() {
        let wghts: Vec<f32> = [
            Weight::Regular,
            Weight::Medium,
            Weight::Semibold,
            Weight::Bold,
            Weight::Black,
        ]
        .iter()
        .map(|weight| weight.wght())
        .collect();
        assert_eq!(wghts, vec![400.0, 500.0, 600.0, 650.0, 700.0]);
        let coords = Weight::Semibold.coords();
        let axes = coords.as_ref();
        assert_eq!(axes.len(), 1);
        assert_eq!(&axes[0].0.to_be_bytes(), b"wght");
        assert!((axes[0].1 - 600.0).abs() < f32::EPSILON);
    }

    /// The weight reaches the rasteriser, and not only the layout: the two
    /// galleys cover a different amount of ink and use different glyph
    /// pictures of the atlas.
    #[test]
    fn a_heavier_weight_draws_other_glyph_pictures() {
        let ctx = egui::Context::default();
        ctx.set_fonts(configure_fonts());
        let regular = layout(&ctx, "Hamburgefonstiv", Weight::Regular);
        let bold = layout(&ctx, "Hamburgefonstiv", Weight::Black);

        let regular_ink = regular.rows[0].visuals.mesh_bounds;
        let bold_ink = bold.rows[0].visuals.mesh_bounds;
        assert!(
            bold_ink.width() > regular_ink.width() + 0.5,
            "the bold glyphs should cover more width: {} against {}",
            bold_ink.width(),
            regular_ink.width()
        );
        let regular_picture = regular.rows[0].row.glyphs[1].uv_rect;
        let bold_picture = bold.rows[0].row.glyphs[1].uv_rect;
        assert_ne!(
            regular_picture.min, bold_picture.min,
            "the two weights should be rasterised apart, not share one atlas slot"
        );
    }
}
