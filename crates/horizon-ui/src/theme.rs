#![allow(non_snake_case)]

use std::sync::atomic::{AtomicU8, Ordering};

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as TerminalColor, NamedColor, Rgb};
use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, Theme, Vec2, Visuals};
use horizon_core::AppearanceTheme;

struct ThemePalette {
    bg: Color32,
    bg_elevated: Color32,
    panel_bg: Color32,
    panel_bg_alt: Color32,
    fg: Color32,
    fg_soft: Color32,
    fg_dim: Color32,
    cursor: Color32,
    grid_dot: Color32,
    accent: Color32,
    border_subtle: Color32,
    border_strong: Color32,
    titlebar_bg: Color32,
    toolbar_bg: Color32,
    canvas_cool_glow: Color32,
    canvas_warm_glow: Color32,
    btn_close: Color32,
    palette_green: Color32,
    palette_red: Color32,
    palette_yellow: Color32,
    palette_cyan: Color32,
    terminal_palette: [Color32; 16],
}

const DARK_THEME: ThemePalette = ThemePalette {
    bg: Color32::from_rgb(7, 10, 16),
    bg_elevated: Color32::from_rgb(12, 16, 24),
    panel_bg: Color32::from_rgb(15, 19, 28),
    panel_bg_alt: Color32::from_rgb(21, 26, 37),
    fg: Color32::from_rgb(224, 230, 241),
    fg_soft: Color32::from_rgb(170, 181, 199),
    fg_dim: Color32::from_rgb(108, 120, 142),
    cursor: Color32::from_rgb(196, 223, 255),
    grid_dot: Color32::from_rgb(28, 34, 46),
    accent: Color32::from_rgb(116, 162, 247),
    border_subtle: Color32::from_rgb(37, 46, 61),
    border_strong: Color32::from_rgb(63, 78, 101),
    titlebar_bg: Color32::from_rgb(8, 11, 17),
    toolbar_bg: Color32::from_rgb(11, 15, 22),
    canvas_cool_glow: Color32::from_rgba_unmultiplied_const(77, 112, 220, 20),
    canvas_warm_glow: Color32::from_rgba_unmultiplied_const(255, 146, 80, 28),
    btn_close: Color32::from_rgb(235, 96, 88),
    palette_green: Color32::from_rgb(166, 227, 161),
    palette_red: Color32::from_rgb(243, 139, 168),
    palette_yellow: Color32::from_rgb(233, 190, 109),
    palette_cyan: Color32::from_rgb(102, 212, 214),
    terminal_palette: [
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
    ],
};

const LIGHT_THEME: ThemePalette = ThemePalette {
    bg: Color32::from_rgb(247, 244, 239),
    bg_elevated: Color32::from_rgb(253, 251, 247),
    panel_bg: Color32::from_rgb(254, 253, 250),
    panel_bg_alt: Color32::from_rgb(243, 240, 233),
    fg: Color32::from_rgb(24, 28, 34),
    fg_soft: Color32::from_rgb(77, 86, 96),
    fg_dim: Color32::from_rgb(131, 143, 154),
    cursor: Color32::from_rgb(94, 106, 210),
    grid_dot: Color32::from_rgb(222, 216, 204),
    accent: Color32::from_rgb(94, 106, 210),
    border_subtle: Color32::from_rgb(229, 224, 213),
    border_strong: Color32::from_rgb(202, 194, 177),
    titlebar_bg: Color32::from_rgb(248, 246, 240),
    toolbar_bg: Color32::from_rgb(248, 246, 240),
    canvas_cool_glow: Color32::from_rgba_unmultiplied_const(94, 106, 210, 14),
    canvas_warm_glow: Color32::from_rgba_unmultiplied_const(243, 158, 80, 22),
    btn_close: Color32::from_rgb(214, 67, 67),
    palette_green: Color32::from_rgb(5, 150, 105),
    palette_red: Color32::from_rgb(210, 15, 57),
    palette_yellow: Color32::from_rgb(217, 119, 6),
    palette_cyan: Color32::from_rgb(8, 145, 178),
    terminal_palette: [
        Color32::from_rgb(92, 95, 119),
        Color32::from_rgb(210, 15, 57),
        Color32::from_rgb(64, 160, 43),
        Color32::from_rgb(223, 142, 29),
        Color32::from_rgb(30, 102, 245),
        Color32::from_rgb(136, 57, 239),
        Color32::from_rgb(4, 165, 229),
        Color32::from_rgb(76, 79, 105),
        Color32::from_rgb(124, 127, 147),
        Color32::from_rgb(224, 108, 129),
        Color32::from_rgb(83, 168, 66),
        Color32::from_rgb(228, 160, 71),
        Color32::from_rgb(79, 135, 255),
        Color32::from_rgb(158, 84, 235),
        Color32::from_rgb(54, 180, 231),
        Color32::from_rgb(42, 47, 64),
    ],
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTheme {
    Dark,
    Light,
}

impl ResolvedTheme {
    #[must_use]
    const fn as_u8(self) -> u8 {
        match self {
            Self::Dark => 0,
            Self::Light => 1,
        }
    }

    #[must_use]
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Light,
            _ => Self::Dark,
        }
    }

    #[must_use]
    const fn from_egui(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self::Dark,
            Theme::Light => Self::Light,
        }
    }

    #[must_use]
    const fn to_egui(self) -> Theme {
        match self {
            Self::Dark => Theme::Dark,
            Self::Light => Theme::Light,
        }
    }
}

static CURRENT_THEME: AtomicU8 = AtomicU8::new(ResolvedTheme::Dark.as_u8());

pub fn apply(ctx: &egui::Context, preference: AppearanceTheme) -> ResolvedTheme {
    ctx.set_style_of(Theme::Dark, style_for(ResolvedTheme::Dark));
    ctx.set_style_of(Theme::Light, style_for(ResolvedTheme::Light));

    let resolved = resolve_theme(preference, ctx.system_theme());
    ctx.set_theme(resolved.to_egui());
    set_theme(resolved);
    resolved
}

pub fn set_theme(theme: ResolvedTheme) {
    CURRENT_THEME.store(theme.as_u8(), Ordering::Relaxed);
}

#[must_use]
pub fn current_theme() -> ResolvedTheme {
    ResolvedTheme::from_u8(CURRENT_THEME.load(Ordering::Relaxed))
}

#[must_use]
pub fn resolve_theme(preference: AppearanceTheme, system_theme: Option<Theme>) -> ResolvedTheme {
    match preference {
        AppearanceTheme::Auto => system_theme.map_or(ResolvedTheme::Dark, ResolvedTheme::from_egui),
        AppearanceTheme::Dark => ResolvedTheme::Dark,
        AppearanceTheme::Light => ResolvedTheme::Light,
    }
}

#[must_use]
pub fn bg_for(theme: ResolvedTheme) -> Color32 {
    palette_for(theme).bg
}

#[must_use]
pub fn BG() -> Color32 {
    active_palette().bg
}

#[must_use]
pub fn BG_ELEVATED() -> Color32 {
    active_palette().bg_elevated
}

#[must_use]
pub fn PANEL_BG() -> Color32 {
    active_palette().panel_bg
}

#[must_use]
pub fn PANEL_BG_ALT() -> Color32 {
    active_palette().panel_bg_alt
}

#[must_use]
pub fn FG() -> Color32 {
    active_palette().fg
}

#[must_use]
pub fn FG_SOFT() -> Color32 {
    active_palette().fg_soft
}

#[must_use]
pub fn FG_DIM() -> Color32 {
    active_palette().fg_dim
}

#[must_use]
pub fn CURSOR() -> Color32 {
    active_palette().cursor
}

#[must_use]
pub fn GRID_DOT() -> Color32 {
    active_palette().grid_dot
}

#[must_use]
pub fn ACCENT() -> Color32 {
    active_palette().accent
}

#[must_use]
pub fn BORDER_SUBTLE() -> Color32 {
    active_palette().border_subtle
}

#[must_use]
pub fn BORDER_STRONG() -> Color32 {
    active_palette().border_strong
}

#[must_use]
pub fn TITLEBAR_BG() -> Color32 {
    active_palette().titlebar_bg
}

#[must_use]
pub fn CANVAS_COOL_GLOW() -> Color32 {
    active_palette().canvas_cool_glow
}

#[must_use]
pub fn CANVAS_WARM_GLOW() -> Color32 {
    active_palette().canvas_warm_glow
}

#[must_use]
pub fn BTN_CLOSE() -> Color32 {
    active_palette().btn_close
}

#[must_use]
pub fn PALETTE_GREEN() -> Color32 {
    active_palette().palette_green
}

#[must_use]
pub fn PALETTE_RED() -> Color32 {
    active_palette().palette_red
}

#[must_use]
pub fn PALETTE_YELLOW() -> Color32 {
    active_palette().palette_yellow
}

#[must_use]
pub fn PALETTE_CYAN() -> Color32 {
    active_palette().palette_cyan
}

#[must_use]
pub fn blend(base: Color32, tint: Color32, tint_amount: f32) -> Color32 {
    let amount = tint_amount.clamp(0.0, 1.0);
    let keep = 1.0 - amount;

    Color32::from_rgb(
        blend_channel(base.r(), tint.r(), keep, amount),
        blend_channel(base.g(), tint.g(), keep, amount),
        blend_channel(base.b(), tint.b(), keep, amount),
    )
}

#[must_use]
pub fn alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[must_use]
pub fn composite_over(background: Color32, foreground: Color32) -> Color32 {
    if foreground.a() == 0 {
        return background;
    }
    if foreground.a() == u8::MAX {
        return foreground;
    }

    let [fg_r, fg_g, fg_b, _] = foreground.to_srgba_unmultiplied();
    let amount = f32::from(foreground.a()) / 255.0;
    let keep = 1.0 - amount;

    Color32::from_rgb(
        blend_channel(background.r(), fg_r, keep, amount),
        blend_channel(background.g(), fg_g, keep, amount),
        blend_channel(background.b(), fg_b, keep, amount),
    )
}

#[must_use]
pub fn panel_border(accent: Color32, focused: bool) -> Color32 {
    if focused {
        blend(BORDER_STRONG(), accent, 0.78)
    } else {
        alpha(blend(BORDER_SUBTLE(), accent, 0.32), 196)
    }
}

#[must_use]
pub fn terminal_color_to_egui(color: TerminalColor, colors: &Colors) -> Color32 {
    match color {
        TerminalColor::Named(name) => colors[name].map_or_else(|| named_color_to_egui(name), rgb_to_egui),
        TerminalColor::Spec(rgb) => rgb_to_egui(rgb),
        TerminalColor::Indexed(index) => palette_color(index, colors),
    }
}

#[must_use]
pub fn ensure_terminal_text_contrast(fg: Color32, bg: Color32) -> Color32 {
    const MIN_CONTRAST_RATIO: f32 = 3.6;

    if contrast_ratio(fg, bg) >= MIN_CONTRAST_RATIO {
        return fg;
    }

    let target = if relative_luminance(bg) < 0.25 {
        Color32::from_rgb(245, 247, 255)
    } else {
        Color32::from_rgb(24, 27, 39)
    };

    for amount in [0.18, 0.32, 0.46, 0.60, 0.74, 0.88, 1.0] {
        let adjusted = blend(fg, target, amount);
        if contrast_ratio(adjusted, bg) >= MIN_CONTRAST_RATIO {
            return adjusted;
        }
    }

    target
}

fn palette_for(theme: ResolvedTheme) -> &'static ThemePalette {
    match theme {
        ResolvedTheme::Dark => &DARK_THEME,
        ResolvedTheme::Light => &LIGHT_THEME,
    }
}

fn active_palette() -> &'static ThemePalette {
    palette_for(current_theme())
}

fn style_for(theme: ResolvedTheme) -> Style {
    let mut style = Style::default();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.window_margin = Margin::same(0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.visuals = visuals(theme);
    style
}

fn visuals(theme: ResolvedTheme) -> Visuals {
    let palette = palette_for(theme);
    let mut visuals = match theme {
        ResolvedTheme::Dark => Visuals::dark(),
        ResolvedTheme::Light => Visuals::light(),
    };

    let (window_shadow_alpha, popup_shadow_alpha) = match theme {
        ResolvedTheme::Dark => (128, 118),
        ResolvedTheme::Light => (36, 24),
    };

    visuals.window_corner_radius = CornerRadius::same(16);
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 2,
        color: Color32::from_black_alpha(window_shadow_alpha),
    };
    visuals.window_stroke = Stroke::new(1.0, palette.border_subtle);
    visuals.window_fill = palette.panel_bg;
    visuals.window_highlight_topmost = false;

    visuals.panel_fill = palette.toolbar_bg;
    visuals.faint_bg_color = palette.panel_bg_alt;
    visuals.extreme_bg_color = palette.bg;
    visuals.code_bg_color = palette.bg_elevated;
    visuals.override_text_color = Some(palette.fg);
    visuals.hyperlink_color = palette.accent;
    visuals.resize_corner_size = 15.0;

    visuals.widgets.noninteractive.bg_fill = palette.bg_elevated;
    visuals.widgets.noninteractive.weak_bg_fill = palette.panel_bg_alt;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.fg_dim);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(12);

    visuals.widgets.inactive.bg_fill = palette.panel_bg_alt;
    visuals.widgets.inactive.weak_bg_fill = palette.bg_elevated;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.fg_soft);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(12);

    visuals.widgets.hovered.bg_fill = blend(palette.panel_bg_alt, palette.accent, 0.16);
    visuals.widgets.hovered.weak_bg_fill = palette.bg_elevated;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.fg);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(12);

    visuals.widgets.active.bg_fill = blend(palette.panel_bg_alt, palette.accent, 0.22);
    visuals.widgets.active.weak_bg_fill = palette.bg_elevated;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.fg);
    visuals.widgets.active.corner_radius = CornerRadius::same(12);

    visuals.selection.bg_fill = alpha(palette.accent, 54);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(popup_shadow_alpha),
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
    let palette = active_palette();

    match color {
        NamedColor::Black => palette.terminal_palette[0],
        NamedColor::Red => palette.terminal_palette[1],
        NamedColor::Green => palette.terminal_palette[2],
        NamedColor::Yellow => palette.terminal_palette[3],
        NamedColor::Blue => palette.terminal_palette[4],
        NamedColor::Magenta => palette.terminal_palette[5],
        NamedColor::Cyan => palette.terminal_palette[6],
        NamedColor::White => palette.terminal_palette[7],
        NamedColor::BrightBlack => palette.terminal_palette[8],
        NamedColor::BrightRed => palette.terminal_palette[9],
        NamedColor::BrightGreen => palette.terminal_palette[10],
        NamedColor::BrightYellow => palette.terminal_palette[11],
        NamedColor::BrightBlue => palette.terminal_palette[12],
        NamedColor::BrightMagenta => palette.terminal_palette[13],
        NamedColor::BrightCyan => palette.terminal_palette[14],
        NamedColor::BrightWhite => palette.terminal_palette[15],
        NamedColor::Foreground | NamedColor::BrightForeground => palette.fg,
        NamedColor::Background => palette.panel_bg,
        NamedColor::Cursor => palette.cursor,
        NamedColor::DimForeground => alpha(palette.fg_soft, 196),
        NamedColor::DimBlack => alpha(palette.terminal_palette[0], 196),
        NamedColor::DimRed => alpha(palette.terminal_palette[1], 196),
        NamedColor::DimGreen => alpha(palette.terminal_palette[2], 196),
        NamedColor::DimYellow => alpha(palette.terminal_palette[3], 196),
        NamedColor::DimBlue => alpha(palette.terminal_palette[4], 196),
        NamedColor::DimMagenta => alpha(palette.terminal_palette[5], 196),
        NamedColor::DimCyan => alpha(palette.terminal_palette[6], 196),
        NamedColor::DimWhite => alpha(palette.terminal_palette[7], 196),
    }
}

fn palette_color(index: u8, colors: &Colors) -> Color32 {
    let palette = active_palette();
    let index = usize::from(index);
    if index < palette.terminal_palette.len() {
        colors[index].map_or(palette.terminal_palette[index], rgb_to_egui)
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

fn relative_luminance(color: Color32) -> f32 {
    fn linear(channel: u8) -> f32 {
        let srgb = f32::from(channel) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedTheme, alpha, bg_for, composite_over, contrast_ratio, ensure_terminal_text_contrast, resolve_theme,
    };
    use egui::{Color32, Theme};
    use horizon_core::AppearanceTheme;

    #[test]
    fn alpha_preserves_unmultiplied_color_channels() {
        assert_eq!(
            alpha(Color32::from_rgb(116, 162, 247), 40),
            Color32::from_rgba_unmultiplied(116, 162, 247, 40),
        );
    }

    #[test]
    fn light_and_dark_backgrounds_differ() {
        assert_ne!(bg_for(ResolvedTheme::Dark), bg_for(ResolvedTheme::Light));
    }

    #[test]
    fn auto_theme_prefers_system_theme() {
        assert_eq!(
            resolve_theme(AppearanceTheme::Auto, Some(Theme::Light)),
            ResolvedTheme::Light
        );
        assert_eq!(
            resolve_theme(AppearanceTheme::Auto, Some(Theme::Dark)),
            ResolvedTheme::Dark
        );
    }

    #[test]
    fn auto_theme_falls_back_to_dark_without_system_preference() {
        assert_eq!(resolve_theme(AppearanceTheme::Auto, None), ResolvedTheme::Dark);
    }

    #[test]
    fn ensure_terminal_text_contrast_lifts_low_contrast_pairs() {
        let bg = Color32::from_rgb(92, 95, 119);
        let fg = Color32::from_rgb(124, 127, 147);

        let adjusted = ensure_terminal_text_contrast(fg, bg);

        assert!(contrast_ratio(adjusted, bg) >= 3.6);
    }

    #[test]
    fn composite_over_flattens_translucent_text_against_background() {
        let bg = Color32::from_rgb(230, 234, 242);
        let fg = alpha(Color32::from_rgb(92, 95, 119), 196);

        assert_eq!(composite_over(bg, fg), Color32::from_rgb(124, 127, 147));
    }
}
