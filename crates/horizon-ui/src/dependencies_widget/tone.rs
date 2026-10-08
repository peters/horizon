//! Theme colors for the states the maintenance model describes.

use egui::Color32;
use horizon_core::maintenance::portfolio::Tone;

use crate::theme;

pub(super) fn color(tone: Tone) -> Color32 {
    match tone {
        Tone::Good => theme::PALETTE_GREEN(),
        Tone::Active => theme::ACCENT(),
        Tone::Warning => theme::PALETTE_YELLOW(),
        Tone::Danger => theme::PALETTE_RED(),
        Tone::Neutral => theme::FG_SOFT(),
        Tone::Quiet => theme::FG_DIM(),
        Tone::Muted => theme::BORDER_STRONG(),
    }
}
