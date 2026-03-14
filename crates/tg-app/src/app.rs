use egui::{Color32, Pos2, Rounding, Shadow, Stroke, Vec2};
use tg_core::{Board, Config, PanelId, PanelOptions};

use crate::terminal_widget::TerminalView;
use crate::theme;

pub struct TermgaloreApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    new_workspace_name: String,
    theme_applied: bool,
    zoom: f32,
    pan_offset: Vec2,
    panel_positions: std::collections::HashMap<PanelId, Pos2>,
    config_path: Option<std::path::PathBuf>,
    show_config_editor: bool,
    config_text: String,
    config_last_modified: Option<std::time::SystemTime>,
}

impl TermgaloreApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, config: &Config, config_path: Option<std::path::PathBuf>) -> Self {
        let board = Board::from_config(config).unwrap_or_else(|e| {
            tracing::error!("failed to load config: {e}");
            Board::new()
        });

        // Load config text for the editor
        let config_text = config_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let config_last_modified = config_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        Self {
            board,
            panels_to_close: Vec::new(),
            new_workspace_name: String::new(),
            theme_applied: false,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            panel_positions: std::collections::HashMap::new(),
            config_path,
            show_config_editor: false,
            config_text,
            config_last_modified,
        }
    }
}

impl eframe::App for TermgaloreApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }

        // Ctrl+scroll zoom
        let zoom_delta = ctx.input(egui::InputState::zoom_delta);
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            self.zoom = (self.zoom * zoom_delta).clamp(0.4, 3.0);
            ctx.set_zoom_factor(self.zoom);
        }

        // Canvas panning: middle-mouse drag or scroll without Ctrl
        let pan_delta = ctx.input(|i| {
            if i.pointer.middle_down() {
                i.pointer.delta()
            } else if !i.modifiers.ctrl {
                i.smooth_scroll_delta + i.raw_scroll_delta
            } else {
                Vec2::ZERO
            }
        });
        if pan_delta != Vec2::ZERO {
            self.pan_offset += pan_delta;
            // Move all floating windows by the pan delta
            pan_all_windows(ctx, pan_delta, &self.board, &self.panel_positions);
        }

        handle_edge_resize(ctx);

        self.board.process_output();

        for id in self.panels_to_close.drain(..) {
            self.board.close_panel(id);
        }

        // Shortcuts
        let create_new = ctx.input(|i| i.key_pressed(egui::Key::N) && i.modifiers.ctrl && i.modifiers.shift);
        if create_new {
            let ws = self.board.workspaces.first().map(|w| w.id);
            let _ = self.board.create_panel(PanelOptions::default(), ws);
        }
        // Ctrl+, to toggle config editor
        if ctx.input(|i| i.key_pressed(egui::Key::Comma) && i.modifiers.ctrl) {
            self.show_config_editor = !self.show_config_editor;
        }

        // Auto-reload config from disk
        self.check_config_reload();

        render_custom_titlebar(ctx);
        self.render_toolbar(ctx);
        self.render_statusbar(ctx);

        // Canvas with dot grid
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                draw_dot_grid(ui, self.pan_offset);
                self.draw_connectors(ui);
            });

        self.render_workspace_badges(ctx);
        self.render_panels(ctx);

        // Config editor overlay
        if self.show_config_editor {
            self.render_config_editor(ctx);
        }

        ctx.request_repaint();
    }
}

impl TermgaloreApp {
    fn render_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(36.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::TOOLBAR_BG)
                    .inner_margin(egui::Margin::symmetric(12.0, 4.0))
                    .stroke(Stroke::new(0.5, theme::BORDER_SUBTLE)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new("workspace").color(theme::FG_DIM).size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_workspace_name)
                            .desired_width(120.0)
                            .hint_text("name…"),
                    );
                    if ui.add(styled_button("+ Workspace")).clicked() && !self.new_workspace_name.is_empty() {
                        let name = self.new_workspace_name.clone();
                        self.board.create_workspace(&name);
                        self.new_workspace_name.clear();
                    }

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    let ws_info: Vec<_> = self
                        .board
                        .workspaces
                        .iter()
                        .map(|w| (w.id, w.name.clone(), w.accent(), w.panels.len()))
                        .collect();

                    for (ws_id, name, accent, count) in ws_info {
                        let color = Color32::from_rgb(accent.0, accent.1, accent.2);
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                                ui.painter().circle_filled(dot_rect.center(), 4.0, color);
                                ui.label(
                                    egui::RichText::new(format!(" {name} ({count}) "))
                                        .color(color)
                                        .size(11.5),
                                );
                                if ui.small_button("+").on_hover_text("Add terminal").clicked() {
                                    let _ = self.board.create_panel(PanelOptions::default(), Some(ws_id));
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} panels", self.board.panels.len()))
                                .color(theme::FG_DIM)
                                .size(11.0),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Ctrl+Shift+N: new terminal")
                                .color(theme::FG_DIM)
                                .size(10.0),
                        );
                    });
                });
            });
    }

    fn render_statusbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(24.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::STATUSBAR_BG)
                    .inner_margin(egui::Margin::symmetric(12.0, 2.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new("termgalore").color(theme::FG_DIM).size(10.0));
                    ui.separator();
                    for ws in &self.board.workspaces {
                        let (r, g, b) = ws.accent();
                        let color = Color32::from_rgb(r, g, b);
                        let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(6.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 3.0, color);
                        ui.label(
                            egui::RichText::new(format!("{}: {}", ws.name, ws.panels.len()))
                                .color(theme::FG_DIM)
                                .size(10.0),
                        );
                        ui.add_space(8.0);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(egui::RichText::new("+").size(12.0)).frame(false))
                            .on_hover_text("Zoom in (Ctrl+Scroll)")
                            .clicked()
                        {
                            self.zoom = (self.zoom + 0.1).min(3.0);
                            ctx.set_zoom_factor(self.zoom);
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                                .color(theme::FG_DIM)
                                .size(10.0),
                        );
                        if ui
                            .add(egui::Button::new(egui::RichText::new("−").size(12.0)).frame(false))
                            .on_hover_text("Zoom out (Ctrl+Scroll)")
                            .clicked()
                        {
                            self.zoom = (self.zoom - 0.1).max(0.4);
                            ctx.set_zoom_factor(self.zoom);
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("wgpu").color(theme::FG_DIM).size(10.0));
                    });
                });
            });
    }

    /// Render small draggable workspace badges on the canvas.
    fn check_config_reload(&mut self) {
        let Some(path) = &self.config_path else { return };
        let Ok(meta) = std::fs::metadata(path) else { return };
        let Ok(modified) = meta.modified() else { return };

        if self.config_last_modified.is_some_and(|last| modified > last) {
            tracing::info!("config changed on disk, reloading");
            self.config_last_modified = Some(modified);
            if let Ok(text) = std::fs::read_to_string(path) {
                self.config_text.clone_from(&text);
                if let Ok(config) = serde_yaml::from_str::<tg_core::Config>(&text)
                    && let Ok(new_board) = Board::from_config(&config)
                {
                    self.board = new_board;
                    self.panel_positions.clear();
                    tracing::info!("config reloaded successfully");
                }
            }
        }
    }

    fn render_config_editor(&mut self, ctx: &egui::Context) {
        egui::Window::new("Config Editor")
            .id(egui::Id::new("config_editor"))
            .default_size([500.0, 400.0])
            .resizable(true)
            .collapsible(true)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED)
                    .rounding(Rounding::same(10.0))
                    .inner_margin(8.0)
                    .stroke(Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.4)))
                    .shadow(Shadow {
                        offset: [0.0, 6.0].into(),
                        blur: 20.0,
                        spread: 2.0,
                        color: Color32::from_black_alpha(100),
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Ctrl+, to toggle").color(theme::FG_DIM).size(10.0));
                    if let Some(path) = &self.config_path {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .color(theme::FG_DIM)
                                .size(10.0),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(styled_button("Save & Apply")).clicked() {
                            self.save_and_apply_config();
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.config_text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(20),
                    );
                });
            });
    }

    fn save_and_apply_config(&mut self) {
        // Try to parse
        match serde_yaml::from_str::<tg_core::Config>(&self.config_text) {
            Ok(config) => {
                // Save to disk
                if let Some(path) = &self.config_path {
                    if let Err(e) = std::fs::write(path, &self.config_text) {
                        tracing::error!("failed to write config: {e}");
                        return;
                    }
                    self.config_last_modified = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
                }
                // Apply
                match Board::from_config(&config) {
                    Ok(new_board) => {
                        self.board = new_board;
                        self.panel_positions.clear();
                        tracing::info!("config applied");
                    }
                    Err(e) => tracing::error!("failed to apply config: {e}"),
                }
            }
            Err(e) => tracing::error!("invalid YAML: {e}"),
        }
    }

    fn render_workspace_badges(&mut self, ctx: &egui::Context) {
        let ws_data: Vec<_> = self
            .board
            .workspaces
            .iter()
            .map(|ws| (ws.id, ws.name.clone(), ws.accent(), ws.panels.len(), ws.position))
            .collect();

        for (ws_id, name, accent, count, position) in ws_data {
            let color = Color32::from_rgb(accent.0, accent.1, accent.2);

            let response = egui::Area::new(egui::Id::new(("ws_badge", ws_id.0)))
                .default_pos(Pos2::new(position[0], position[1]))
                .movable(true)
                .show(ctx, |ui| {
                    egui::Frame::default()
                        .fill(Color32::from_rgba_premultiplied(
                            accent.0 / 4,
                            accent.1 / 4,
                            accent.2 / 4,
                            240,
                        ))
                        .rounding(Rounding::same(20.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                        .stroke(Stroke::new(1.5, color.gamma_multiply(0.6)))
                        .shadow(Shadow {
                            offset: [0.0, 2.0].into(),
                            blur: 10.0,
                            spread: 1.0,
                            color: Color32::from_rgba_premultiplied(accent.0, accent.1, accent.2, 25),
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Small colored circle
                                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                                ui.painter().circle_filled(dot_rect.center(), 5.0, color);
                                ui.label(egui::RichText::new(&name).color(color).size(12.0).strong());
                                ui.label(
                                    egui::RichText::new(format!("{count}"))
                                        .color(color.gamma_multiply(0.5))
                                        .size(10.0),
                                );
                            });
                        });
                });

            // Track badge position
            let badge_pos = response.response.rect.center();
            if let Some(ws) = self.board.workspaces.iter_mut().find(|w| w.id == ws_id) {
                ws.position = [response.response.rect.min.x, response.response.rect.min.y];
            }

            // Store badge center for connector drawing
            // We'll use a negative panel ID convention for badges — store in a temp map
            // Actually, store in workspace position which we already have
            let _ = badge_pos; // used in draw_connectors via ws.position
        }
    }

    /// Draw thin connector lines from workspace badges to their panel windows.
    fn draw_connectors(&self, ui: &mut egui::Ui) {
        let painter = ui.painter();

        for ws in &self.board.workspaces {
            if ws.panels.is_empty() {
                continue;
            }

            let (r, g, b) = ws.accent();
            let color = Color32::from_rgba_premultiplied(r, g, b, 50);
            let dot_color = Color32::from_rgba_premultiplied(r, g, b, 80);

            // Badge center (approximate from position + typical badge size)
            let badge_center = Pos2::new(ws.position[0] + 50.0, ws.position[1] + 14.0);

            for panel_id in &ws.panels {
                if let Some(&panel_pos) = self.panel_positions.get(panel_id) {
                    // Draw a thin line from badge to panel
                    painter.line_segment([badge_center, panel_pos], Stroke::new(1.0, color));
                    // Small dot at the panel end
                    painter.circle_filled(panel_pos, 3.0, dot_color);
                }
            }
        }
    }

    /// Render all panels as individual floating terminal windows.
    fn render_panels(&mut self, ctx: &egui::Context) {
        let panel_info: Vec<_> = self
            .board
            .panels
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ws = self.board.workspaces.iter().find(|w| w.panels.contains(&p.id));
                // Auto-stack: compute default position in a grid
                let col = i % 3;
                let row = i / 3;
                let default_x = 120.0 + col as f32 * 640.0;
                let default_y = 120.0 + row as f32 * 440.0;
                (
                    p.id,
                    p.title.clone(),
                    ws.map(tg_core::Workspace::accent),
                    Pos2::new(default_x, default_y),
                )
            })
            .collect();

        let mut close_ids = Vec::new();

        for (id, title, accent, default_pos) in &panel_info {
            let mut open = true;
            let is_focused = self.board.focused == Some(*id);

            let accent_color = accent.map_or(theme::BORDER_SUBTLE, |a| Color32::from_rgb(a.0, a.1, a.2));

            let border_color = if is_focused {
                accent_color
            } else {
                Color32::from_rgba_premultiplied(accent_color.r(), accent_color.g(), accent_color.b(), 50)
            };

            let shadow = if is_focused {
                Shadow {
                    offset: [0.0, 6.0].into(),
                    blur: 20.0,
                    spread: 2.0,
                    color: Color32::from_rgba_premultiplied(accent_color.r(), accent_color.g(), accent_color.b(), 30),
                }
            } else {
                Shadow {
                    offset: [0.0, 4.0].into(),
                    blur: 16.0,
                    spread: 0.0,
                    color: Color32::from_black_alpha(80),
                }
            };

            let stroke_width = if is_focused { 1.5 } else { 0.5 };

            let response = egui::Window::new(title)
                .id(egui::Id::new(("panel", id.0)))
                .open(&mut open)
                .resizable(true)
                .collapsible(true)
                .title_bar(true)
                .default_pos(*default_pos)
                .default_size([600.0, 380.0])
                .frame(
                    egui::Frame::default()
                        .fill(theme::PANEL_BG)
                        .rounding(Rounding::same(10.0))
                        .inner_margin(4.0)
                        .stroke(Stroke::new(stroke_width, border_color))
                        .shadow(shadow),
                )
                .show(ctx, |ui| {
                    if let Some(panel) = self.board.panels.iter_mut().find(|p| p.id == *id) {
                        let clicked = TerminalView::new(panel).show(ui);
                        if clicked {
                            self.board.focused = Some(*id);
                        }
                    }
                });

            // Track panel center for connector lines
            if let Some(inner) = &response {
                let center_top = Pos2::new(inner.response.rect.center().x, inner.response.rect.min.y);
                self.panel_positions.insert(*id, center_top);
            }

            if !open {
                close_ids.push(*id);
            }
        }

        self.panels_to_close = close_ids;
    }
}

/// Move all floating egui windows (panels + badges) by a delta for canvas panning.
fn pan_all_windows(
    ctx: &egui::Context,
    delta: Vec2,
    board: &Board,
    panel_positions: &std::collections::HashMap<PanelId, Pos2>,
) {
    let mut ids: Vec<egui::Id> = Vec::new();

    // Panel window IDs
    for p in &board.panels {
        ids.push(egui::Id::new(("panel", p.id.0)));
    }
    // Badge area IDs
    for ws in &board.workspaces {
        ids.push(egui::Id::new(("ws_badge", ws.id.0)));
    }

    // Also move any panels tracked in panel_positions
    let _ = panel_positions;

    for id in ids {
        let layer_id = egui::LayerId::new(egui::Order::Middle, id);
        ctx.transform_layer_shapes(layer_id, egui::emath::TSTransform::from_translation(delta));
    }
}

fn handle_edge_resize(ctx: &egui::Context) {
    let Some(rect) = ctx.input(|i| i.viewport().inner_rect) else {
        return;
    };
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };

    let margin = 5.0;
    let titlebar_height = 36.0;

    let left = pos.x - rect.min.x < margin;
    let right = rect.max.x - pos.x < margin;
    let bottom = rect.max.y - pos.y < margin;
    let below_titlebar = pos.y - rect.min.y > titlebar_height;

    let direction = match (left, right, bottom, below_titlebar) {
        (true, _, true, _) => Some(egui::ResizeDirection::SouthWest),
        (_, true, true, _) => Some(egui::ResizeDirection::SouthEast),
        (_, _, true, _) => Some(egui::ResizeDirection::South),
        (true, _, _, true) => Some(egui::ResizeDirection::West),
        (_, true, _, true) => Some(egui::ResizeDirection::East),
        _ => None,
    };

    // Set resize cursor
    if let Some(dir) = direction {
        let cursor = match dir {
            egui::ResizeDirection::East | egui::ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
            egui::ResizeDirection::South => egui::CursorIcon::ResizeVertical,
            egui::ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
            egui::ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
            _ => egui::CursorIcon::Default,
        };
        ctx.set_cursor_icon(cursor);
    }

    if let Some(dir) = direction
        && ctx.input(|i| i.pointer.any_pressed())
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

fn render_custom_titlebar(ctx: &egui::Context) {
    egui::TopBottomPanel::top("titlebar")
        .exact_height(32.0)
        .frame(
            egui::Frame::default()
                .fill(theme::TITLEBAR_BG)
                .inner_margin(egui::Margin::symmetric(12.0, 0.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                let btn_size = 12.0;
                let btn_spacing = 8.0;
                let buttons = [
                    (theme::BTN_CLOSE, "Close"),
                    (theme::BTN_MINIMIZE, "Minimize"),
                    (theme::BTN_MAXIMIZE, "Maximize"),
                ];
                for (color, tooltip) in buttons {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(btn_size), egui::Sense::click());
                    let fill = if response.hovered() {
                        color
                    } else {
                        color.gamma_multiply(0.7)
                    };
                    ui.painter().circle_filled(rect.center(), btn_size / 2.0, fill);
                    let response = response.on_hover_text(tooltip);
                    if response.clicked() {
                        match tooltip {
                            "Close" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            "Minimize" => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                            "Maximize" => {
                                let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                            }
                            _ => {}
                        }
                    }
                    ui.add_space(btn_spacing);
                }

                ui.add_space(16.0);
                ui.label(egui::RichText::new("termgalore").color(theme::FG_DIM).size(13.0));

                let remaining = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(remaining, egui::Sense::click_and_drag());
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if response.double_clicked() {
                    let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                }
            });
        });
}

fn styled_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(11.5))
        .rounding(Rounding::same(6.0))
        .stroke(Stroke::new(0.5, theme::BORDER_SUBTLE))
}

fn draw_dot_grid(ui: &mut egui::Ui, pan_offset: Vec2) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let spacing = 24.0;
    let dot_radius = 1.0;

    // Offset the grid so it scrolls with panning
    let offset_x = pan_offset.x % spacing;
    let offset_y = pan_offset.y % spacing;

    let mut x = rect.min.x + offset_x;
    while x < rect.max.x {
        let mut y = rect.min.y + offset_y;
        while y < rect.max.y {
            painter.circle_filled(Pos2::new(x, y), dot_radius, theme::GRID_DOT);
            y += spacing;
        }
        x += spacing;
    }
}
