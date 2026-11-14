//! A pane of a canvas panel that draws a widget at the size of the screen.
//!
//! A canvas panel draws its content in a layer under the scale transform of the
//! canvas zoom. A stock widget lays its text out at the font of zoom 1, and the
//! transform then stretches a finished picture of every letter, so the text of
//! that widget blurs at every zoom other than 100 percent. A pane of this module
//! is a layer of its own, directly above the layer of its panel, whose transform
//! is the identity: a widget inside it lays its text out at the size that the
//! text has on the screen, while the geometry of the pane is the geometry of
//! zoom 1 times the zoom, so the words break where they break at 100 percent.

use std::hash::Hash;

use egui::{Id, Rect, Response, Ui, UiBuilder, emath::TSTransform};

/// The zoom of the canvas that `ui` draws in, while that zoom is not 100 percent.
pub(crate) fn canvas_scale(ui: &Ui) -> Option<f32> {
    let scale = screen_transform(ui).scaling;
    ((scale - 1.0).abs() > 1.0e-3 && scale > 0.0).then_some(scale)
}

/// The transform that maps a point of `ui` to the screen.
fn screen_transform(ui: &Ui) -> TSTransform {
    ui.ctx()
        .layer_transform_to_global(ui.layer_id())
        .unwrap_or(TSTransform::IDENTITY)
}

/// A pane over the whole of `ui` that the canvas zoom does not transform.
pub(crate) fn pane(ui: &Ui, id_source: impl Hash + std::fmt::Debug, scale: f32) -> Ui {
    let rect = screen_transform(ui).mul_rect(ui.max_rect());
    pane_over(ui, id_source, scale, rect)
}

/// A pane that covers `rect` of `ui` and that the canvas zoom does not transform,
/// with the style of a layout that is `scale` times the layout of zoom 1.
pub(crate) fn pane_over(ui: &Ui, id_source: impl Hash + std::fmt::Debug, scale: f32, rect: Rect) -> Ui {
    let ctx = ui.ctx().clone();
    let parent = ui.layer_id();
    let layer = egui::LayerId::new(parent.order, Id::unique(("screen_pane", &id_source)));
    ctx.set_sublayer(parent, layer);
    ctx.set_transform_layer(layer, TSTransform::IDENTITY);
    let builder = UiBuilder::new().layer_id(layer).max_rect(rect).layout(*ui.layout());
    let mut pane = Ui::new(ctx, Id::unique(("screen_pane_ui", &id_source)), builder);
    pane.set_clip_rect(screen_transform(ui).mul_rect(ui.clip_rect()));
    scale_style(&mut pane, scale);
    pane
}

/// The style of `ui` for a layout that is `scale` times the layout of zoom 1.
///
/// Every font, every gap and the scrollbar grow by the same factor, so the words
/// of a paragraph break where they break at zoom 1 and a person reads the same
/// lines at every zoom.
pub(crate) fn scale_style(ui: &mut Ui, scale: f32) {
    let style = ui.style_mut();
    for font in style.text_styles.values_mut() {
        font.size *= scale;
    }
    style.spacing.item_spacing *= scale;
    style.spacing.button_padding *= scale;
    style.spacing.indent *= scale;
    style.spacing.interact_size *= scale;
    style.spacing.scroll.bar_width *= scale;
    style.spacing.scroll.bar_inner_margin *= scale;
    style.spacing.scroll.bar_outer_margin *= scale;
}

/// Adds the widget that `add` draws at the size of the screen of the canvas zoom
/// of `ui`, and takes the place of that widget in the layout of `ui`.
///
/// The widget takes the place of the layout of zoom 1 times the zoom, and it
/// draws that place in a pane, so the row of the panel keeps its place, its size
/// and its line at every zoom while the letters of the widget stay sharp. At a
/// zoom of 100 percent `add` draws in `ui` itself, and the plain path runs
/// unchanged.
pub(crate) fn add_widget(
    ui: &mut Ui,
    id_source: impl Hash + std::fmt::Debug,
    mut add: impl FnMut(&mut Ui) -> Response,
) -> Response {
    let Some(scale) = canvas_scale(ui) else {
        return add(ui);
    };
    let size = measured_size(ui, &id_source, scale, &mut add);
    let (_, placed) = ui.allocate_space(size / scale);
    let mut pane = pane_over(ui, &id_source, scale, screen_transform(ui).mul_rect(placed));
    add(&mut pane)
}

/// The size in screen points that `add` takes in a pane of the screen size.
///
/// The widget runs without a pointer and without a place of its own first,
/// because the layout of the panel needs its size before the pane can take that
/// size and draw it.
fn measured_size(
    ui: &Ui,
    id_source: &(impl Hash + std::fmt::Debug),
    scale: f32,
    add: &mut impl FnMut(&mut Ui) -> Response,
) -> egui::Vec2 {
    let rect = screen_transform(ui).mul_rect(ui.max_rect());
    let mut scratch = pane_over(
        ui,
        ("measure", id_source),
        scale,
        Rect::from_min_size(egui::Pos2::new(-1.0e5, -1.0e5), rect.size()),
    );
    // A measured widget reads no pointer, takes no focus and paints nothing.
    scratch.disable();
    scratch.set_clip_rect(Rect::NOTHING);
    let response = add(&mut scratch);
    response.rect.size()
}
