//! Shared helpers for headless egui test passes.

/// Clears the textures delta from a pass output that no renderer will consume.
///
/// egui's `TexturesDelta` debug-asserts on drop when deltas were never applied;
/// headless tests never paint, so the delta must be discarded explicitly.
pub(crate) trait DiscardTextures {
    #[must_use]
    fn discard_textures(self) -> Self;
}

impl DiscardTextures for egui::FullOutput {
    fn discard_textures(mut self) -> Self {
        self.textures_delta.clear();
        self
    }
}

/// Labels and disabled flags from one headless pass with AccessKit enabled.
#[cfg(test)]
pub(crate) fn accesskit_labels(draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, bool)> {
    accesskit_nodes(draw, egui::accesskit::Node::label)
}

#[cfg(test)]
fn accesskit_nodes(
    draw: impl FnMut(&mut egui::Ui),
    text: impl Fn(&egui::accesskit::Node) -> Option<&str>,
) -> Vec<(String, bool)> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 160.0))),
                ..egui::RawInput::default()
            },
            draw,
        )
        .discard_textures();
    let update = output.platform_output.accesskit_update.expect("accesskit tree");
    update
        .nodes
        .into_iter()
        .filter_map(|(_, node)| Some((text(&node)?.to_owned(), node.is_disabled())))
        .collect()
}

/// The theme that the thread of a test draws in, while it holds one.
///
/// The theme of the process is one value for every thread, and a test that
/// paints a card can take tens of milliseconds. A test that writes that value
/// while another test paints changes the palette under it, so a theme test
/// keeps its own palette on its own thread instead.
mod palette {
    use std::cell::Cell;

    use crate::theme::ResolvedTheme;

    thread_local! {
        static PALETTE: Cell<Option<ResolvedTheme>> = const { Cell::new(None) };
    }

    /// Holds the palette of `theme` on this thread until it is dropped.
    pub(crate) struct ThemeOverride(Option<ResolvedTheme>);

    impl Drop for ThemeOverride {
        fn drop(&mut self) {
            PALETTE.with(|palette| palette.set(self.0));
        }
    }

    /// Draws as if the process used `theme`, on this thread alone.
    pub(crate) fn with_theme(theme: ResolvedTheme) -> ThemeOverride {
        ThemeOverride(PALETTE.with(|palette| palette.replace(Some(theme))))
    }

    /// The theme that this thread draws in, while it holds one.
    pub(crate) fn theme_override() -> Option<ResolvedTheme> {
        PALETTE.with(Cell::get)
    }
}

pub(crate) use palette::{theme_override, with_theme};

/// One pass in which a canvas layer at `zoom` is the layer of `add`, so a
/// painter inside it reads the scale of the layer as the frame does.
#[cfg(test)]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn with_canvas_layer<R>(ctx: &egui::Context, zoom: f32, add: impl FnOnce(&egui::Ui) -> R) -> Option<R> {
    with_canvas_transform(ctx, egui::emath::TSTransform::from_scaling(zoom), add)
}

/// [`with_canvas_layer`] with the whole transform of the canvas: the zoom and
/// the pan offset that moves it.
#[cfg(test)]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn with_canvas_transform<R>(
    ctx: &egui::Context,
    transform: egui::emath::TSTransform,
    add: impl FnOnce(&egui::Ui) -> R,
) -> Option<R> {
    let mut answer = None;
    let mut add = Some(add);
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("canvas"));
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().set_transform_layer(layer, transform);
            let layer_ui = egui::UiBuilder::new()
                .layer_id(layer)
                .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)));
            let child = egui::Ui::new(ui.ctx().clone(), egui::Id::unique("canvas ui"), layer_ui);
            answer = add.take().map(|add| add(&child));
        })
        .discard_textures();
    answer
}
