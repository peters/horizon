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
pub const CANVAS_COOL_GLOW: Color32 = Color32::from_rgba_premultiplied(77, 112, 220, 20);
pub const CANVAS_WARM_GLOW: Color32 = Color32::from_rgba_premultiplied(255, 146, 80, 28);

pub const BTN_CLOSE: Color32 = Color32::from_rgb(235, 96, 88);

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
    Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn panel_border(accent: Color32, focused: bool) -> Color32 {
    if focused {
        blend(BORDER_STRONG, accent, 0.78)
    } else {
        alpha(blend(BORDER_SUBTLE, accent, 0.32), 196)
    }
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

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();

    visuals.window_corner_radius = CornerRadius::same(14);
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
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    visuals.widgets.inactive.bg_fill = PANEL_BG_ALT;
    visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, FG_SOFT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(10);

    visuals.widgets.hovered.bg_fill = blend(PANEL_BG_ALT, ACCENT, 0.16);
    visuals.widgets.hovered.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, FG);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(10);

    visuals.widgets.active.bg_fill = blend(PANEL_BG_ALT, ACCENT, 0.22);
    visuals.widgets.active.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, FG);
    visuals.widgets.active.corner_radius = CornerRadius::same(10);

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
