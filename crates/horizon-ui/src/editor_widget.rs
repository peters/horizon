use egui::{Align, Color32, CornerRadius, FontId, Key, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use horizon_core::{MarkdownEditor, Panel, PreviewMode};

use crate::theme;

const FONT_SIZE: f32 = 14.0;
const MODE_BAR_HEIGHT: f32 = 28.0;

pub struct MarkdownEditorView<'a> {
    panel: &'a mut Panel,
    preview_cache: Option<&'a mut MarkdownPreviewCache>,
}

pub(crate) type MarkdownPreviewCache = CommonMarkCache;

impl<'a> MarkdownEditorView<'a> {
    pub fn new(panel: &'a mut Panel, preview_cache: Option<&'a mut MarkdownPreviewCache>) -> Self {
        Self { panel, preview_cache }
    }

    /// Renders the editor panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, _is_active_panel: bool) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let mode_rect = {
            let Some(editor) = self.panel.content.editor_mut() else {
                return false;
            };
            render_mode_bar(ui, editor)
        };
        let preview_cache = self.preview_cache.take();

        let body_rect = Rect::from_min_max(Pos2::new(ui.cursor().min.x, mode_rect.max.y + 2.0), ui.max_rect().max);

        if ui.input(|input| input.modifiers.command && input.key_pressed(Key::S))
            && let Some(editor) = self.panel.content.editor_mut()
        {
            editor.save_if_dirty();
        }

        let mode = self
            .panel
            .content
            .editor()
            .map_or(PreviewMode::Edit, |editor| editor.preview_mode);

        render_body(ui, self.panel, body_rect, mode, preview_cache);
        clicked
    }
}

fn render_mode_bar(ui: &mut egui::Ui, editor: &mut MarkdownEditor) -> Rect {
    let mode_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), MODE_BAR_HEIGHT));
    ui.allocate_rect(mode_rect, egui::Sense::hover());

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(mode_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add_space(4.0);

            let preview_active = matches!(editor.preview_mode, PreviewMode::Preview | PreviewMode::Split);
            for (is_active, label, mode) in [
                (editor.preview_mode == PreviewMode::Edit, "Edit", PreviewMode::Edit),
                (preview_active, "Preview", PreviewMode::Preview),
            ] {
                let text = RichText::new(label)
                    .size(11.0)
                    .color(if is_active { theme::FG } else { theme::FG_DIM });
                let button = egui::Button::new(text)
                    .fill(if is_active {
                        theme::PANEL_BG_ALT
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(CornerRadius::same(4));
                if ui.add(button).clicked() {
                    editor.preview_mode = mode;
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if let Some(path) = &editor.file_path {
                    let label = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().to_string());
                    let prefix = if editor.dirty { "* " } else { "" };
                    ui.label(
                        RichText::new(format!("{prefix}{label}"))
                            .size(11.0)
                            .color(theme::FG_DIM),
                    );
                } else if editor.dirty {
                    ui.label(RichText::new("* scratch").size(11.0).color(theme::FG_DIM));
                }
            });
        },
    );

    mode_rect
}

fn render_body(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    body_rect: Rect,
    mode: PreviewMode,
    preview_cache: Option<&mut MarkdownPreviewCache>,
) {
    match mode {
        PreviewMode::Edit => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_edit_pane(ui, panel),
            );
        }
        PreviewMode::Preview | PreviewMode::Split => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_preview_pane(ui, panel, preview_cache),
            );
        }
    }
}

fn render_edit_pane(ui: &mut egui::Ui, panel: &mut Panel) {
    let Some(editor) = panel.content.editor_mut() else {
        return;
    };

    ScrollArea::vertical()
        .id_salt(("editor_edit", panel.id.0))
        .show(ui, |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut editor.text)
                    .font(FontId::monospace(FONT_SIZE))
                    .desired_width(f32::INFINITY)
                    .desired_rows(1)
                    .frame(false)
                    .text_color(theme::FG)
                    .lock_focus(true),
            );
            if response.changed() {
                editor.dirty = true;
            }
        });
}

fn render_preview_pane(ui: &mut egui::Ui, panel: &mut Panel, preview_cache: Option<&mut MarkdownPreviewCache>) {
    let panel_id = panel.id.0;
    let Some(editor) = panel.content.editor_mut() else {
        return;
    };

    let mut fallback_cache = MarkdownPreviewCache::default();
    let cache = preview_cache.unwrap_or(&mut fallback_cache);

    ScrollArea::vertical()
        .id_salt(("editor_preview", panel_id))
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.style_mut().url_in_tooltip = true;
            let response = CommonMarkViewer::new().show_mut(ui, cache, &mut editor.text);
            if response.response.changed() {
                editor.dirty = true;
                editor.save_if_dirty();
                ui.ctx().request_repaint();
            }
            ui.add_space(8.0);
        });
}
