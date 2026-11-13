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

/// One pass in which a canvas layer at `zoom` is the layer of `add`, so a
/// painter inside it reads the scale of the layer as the frame does.
#[cfg(test)]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn with_canvas_layer<R>(ctx: &egui::Context, zoom: f32, add: impl FnOnce(&egui::Ui) -> R) -> Option<R> {
    let mut answer = None;
    let mut add = Some(add);
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("canvas"));
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            ui.ctx()
                .set_transform_layer(layer, egui::emath::TSTransform::from_scaling(zoom));
            let layer_ui = egui::UiBuilder::new()
                .layer_id(layer)
                .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)));
            let child = egui::Ui::new(ui.ctx().clone(), egui::Id::unique("canvas ui"), layer_ui);
            answer = add.take().map(|add| add(&child));
        })
        .discard_textures();
    answer
}
