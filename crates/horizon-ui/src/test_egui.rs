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
