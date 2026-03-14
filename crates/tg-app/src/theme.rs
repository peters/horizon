use egui::{Color32, Rounding, Shadow, Stroke, Style, Vec2, Visuals, epaint};

// ── Base palette — very dark, matching reference terminal design ─────────────
pub const BG: Color32 = Color32::from_rgb(12, 12, 18);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(20, 20, 30);
pub const PANEL_BG: Color32 = Color32::from_rgb(16, 16, 24);
pub const FG: Color32 = Color32::from_rgb(190, 195, 210);
pub const FG_DIM: Color32 = Color32::from_rgb(90, 95, 110);
pub const CURSOR: Color32 = Color32::from_rgb(200, 200, 220);
pub const GRID_DOT: Color32 = Color32::from_rgb(30, 30, 42);
pub const ACCENT: Color32 = Color32::from_rgb(120, 160, 230);
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(36, 36, 52);
pub const TITLEBAR_BG: Color32 = Color32::from_rgb(10, 10, 16);
pub const TOOLBAR_BG: Color32 = Color32::from_rgb(14, 14, 22);
pub const STATUSBAR_BG: Color32 = Color32::from_rgb(10, 10, 16);

// Traffic-light buttons
pub const BTN_CLOSE: Color32 = Color32::from_rgb(237, 106, 94);
pub const BTN_MINIMIZE: Color32 = Color32::from_rgb(245, 191, 79);
pub const BTN_MAXIMIZE: Color32 = Color32::from_rgb(98, 197, 84);

/// Standard 16-color terminal palette — rich, vivid colors on dark background
const PALETTE: [Color32; 16] = [
    Color32::from_rgb(50, 52, 66),    // 0  black
    Color32::from_rgb(220, 90, 90),   // 1  red
    Color32::from_rgb(90, 200, 90),   // 2  green
    Color32::from_rgb(220, 190, 100), // 3  yellow
    Color32::from_rgb(90, 140, 220),  // 4  blue
    Color32::from_rgb(190, 130, 210), // 5  magenta
    Color32::from_rgb(90, 200, 190),  // 6  cyan
    Color32::from_rgb(180, 185, 200), // 7  white
    Color32::from_rgb(70, 72, 90),    // 8  bright black
    Color32::from_rgb(240, 110, 110), // 9  bright red
    Color32::from_rgb(110, 230, 110), // 10 bright green
    Color32::from_rgb(240, 210, 120), // 11 bright yellow
    Color32::from_rgb(110, 160, 240), // 12 bright blue
    Color32::from_rgb(210, 150, 230), // 13 bright magenta
    Color32::from_rgb(110, 220, 210), // 14 bright cyan
    Color32::from_rgb(210, 215, 230), // 15 bright white
];

/// Apply the full dark theme to egui.
pub fn apply(ctx: &egui::Context) {
    let mut style = Style::default();

    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.window_margin = epaint::Margin::same(0.0);
    style.spacing.button_padding = Vec2::new(10.0, 4.0);

    let mut v = Visuals::dark();

    v.window_rounding = Rounding::same(12.0);
    v.window_shadow = Shadow {
        offset: [0.0, 8.0].into(),
        blur: 28.0,
        spread: 2.0,
        color: Color32::from_black_alpha(140),
    };
    v.window_stroke = Stroke::new(0.5, BORDER_SUBTLE);
    v.window_fill = PANEL_BG;
    v.window_highlight_topmost = false;

    v.panel_fill = TOOLBAR_BG;

    v.widgets.noninteractive.bg_fill = BG_ELEVATED;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG_DIM);
    v.widgets.noninteractive.rounding = Rounding::same(6.0);

    v.widgets.inactive.bg_fill = Color32::from_rgb(28, 28, 42);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, FG_DIM);
    v.widgets.inactive.rounding = Rounding::same(6.0);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(24, 24, 38);

    v.widgets.hovered.bg_fill = Color32::from_rgb(38, 38, 56);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, FG);
    v.widgets.hovered.rounding = Rounding::same(6.0);

    v.widgets.active.bg_fill = Color32::from_rgb(48, 48, 70);
    v.widgets.active.fg_stroke = Stroke::new(1.0, FG);
    v.widgets.active.rounding = Rounding::same(6.0);

    v.selection.bg_fill = Color32::from_rgba_premultiplied(120, 160, 230, 50);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(18, 18, 28);
    v.popup_shadow = Shadow {
        offset: [0.0, 4.0].into(),
        blur: 16.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };

    v.override_text_color = Some(FG);

    style.visuals = v;
    ctx.set_style(style);
}

pub fn vt100_color_to_egui(color: vt100::Color, is_fg: bool) -> Color32 {
    match color {
        vt100::Color::Default => {
            if is_fg {
                FG
            } else {
                PANEL_BG
            }
        }
        vt100::Color::Idx(idx) => {
            if idx < 16 {
                PALETTE[idx as usize]
            } else if idx < 232 {
                let idx = idx - 16;
                let r = (idx / 36) * 51;
                let g = ((idx % 36) / 6) * 51;
                let b = (idx % 6) * 51;
                Color32::from_rgb(r, g, b)
            } else {
                let v = 8 + (idx - 232) * 10;
                Color32::from_rgb(v, v, v)
            }
        }
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}
