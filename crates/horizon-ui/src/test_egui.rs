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
    accesskit_nodes(1, draw, egui::accesskit::Node::label)
}

/// What a screen reader reads from each node of one headless pass: its label, or the
/// value that egui gives a text label instead. Each comes with its disabled flag.
#[cfg(test)]
pub(crate) fn accesskit_texts(draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, bool)> {
    accesskit_nodes(1, draw, |node| node.label().or_else(|| node.value()))
}

/// As [`accesskit_texts`], from the second pass: a window or modal draws its first
/// pass invisible to size itself, and its widgets are disabled in it.
#[cfg(test)]
pub(crate) fn accesskit_texts_after_sizing(draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, bool)> {
    accesskit_nodes(2, draw, |node| node.label().or_else(|| node.value()))
}

#[cfg(test)]
fn accesskit_nodes(
    passes: usize,
    mut draw: impl FnMut(&mut egui::Ui),
    text: impl Fn(&egui::accesskit::Node) -> Option<&str>,
) -> Vec<(String, bool)> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut output = None;
    for _ in 0..passes {
        output = Some(
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 160.0))),
                    ..egui::RawInput::default()
                },
                &mut draw,
            )
            .discard_textures(),
        );
    }
    let output = output.expect("at least one pass");
    let update = output.platform_output.accesskit_update.expect("accesskit tree");
    update
        .nodes
        .into_iter()
        .filter_map(|(_, node)| Some((text(&node)?.to_owned(), node.is_disabled())))
        .collect()
}
