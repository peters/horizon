use egui::containers::scroll_area::{ScrollBarVisibility, State as ScrollAreaState};
use egui::{Align, Color32, CornerRadius, FontId, Id, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use horizon_core::{MarkdownEditor, Panel, PreviewMode, ShortcutBinding};

use crate::app::shortcuts::shortcut_pressed;
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
    pub fn show(&mut self, ui: &mut egui::Ui, _is_active_panel: bool, save_shortcut: ShortcutBinding) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let mode_rect = {
            let Some(editor) = self.panel.content.editor_mut() else {
                return false;
            };
            render_mode_bar(ui, editor)
        };
        let preview_cache = self.preview_cache.take();

        let body_rect = Rect::from_min_max(Pos2::new(ui.cursor().min.x, mode_rect.max.y + 2.0), ui.max_rect().max);

        if ui.input(|input| shortcut_pressed(input, save_shortcut))
            && let Some(ed) = self.panel.content.editor_mut()
        {
            ed.save_if_dirty();
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
                |ui| {
                    // Tighten the clip rect to the body bounds. The parent
                    // canvas layer sets a very wide clip rect; without this
                    // the vertical-only ScrollArea inherits that width and
                    // lets content overflow the panel horizontally.
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    render_edit_pane(ui, panel);
                },
            );
        }
        PreviewMode::Preview | PreviewMode::Split => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    render_preview_pane(ui, panel, preview_cache);
                },
            );
        }
    }
}

fn render_edit_pane(ui: &mut egui::Ui, panel: &mut Panel) {
    let Some(editor) = panel.content.editor_mut() else {
        return;
    };

    let output = ScrollArea::vertical()
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
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
    forward_scroll_to_scroll_area(ui, output.id, output.inner_rect, output.content_size.y);
}

fn render_preview_pane(ui: &mut egui::Ui, panel: &mut Panel, preview_cache: Option<&mut MarkdownPreviewCache>) {
    let panel_id = panel.id.0;
    let Some(editor) = panel.content.editor_mut() else {
        return;
    };

    let mut fallback_cache = MarkdownPreviewCache::default();
    let cache = preview_cache.unwrap_or(&mut fallback_cache);

    let output = ScrollArea::vertical()
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
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
    forward_scroll_to_scroll_area(ui, output.id, output.inner_rect, output.content_size.y);
}

/// The panel `Area` uses `interactable(false)`, which makes egui's
/// `layer_id_at` skip the layer during hover detection. `ScrollArea`
/// therefore never sees the pointer as hovering and silently ignores
/// mouse-wheel events. We detect hover ourselves and apply the delta
/// to the stored scroll state.
fn forward_scroll_to_scroll_area(ui: &egui::Ui, scroll_id: Id, inner_rect: Rect, content_height: f32) {
    let from_global = ui.ctx().layer_transform_from_global(ui.layer_id());
    let pointer_in_area = ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| {
        let local = from_global.map_or(pos, |t| t * pos);
        inner_rect.contains(local)
    });
    if !pointer_in_area {
        return;
    }

    let scroll_delta = ui.ctx().input(|i| i.smooth_scroll_delta.y);
    if scroll_delta == 0.0 {
        return;
    }

    let max_offset = (content_height - inner_rect.height()).max(0.0);
    if let Some(mut state) = ScrollAreaState::load(ui.ctx(), scroll_id) {
        let new_offset = (state.offset.y - scroll_delta).clamp(0.0, max_offset);
        if (new_offset - state.offset.y).abs() > f32::EPSILON {
            state.offset.y = new_offset;
            state.store(ui.ctx(), scroll_id);
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            ui.ctx().request_repaint();
        }
    }
}
