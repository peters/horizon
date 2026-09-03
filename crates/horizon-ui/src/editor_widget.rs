use egui::containers::scroll_area::ScrollBarVisibility;
use egui::text::{CCursor, CCursorRange};
use egui::{Align, Color32, CornerRadius, FontId, Id, Layout, Pos2, Rect, RichText, ScrollArea, TextEdit, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use horizon_core::{MarkdownEditor, Panel, PanelId, PreviewMode, ShortcutBinding};

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
    pub fn show(&mut self, ui: &mut egui::Ui, is_active_panel: bool, save_shortcut: ShortcutBinding) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let mode_rect = {
            let Some(editor) = self.panel.content.editor_mut() else {
                return false;
            };
            render_mode_bar(ui, editor)
        };
        let preview_cache = self.preview_cache.take();

        let body_rect = Rect::from_min_max(Pos2::new(ui.cursor().min.x, mode_rect.max.y + 2.0), ui.max_rect().max);

        // Suppress the save chord while the speech hotkey binder is
        // capturing, so rebinding e.g. Ctrl+Shift+S does not also save.
        if !crate::app::shortcuts::hotkey_capture_active(ui.ctx())
            && ui.input(|input| shortcut_pressed(input, save_shortcut))
            && let Some(ed) = self.panel.content.editor_mut()
        {
            ed.save_if_dirty();
        }

        let mode = self
            .panel
            .content
            .editor()
            .map_or(PreviewMode::Edit, |editor| editor.preview_mode);

        render_body(ui, self.panel, body_rect, mode, preview_cache, is_active_panel);
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
                    .color(if is_active { theme::FG() } else { theme::FG_DIM() });
                let button = egui::Button::new(text)
                    .fill(if is_active {
                        theme::PANEL_BG_ALT()
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
                            .color(theme::FG_DIM()),
                    );
                } else if editor.dirty {
                    ui.label(RichText::new("* scratch").size(11.0).color(theme::FG_DIM()));
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
    claim_focus: bool,
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
                    render_edit_pane(ui, panel, claim_focus);
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

fn editor_text_id(panel_id: PanelId) -> Id {
    Id::new(("horizon_editor_text", panel_id.0))
}

fn sync_text_edit_caret(ctx: &egui::Context, id: Id, caret: usize) {
    let mut state = TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
    state.store(ctx, id);
}

fn render_edit_pane(ui: &mut egui::Ui, panel: &mut Panel, claim_focus: bool) -> Option<Rect> {
    let editor_id = editor_text_id(panel.id);
    let editor = panel.content.editor_mut()?;
    if let Some(caret) = editor.take_pending_caret() {
        sync_text_edit_caret(ui.ctx(), editor_id, caret);
        // Do not steal keyboard focus when the user has moved to another
        // panel or an overlay text field while transcription was running.
        if claim_focus && !crate::app::shortcuts::hotkey_capture_active(ui.ctx()) {
            ui.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    let output = ScrollArea::vertical()
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
        .id_salt(("editor_edit", panel.id.0))
        .show_viewport(ui, |ui, viewport| {
            // egui 0.36 sizes a TextEdit from its row count and ignores the
            // `min_size` height, so fill the visible viewport via rows to keep
            // the whole pane clickable.
            let row_height = ui.fonts_mut(|fonts| fonts.row_height(&FontId::monospace(FONT_SIZE)));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let desired_rows = (viewport.height() / row_height).ceil().max(1.0) as usize;
            let output = markdown_text_edit(&mut editor.text, viewport.size(), desired_rows, editor_id).show(ui);
            if output.response.changed() {
                editor.dirty = true;
            }
            if let Some(range) = output.cursor_range {
                editor.caret = range.primary.index.into();
            }
            output.response.rect
        });
    Some(output.inner)
}

fn markdown_text_edit(text: &mut String, min_size: Vec2, desired_rows: usize, id: Id) -> TextEdit<'_> {
    TextEdit::multiline(text)
        .id(id)
        .font(FontId::monospace(FONT_SIZE))
        .desired_width(f32::INFINITY)
        .desired_rows(desired_rows)
        .min_size(min_size)
        .frame(egui::Frame::NONE)
        .text_color(theme::FG())
        .lock_focus(true)
}

fn render_preview_pane(ui: &mut egui::Ui, panel: &mut Panel, preview_cache: Option<&mut MarkdownPreviewCache>) {
    let panel_id = panel.id.0;
    let Some(editor) = panel.content.editor_mut() else {
        return;
    };

    let mut fallback_cache = MarkdownPreviewCache::default();
    let cache = preview_cache.unwrap_or(&mut fallback_cache);

    ScrollArea::vertical()
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
}

#[cfg(test)]
mod tests {
    use crate::test_egui::DiscardTextures;
    use std::path::PathBuf;

    use egui::{Align, Context, Layout, Pos2, RawInput, Rect, UiBuilder, Vec2, ViewportId};
    use horizon_core::{AppearanceTheme, Panel, PanelId, PanelKind, PanelOptions, PreviewMode, WorkspaceId};

    use super::{editor_text_id, render_edit_pane};
    use crate::theme;

    fn test_editor_panel(text: &str) -> Panel {
        let scratch = tempfile::NamedTempFile::new().expect("temp markdown file");
        std::fs::write(scratch.path(), text).expect("seed markdown file");
        Panel::spawn(
            PanelId(7),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Editor,
                command: Some(scratch.path().display().to_string()),
                cwd: Some(
                    scratch
                        .path()
                        .parent()
                        .map_or_else(|| PathBuf::from("."), PathBuf::from),
                ),
                ..PanelOptions::default()
            },
        )
        .expect("spawn editor panel")
    }

    #[test]
    fn edit_text_hitbox_fills_visible_body() {
        let ctx = Context::default();
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
            ..egui::RawInput::default()
        };
        input.viewport_id = egui::ViewportId::ROOT;

        let body_rect = Rect::from_min_size(Pos2::new(20.0, 24.0), Vec2::new(320.0, 200.0));
        let mut panel = test_editor_panel("");

        theme::apply(&ctx, AppearanceTheme::Dark);
        let mut text_rect = None;
        let _ = ctx
            .run_ui(input, |ui| {
                text_rect = Some(
                    egui::CentralPanel::default()
                        .show(ui, |ui| {
                            ui.scope_builder(
                                UiBuilder::new()
                                    .max_rect(body_rect)
                                    .layout(Layout::top_down(Align::Min)),
                                |ui| render_edit_pane(ui, &mut panel, true).expect("editor response"),
                            )
                            .inner
                        })
                        .inner,
                );
            })
            .discard_textures();
        let text_rect = text_rect.expect("editor frame ran");

        let probe_points = [
            Pos2::new(body_rect.left() + 8.0, body_rect.top() + 8.0),
            body_rect.center(),
            Pos2::new(body_rect.left() + 8.0, body_rect.bottom() - 8.0),
            Pos2::new(body_rect.center().x, body_rect.bottom() - 8.0),
        ];

        for point in probe_points {
            assert!(
                text_rect.contains(point),
                "expected editor hitbox {text_rect:?} to contain {point:?}"
            );
        }
    }

    fn run_focused_edit_pane(ctx: &Context, panel: &mut Panel, body_rect: Rect) {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
            ..RawInput::default()
        };
        input.viewport_id = ViewportId::ROOT;
        let _ = ctx
            .run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(body_rect)
                            .layout(Layout::top_down(Align::Min)),
                        |ui| render_edit_pane(ui, panel, true),
                    );
                });
            })
            .discard_textures();
    }

    #[test]
    fn consecutive_dictation_keeps_order_in_a_focused_editor() {
        let ctx = Context::default();
        theme::apply(&ctx, AppearanceTheme::Dark);
        let body_rect = Rect::from_min_size(Pos2::new(20.0, 24.0), Vec2::new(320.0, 200.0));
        let mut panel = test_editor_panel("ab cd");
        {
            let editor = panel.content.editor_mut().expect("editor content");
            editor.preview_mode = PreviewMode::Edit;
            editor.set_caret(2);
        }

        run_focused_edit_pane(&ctx, &mut panel, body_rect);
        ctx.memory_mut(|memory| memory.request_focus(editor_text_id(panel.id)));
        run_focused_edit_pane(&ctx, &mut panel, body_rect);

        panel
            .content
            .editor_mut()
            .expect("editor content")
            .insert_dictation("XY ");
        run_focused_edit_pane(&ctx, &mut panel, body_rect);
        panel
            .content
            .editor_mut()
            .expect("editor content")
            .insert_dictation("ZW ");
        run_focused_edit_pane(&ctx, &mut panel, body_rect);

        let editor = panel.editor().expect("editor content");
        assert_eq!(editor.text, "abXY ZW  cd");
        assert_eq!(editor.caret, 8);
    }
}
