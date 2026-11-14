use std::sync::Arc;

use egui::containers::scroll_area::ScrollBarVisibility;
use egui::{Align, Color32, CornerRadius, FontId, Id, Layout, Pos2, Rect, ScrollArea, TextEdit, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use horizon_core::{MarkdownEditor, Panel, PanelId, PreviewMode, ShortcutBinding};

use crate::app::shortcuts::shortcut_pressed;
use crate::theme;
use crate::zoom_text;

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
    pub fn show(&mut self, ui: &mut egui::Ui, save_shortcut: ShortcutBinding) -> bool {
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
            // The frame of the app writes the file on the coordination worker.
            ed.request_save();
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

/// The text of the mode bar at the size it has on the screen of the canvas zoom.
///
/// The bar is a panel of the canvas, so a stock label would be a picture of zoom 1
/// that the layer transform scales: `Preview` and the file name blur at any zoom
/// but 100 percent. The galley keeps the size and the line of zoom 1.
fn mode_bar_text(ui: &egui::Ui, text: &str, color: Color32) -> egui::WidgetText {
    zoom_text::widget_text(ui.painter(), text, FontId::proportional(11.0), color, None)
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
                let text = mode_bar_text(ui, label, if is_active { theme::FG() } else { theme::FG_DIM() });
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
                if editor.saving {
                    let saving = mode_bar_text(ui, "Saving...", theme::FG_DIM());
                    ui.add(egui::Label::new(saving));
                }
                if let Some(error) = &editor.error {
                    let text = mode_bar_text(ui, error, theme::PALETTE_RED());
                    ui.add(egui::Label::new(text));
                }
                if let Some(path) = &editor.file_path {
                    let label = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().to_string());
                    let prefix = if editor.dirty { "* " } else { "" };
                    let text = mode_bar_text(ui, &format!("{prefix}{label}"), theme::FG_DIM());
                    ui.add(egui::Label::new(text));
                } else if editor.dirty {
                    let scratch = mode_bar_text(ui, "* scratch", theme::FG_DIM());
                    ui.add(egui::Label::new(scratch));
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

fn editor_text_id(panel_id: PanelId) -> Id {
    Id::unique(("horizon_editor_text", panel_id.0))
}

fn render_edit_pane(ui: &mut egui::Ui, panel: &mut Panel) -> Option<Rect> {
    let editor_id = editor_text_id(panel.id);
    let editor = panel.content.editor_mut()?;

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
            let font = FontId::monospace(FONT_SIZE);
            let mut layouter = move |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                let job =
                    egui::text::LayoutJob::simple(buffer.as_str().to_owned(), font.clone(), theme::FG(), wrap_width);
                // The editor is a panel of the canvas, so its text is under the
                // canvas zoom: it is laid out at the size of the screen.
                zoom_text::layout_job(ui.painter(), job)
            };
            let output = markdown_text_edit(
                &mut editor.text,
                viewport.size(),
                desired_rows,
                editor_id,
                &mut layouter,
            )
            .show(ui);
            if output.response.changed() {
                editor.dirty = true;
            }
            output.response.rect
        });
    Some(output.inner)
}

fn markdown_text_edit<'a>(
    text: &'a mut String,
    min_size: Vec2,
    desired_rows: usize,
    id: Id,
    layouter: &'a mut dyn FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> Arc<egui::Galley>,
) -> TextEdit<'a> {
    TextEdit::multiline(text)
        .id(id)
        .font(FontId::monospace(FONT_SIZE))
        .layouter(layouter)
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
                editor.request_save();
                ui.ctx().request_repaint();
            }
            ui.add_space(8.0);
        });
}

#[cfg(test)]
mod tests {
    use crate::test_egui::DiscardTextures;
    use std::path::PathBuf;

    use egui::{Align, Context, Layout, Pos2, Rect, UiBuilder, Vec2};
    use horizon_core::{AppearanceTheme, Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

    use super::render_edit_pane;
    use crate::theme;

    /// The mode bar is a panel of the canvas, so its two modes and the file name
    /// are made for the screen size of the zoom. A stock label and a stock button
    /// draw a picture of zoom 1 that the layer transform scales, and `Preview`
    /// blurs at a zoom of 67 percent.
    #[test]
    fn the_text_of_the_mode_bar_is_made_for_the_screen_size_of_the_zoom() {
        for display_scale in [1.0_f32, 2.0] {
            for zoom in [0.67_f32, 1.0, 1.5] {
                let (quads, worst_phase, worst_stretch) =
                    crate::zoom_text::measure_canvas_quads(display_scale, zoom, |ui, _| {
                        for (label, color) in [("Edit", theme::FG_DIM()), ("Preview", theme::FG())] {
                            let text = super::mode_bar_text(ui, label, color);
                            ui.add(egui::Button::new(text).corner_radius(4));
                        }
                        let name = super::mode_bar_text(ui, "* notes.md", theme::FG_DIM());
                        ui.add(egui::Label::new(name));
                    });
                assert!(quads > 15, "the mode bar draws its text: {quads}");
                assert!(
                    worst_phase <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph corner sits {worst_phase} device pixels from a whole pixel"
                );
                assert!(
                    worst_stretch <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph picture is stretched by {worst_stretch} device pixels"
                );
            }
        }
    }

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
                                |ui| render_edit_pane(ui, &mut panel).expect("editor response"),
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
}
