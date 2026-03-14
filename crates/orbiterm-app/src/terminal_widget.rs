use egui::{FontId, Pos2, Rect, Rounding, Vec2};
use orbiterm_core::Panel;

use crate::input;
use crate::theme;

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;

pub struct TerminalView<'a> {
    panel: &'a mut Panel,
}

impl<'a> TerminalView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Renders the terminal panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, is_active_panel: bool) -> bool {
        let font_id = FontId::monospace(FONT_SIZE);
        let char_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let line_height = FONT_SIZE * LINE_HEIGHT_FACTOR;

        let available = ui.available_size();
        let terminal_rect_size = Vec2::new(available.x.max(char_width), available.y.max(line_height));
        let new_cols = quantize_dimension(available.x / char_width);
        let new_rows = quantize_dimension(available.y / line_height);

        if new_cols != self.panel.terminal.cols() || new_rows != self.panel.terminal.rows() {
            self.panel.resize(new_rows, new_cols);
        }

        let screen = self.panel.terminal.screen();

        let (rect, response) = ui.allocate_exact_size(terminal_rect_size, egui::Sense::click());

        if response.clicked() {
            response.request_focus();
        }

        let metrics = GridMetrics {
            char_width,
            line_height,
            font_id,
        };
        let has_terminal_focus = response.has_focus() || (is_active_panel && !ui.ctx().wants_keyboard_input());

        if ui.is_rect_visible(rect) {
            render_grid(ui, rect, screen, &metrics);
            render_cursor(ui, rect, screen, &metrics, has_terminal_focus);
        }

        if has_terminal_focus {
            let events: Vec<egui::Event> = ui.input(|i| i.events.clone());
            for event in &events {
                match event {
                    egui::Event::Text(text) => {
                        self.panel.write_input(text.as_bytes());
                    }
                    egui::Event::Copy => self.panel.write_input(&[3]),
                    egui::Event::Cut => self.panel.write_input(&[24]),
                    egui::Event::Paste(text) => self.panel.write_input(text.as_bytes()),
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if modifiers.ctrl && modifiers.shift {
                            continue;
                        }
                        if let Some(bytes) = input::key_to_bytes(*key, *modifiers) {
                            self.panel.write_input(&bytes);
                        }
                    }
                    _ => {}
                }
            }
        }

        response.clicked()
    }
}

struct GridMetrics {
    char_width: f32,
    line_height: f32,
    font_id: FontId,
}

fn render_grid(ui: &egui::Ui, rect: Rect, screen: &vt100::Screen, metrics: &GridMetrics) {
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, Rounding::same(6.0), theme::PANEL_BG);

    let (rows, cols) = screen.size();

    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                let x = rect.min.x + f32::from(col) * metrics.char_width;
                let y = rect.min.y + f32::from(row) * metrics.line_height;

                let bg = theme::vt100_color_to_egui(cell.bgcolor(), false);
                if bg != theme::PANEL_BG {
                    let cell_rect =
                        Rect::from_min_size(Pos2::new(x, y), Vec2::new(metrics.char_width, metrics.line_height));
                    painter.rect_filled(cell_rect, 0.0, bg);
                }

                let contents = cell.contents();
                if !contents.is_empty() && contents != " " {
                    let fg = theme::vt100_color_to_egui(cell.fgcolor(), true);
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        &contents,
                        metrics.font_id.clone(),
                        fg,
                    );
                }
            }
        }
    }
}

fn render_cursor(ui: &egui::Ui, rect: Rect, screen: &vt100::Screen, metrics: &GridMetrics, has_focus: bool) {
    let (cursor_row, cursor_col) = screen.cursor_position();
    let cx = rect.min.x + f32::from(cursor_col) * metrics.char_width;
    let cy = rect.min.y + f32::from(cursor_row) * metrics.line_height;
    let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(metrics.char_width, metrics.line_height));

    let painter = ui.painter_at(rect);

    if has_focus {
        // Solid block cursor when focused
        painter.rect_filled(cursor_rect, Rounding::same(1.0), theme::CURSOR.gamma_multiply(0.8));
    } else {
        // Hollow rectangle when not focused
        painter.rect_stroke(
            cursor_rect,
            Rounding::same(1.0),
            egui::Stroke::new(1.0, theme::CURSOR.gamma_multiply(0.4)),
        );
    }
}

fn quantize_dimension(value: f32) -> u16 {
    let clamped = if value.is_finite() {
        value.floor().clamp(1.0, f32::from(u16::MAX))
    } else {
        1.0
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        clamped as u16
    }
}
