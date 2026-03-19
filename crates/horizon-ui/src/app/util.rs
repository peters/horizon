use egui::{Button, Context, Modifiers, Pos2, Rect, Stroke, Vec2};

use crate::theme;

/// Screen-space rectangles occupied by fixed overlay widgets (sidebar,
/// minimap, attention feed).  Canvas-space elements such as workspace
/// labels must not render inside these regions.
///
/// Register any new fixed widget here so canvas-space content avoids it.
pub(super) struct OverlayExclusion {
    zones: Vec<Rect>,
}

impl OverlayExclusion {
    pub(super) fn new(zones: Vec<Rect>) -> Self {
        Self { zones }
    }

    /// Returns `true` if `rect` overlaps any exclusion zone.
    pub(super) fn intersects(&self, rect: Rect) -> bool {
        self.zones.iter().any(|zone| zone.intersects(rect))
    }
}

pub(super) fn viewport_local_rect(ctx: &Context) -> Rect {
    ctx.input(|input| input.viewport().inner_rect.or(input.viewport().outer_rect))
        .map_or_else(
            || {
                let rect = ctx.content_rect();
                Rect::from_min_size(Pos2::ZERO, rect.size())
            },
            |rect| Rect::from_min_size(Pos2::ZERO, rect.size()),
        )
}

pub(super) fn empty_string_as_none(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

pub(super) fn primary_shortcut_modifier(modifiers: Modifiers) -> bool {
    modifiers.ctrl || modifiers.command
}

pub(super) fn primary_shortcut_label() -> &'static str {
    if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" }
}

pub(super) fn short_session_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

pub(super) fn editor_panel_size_for_file(path: &std::path::Path) -> [f32; 2] {
    const CHAR_W: f32 = 8.0;
    const LINE_H: f32 = 20.0;
    const PAD: f32 = 80.0;
    const MIN_W: f32 = 400.0;
    const MIN_H: f32 = 280.0;
    const MAX_W: f32 = 900.0;
    const MAX_H: f32 = 800.0;

    let Ok(content) = std::fs::read_to_string(path) else {
        return [520.0, 400.0];
    };

    let line_count = content.lines().count().max(1);
    let max_line_len = content.lines().map(str::len).max().unwrap_or(40);

    let w = (usize_to_f32(max_line_len) * CHAR_W + PAD).clamp(MIN_W, MAX_W);
    let h = (usize_to_f32(line_count) * LINE_H + PAD).clamp(MIN_H, MAX_H);

    [w, h]
}

pub(super) fn truncate_session_label(label: &str) -> String {
    const MAX_CHARS: usize = 40;
    if label.chars().count() <= MAX_CHARS {
        return label.to_string();
    }

    let mut truncated = label.chars().take(MAX_CHARS - 1).collect::<String>();
    truncated.push('…');
    truncated
}

pub(super) fn paint_empty_state(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let card_rect = Rect::from_center_size(rect.center(), Vec2::new(380.0, 120.0));
    let painter = ui.painter();
    let shortcut = primary_shortcut_label();

    painter.rect_filled(
        card_rect,
        egui::CornerRadius::same(20),
        theme::alpha(theme::PANEL_BG, 236),
    );
    painter.rect_stroke(
        card_rect,
        egui::CornerRadius::same(20),
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)),
        egui::StrokeKind::Outside,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 30.0),
        egui::Align2::CENTER_CENTER,
        "Spatial terminal observatory",
        egui::FontId::proportional(17.0),
        theme::FG,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 60.0),
        egui::Align2::CENTER_CENTER,
        format!("{shortcut}+double-click to create a workspace."),
        egui::FontId::proportional(11.5),
        theme::FG_SOFT,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 80.0),
        egui::Align2::CENTER_CENTER,
        format!("{shortcut}+double-click inside a workspace to add a terminal."),
        egui::FontId::proportional(11.5),
        theme::FG_SOFT,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 102.0),
        egui::Align2::CENTER_CENTER,
        "Scroll to pan vertically, Shift+scroll horizontally.",
        egui::FontId::proportional(10.5),
        theme::FG_DIM,
    );
}

pub(super) fn paint_canvas_glow(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();

    painter.circle_filled(
        Pos2::new(rect.max.x + 48.0, rect.center().y),
        rect.height() * 0.44,
        theme::CANVAS_WARM_GLOW,
    );
    painter.circle_filled(
        Pos2::new(rect.min.x - 72.0, rect.min.y + rect.height() * 0.16),
        rect.height() * 0.28,
        theme::CANVAS_COOL_GLOW,
    );
}

pub(super) fn clamp_panel_size(size: Vec2) -> Vec2 {
    Vec2::new(
        size.x.max(super::PANEL_MIN_SIZE[0]),
        size.y.max(super::PANEL_MIN_SIZE[1]),
    )
}

pub(super) fn workspace_label_width(name: &str) -> f32 {
    let estimated_text_width: f32 = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_uppercase() {
                8.6
            } else if ch.is_ascii_whitespace() {
                4.5
            } else {
                7.6
            }
        })
        .sum();

    (estimated_text_width + 60.0).clamp(super::WS_LABEL_MIN_WIDTH, super::WS_LABEL_MAX_WIDTH)
}

pub(super) fn format_compact_count(value: usize) -> String {
    if value >= 10_000 {
        return format!("{}k", value / 1_000);
    }

    if value >= 1_000 {
        let whole = value / 1_000;
        let tenth = (value % 1_000) / 100;
        if tenth == 0 {
            return format!("{whole}k");
        }
        return format!("{whole}.{tenth}k");
    }

    value.to_string()
}

pub(super) fn format_grid_position(position: Pos2) -> String {
    format!("{}, {}", rounded_i32(position.x), rounded_i32(position.y))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(super) fn rounded_i32(value: f32) -> i32 {
    let rounded = value.round();
    if rounded.is_nan() {
        0
    } else {
        rounded.clamp(i32::MIN as f32, i32::MAX as f32) as i32
    }
}

pub(super) fn primary_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.5).color(theme::FG))
        .fill(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28))
        .stroke(Stroke::new(
            1.0,
            theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.72),
        ))
        .corner_radius(10)
}

pub(super) fn chrome_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.0).color(theme::FG_SOFT))
        .fill(theme::PANEL_BG_ALT)
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
        .corner_radius(10)
}

pub(crate) fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

pub(super) fn atomic_write(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(content.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{clamp_panel_size, format_grid_position, primary_shortcut_label, primary_shortcut_modifier};
    use egui::{Modifiers, Pos2, Vec2};

    fn default_panel_canvas_pos(index: usize) -> Pos2 {
        const PANEL_COLUMN_SPACING: f32 = 540.0;
        const PANEL_ROW_SPACING: f32 = 360.0;

        let column = super::usize_to_f32(index % 3);
        let row = super::usize_to_f32(index / 3);
        Pos2::new(120.0 + column * PANEL_COLUMN_SPACING, 120.0 + row * PANEL_ROW_SPACING)
    }

    #[test]
    fn default_panel_positions_tile_in_rows() {
        assert_eq!(default_panel_canvas_pos(0), Pos2::new(120.0, 120.0));
        assert_eq!(default_panel_canvas_pos(1), Pos2::new(660.0, 120.0));
        assert_eq!(default_panel_canvas_pos(3), Pos2::new(120.0, 480.0));
    }

    #[test]
    fn panel_size_is_clamped_to_minimums() {
        let clamped = clamp_panel_size(Vec2::new(100.0, 120.0));

        assert!((clamped.x - super::super::PANEL_MIN_SIZE[0]).abs() <= f32::EPSILON);
        assert!(clamped.y >= super::super::PANEL_MIN_SIZE[1]);
    }

    #[test]
    fn grid_positions_are_rounded_for_display() {
        assert_eq!(format_grid_position(Pos2::new(12.4, -7.6)), "12, -8");
        assert_eq!(format_grid_position(Pos2::new(-3.5, 2.5)), "-4, 3");
    }

    #[test]
    fn primary_shortcut_modifier_accepts_ctrl() {
        let modifiers = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };

        assert!(primary_shortcut_modifier(modifiers));
    }

    #[test]
    fn primary_shortcut_modifier_accepts_command() {
        let modifiers = Modifiers {
            command: true,
            ..Modifiers::default()
        };

        assert!(primary_shortcut_modifier(modifiers));
    }

    #[test]
    fn primary_shortcut_label_matches_platform_convention() {
        let expected = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };

        assert_eq!(primary_shortcut_label(), expected);
    }
}
