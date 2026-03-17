use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as TerminalColor, NamedColor, Rgb};
use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, Vec2, Visuals};

pub const BG: Color32 = Color32::from_rgb(7, 10, 16);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(12, 16, 24);
pub const PANEL_BG: Color32 = Color32::from_rgb(15, 19, 28);
pub const PANEL_BG_ALT: Color32 = Color32::from_rgb(21, 26, 37);
pub const FG: Color32 = Color32::from_rgb(224, 230, 241);
pub const FG_SOFT: Color32 = Color32::from_rgb(170, 181, 199);
pub const FG_DIM: Color32 = Color32::from_rgb(108, 120, 142);
pub const CURSOR: Color32 = Color32::from_rgb(196, 223, 255);
pub const GRID_DOT: Color32 = Color32::from_rgb(28, 34, 46);
pub const ACCENT: Color32 = Color32::from_rgb(116, 162, 247);
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(37, 46, 61);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(63, 78, 101);
pub const TITLEBAR_BG: Color32 = Color32::from_rgb(8, 11, 17);
pub const TOOLBAR_BG: Color32 = Color32::from_rgb(11, 15, 22);
pub const CANVAS_COOL_GLOW: Color32 = Color32::from_rgba_unmultiplied_const(77, 112, 220, 20);
pub const CANVAS_WARM_GLOW: Color32 = Color32::from_rgba_unmultiplied_const(255, 146, 80, 28);

pub const BTN_CLOSE: Color32 = Color32::from_rgb(235, 96, 88);
pub const PALETTE_GREEN: Color32 = Color32::from_rgb(166, 227, 161);
pub const PALETTE_RED: Color32 = Color32::from_rgb(243, 139, 168);

const PALETTE: [Color32; 16] = [
    Color32::from_rgb(45, 49, 62),
    Color32::from_rgb(227, 107, 117),
    Color32::from_rgb(143, 213, 130),
    Color32::from_rgb(233, 190, 109),
    Color32::from_rgb(116, 162, 247),
    Color32::from_rgb(202, 151, 234),
    Color32::from_rgb(102, 212, 214),
    Color32::from_rgb(196, 204, 219),
    Color32::from_rgb(74, 80, 97),
    Color32::from_rgb(242, 130, 135),
    Color32::from_rgb(170, 224, 158),
    Color32::from_rgb(244, 207, 133),
    Color32::from_rgb(147, 187, 255),
    Color32::from_rgb(224, 178, 247),
    Color32::from_rgb(141, 225, 227),
    Color32::from_rgb(231, 236, 245),
];

pub fn apply(ctx: &egui::Context) {
    let mut style = Style::default();

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.window_margin = Margin::same(0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.visuals = visuals();

    ctx.set_style(style);
}

pub fn blend(base: Color32, tint: Color32, tint_amount: f32) -> Color32 {
    let amount = tint_amount.clamp(0.0, 1.0);
    let keep = 1.0 - amount;

    Color32::from_rgb(
        blend_channel(base.r(), tint.r(), keep, amount),
        blend_channel(base.g(), tint.g(), keep, amount),
        blend_channel(base.b(), tint.b(), keep, amount),
    )
}

pub fn alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn panel_border(accent: Color32, focused: bool) -> Color32 {
    if focused {
        blend(BORDER_STRONG, accent, 0.78)
    } else {
        alpha(blend(BORDER_SUBTLE, accent, 0.32), 196)
    }
}

pub fn terminal_color_to_egui(color: TerminalColor, colors: &Colors) -> Color32 {
    match color {
        TerminalColor::Named(name) => colors[name].map_or_else(|| named_color_to_egui(name), rgb_to_egui),
        TerminalColor::Spec(rgb) => rgb_to_egui(rgb),
        TerminalColor::Indexed(index) => palette_color(index, colors),
    }
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();

    visuals.window_corner_radius = CornerRadius::same(16);
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 2,
        color: Color32::from_black_alpha(128),
    };
    visuals.window_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.window_fill = PANEL_BG;
    visuals.window_highlight_topmost = false;

    visuals.panel_fill = TOOLBAR_BG;
    visuals.faint_bg_color = PANEL_BG_ALT;
    visuals.extreme_bg_color = BG;
    visuals.code_bg_color = BG_ELEVATED;
    visuals.override_text_color = Some(FG);
    visuals.resize_corner_size = 15.0;

    visuals.widgets.noninteractive.bg_fill = BG_ELEVATED;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG_ALT;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG_DIM);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(12);

    visuals.widgets.inactive.bg_fill = PANEL_BG_ALT;
    visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, FG_SOFT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(12);

    visuals.widgets.hovered.bg_fill = blend(PANEL_BG_ALT, ACCENT, 0.16);
    visuals.widgets.hovered.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, FG);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(12);

    visuals.widgets.active.bg_fill = blend(PANEL_BG_ALT, ACCENT, 0.22);
    visuals.widgets.active.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, FG);
    visuals.widgets.active.corner_radius = CornerRadius::same(12);

    visuals.selection.bg_fill = alpha(ACCENT, 54);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(118),
    };

    visuals
}

fn blend_channel(base: u8, tint: u8, keep: f32, amount: f32) -> u8 {
    let mixed = ((f32::from(base) * keep) + (f32::from(tint) * amount))
        .round()
        .clamp(0.0, 255.0);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        mixed as u8
    }
}

fn named_color_to_egui(color: NamedColor) -> Color32 {
    match color {
        NamedColor::Black => PALETTE[0],
        NamedColor::Red => PALETTE[1],
        NamedColor::Green => PALETTE[2],
        NamedColor::Yellow => PALETTE[3],
        NamedColor::Blue => PALETTE[4],
        NamedColor::Magenta => PALETTE[5],
        NamedColor::Cyan => PALETTE[6],
        NamedColor::White => PALETTE[7],
        NamedColor::BrightBlack => PALETTE[8],
        NamedColor::BrightRed => PALETTE[9],
        NamedColor::BrightGreen => PALETTE[10],
        NamedColor::BrightYellow => PALETTE[11],
        NamedColor::BrightBlue => PALETTE[12],
        NamedColor::BrightMagenta => PALETTE[13],
        NamedColor::BrightCyan => PALETTE[14],
        NamedColor::BrightWhite => PALETTE[15],
        NamedColor::Foreground | NamedColor::BrightForeground => FG,
        NamedColor::Background => PANEL_BG,
        NamedColor::Cursor => CURSOR,
        NamedColor::DimForeground => alpha(FG_SOFT, 196),
        NamedColor::DimBlack => alpha(PALETTE[0], 196),
        NamedColor::DimRed => alpha(PALETTE[1], 196),
        NamedColor::DimGreen => alpha(PALETTE[2], 196),
        NamedColor::DimYellow => alpha(PALETTE[3], 196),
        NamedColor::DimBlue => alpha(PALETTE[4], 196),
        NamedColor::DimMagenta => alpha(PALETTE[5], 196),
        NamedColor::DimCyan => alpha(PALETTE[6], 196),
        NamedColor::DimWhite => alpha(PALETTE[7], 196),
    }
}

fn palette_color(index: u8, colors: &Colors) -> Color32 {
    let index = usize::from(index);
    if index < PALETTE.len() {
        colors[index].map_or(PALETTE[index], rgb_to_egui)
    } else if let Some(rgb) = colors[index] {
        rgb_to_egui(rgb)
    } else if index < 232 {
        let idx = index - 16;
        let steps = [0_u8, 95, 135, 175, 215, 255];
        Color32::from_rgb(steps[idx / 36], steps[(idx % 36) / 6], steps[idx % 6])
    } else {
        let value = 8 + ((index - 232) * 10);
        let value = u8::try_from(value).unwrap_or(u8::MAX);
        Color32::from_rgb(value, value, value)
    }
}

fn rgb_to_egui(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

#[cfg(test)]
mod tests {
    use super::alpha;
    use egui::Color32;

    #[test]
    fn alpha_preserves_unmultiplied_color_channels() {
        assert_eq!(
            alpha(Color32::from_rgb(116, 162, 247), 40),
            Color32::from_rgba_unmultiplied(116, 162, 247, 40),
        );
    }
}
