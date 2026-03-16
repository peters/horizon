use comrak::nodes::NodeValue;
use comrak::{Arena, Options, parse_document};
use egui::{Align, Color32, CornerRadius, FontId, Key, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{MarkdownEditor, Panel, PreviewMode};

use crate::theme;

const FONT_SIZE: f32 = 14.0;
const MODE_BAR_HEIGHT: f32 = 28.0;

pub struct MarkdownEditorView<'a> {
    panel: &'a mut Panel,
}

impl<'a> MarkdownEditorView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Renders the editor panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, _is_active_panel: bool) -> bool {
        let Some(editor) = self.panel.content.editor_mut() else {
            return false;
        };

        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let mode_rect = render_mode_bar(ui, editor);

        // Body area
        let body_rect = Rect::from_min_max(Pos2::new(ui.cursor().min.x, mode_rect.max.y + 2.0), ui.max_rect().max);

        // Handle Ctrl+S
        if ui.input(|i| i.modifiers.command && i.key_pressed(Key::S))
            && let Some(ed) = self.panel.content.editor_mut()
        {
            ed.save_if_dirty();
        }

        let mode = self
            .panel
            .content
            .editor()
            .map_or(PreviewMode::Edit, |e| e.preview_mode);

        render_body(ui, self.panel, body_rect, mode);
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
            for (mode, label) in [
                (PreviewMode::Edit, "Edit"),
                (PreviewMode::Split, "Split"),
                (PreviewMode::Preview, "Preview"),
            ] {
                let active = editor.preview_mode == mode;
                let text = RichText::new(label)
                    .size(11.0)
                    .color(if active { theme::FG } else { theme::FG_DIM });
                let button = egui::Button::new(text)
                    .fill(if active {
                        theme::PANEL_BG_ALT
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(CornerRadius::same(4));
                if ui.add(button).clicked() {
                    editor.preview_mode = mode;
                }
            }

            // Dirty indicator + file path
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if let Some(path) = &editor.file_path {
                    let label = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string());
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

fn render_body(ui: &mut egui::Ui, panel: &mut Panel, body_rect: Rect, mode: PreviewMode) {
    match mode {
        PreviewMode::Edit => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_edit_pane(ui, panel),
            );
        }
        PreviewMode::Preview => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    let text = panel.content.editor().map_or(String::new(), |e| e.text.clone());
                    render_preview_pane(ui, &text);
                },
            );
        }
        PreviewMode::Split => {
            let mid = body_rect.min.x + body_rect.width() / 2.0 - 2.0;
            let left = Rect::from_min_max(body_rect.min, Pos2::new(mid, body_rect.max.y));
            let right = Rect::from_min_max(Pos2::new(mid + 4.0, body_rect.min.y), body_rect.max);

            ui.painter().line_segment(
                [
                    Pos2::new(mid + 1.0, body_rect.min.y),
                    Pos2::new(mid + 1.0, body_rect.max.y),
                ],
                egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
            );

            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(left)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_edit_pane(ui, panel),
            );

            let text = panel.content.editor().map_or(String::new(), |e| e.text.clone());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(right)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_preview_pane(ui, &text),
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

fn render_preview_pane(ui: &mut egui::Ui, text: &str) {
    ScrollArea::vertical().id_salt("editor_preview").show(ui, |ui| {
        ui.add_space(4.0);
        render_markdown(ui, text);
        ui.add_space(8.0);
    });
}

fn render_markdown(ui: &mut egui::Ui, markdown: &str) {
    let arena = Arena::new();
    let options = Options::default();
    let root = parse_document(&arena, markdown, &options);

    for node in root.children() {
        render_node(ui, node, 0);
    }
}

fn render_node<'a>(ui: &mut egui::Ui, node: &'a comrak::nodes::AstNode<'a>, indent_level: u32) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Heading(heading) => {
            render_heading(ui, node, heading.level);
        }
        NodeValue::Paragraph => {
            drop(data);
            let job = build_inline_layout(node);
            if indent_level > 0 {
                ui.indent(indent_level, |ui| {
                    ui.label(job);
                });
            } else {
                ui.label(job);
            }
            ui.add_space(4.0);
        }
        NodeValue::CodeBlock(code_block) => {
            render_code_block(ui, &code_block.literal);
        }
        NodeValue::List(list) => {
            render_list(ui, node, list, indent_level);
        }
        NodeValue::BlockQuote => {
            render_blockquote(ui, node, indent_level);
        }
        NodeValue::ThematicBreak => {
            render_thematic_break(ui);
        }
        _ => {
            for child in node.children() {
                render_node(ui, child, indent_level);
            }
        }
    }
}

fn render_heading<'a>(ui: &mut egui::Ui, node: &'a comrak::nodes::AstNode<'a>, level: u8) {
    let text = collect_text(node);
    let size = match level {
        1 => 22.0,
        2 => 18.0,
        3 => 16.0,
        _ => 14.0,
    };
    ui.add_space(4.0);
    ui.label(RichText::new(&text).size(size).strong().color(theme::FG));
    ui.add_space(2.0);
    if level <= 2 {
        let rect = ui.cursor();
        ui.painter().line_segment(
            [
                Pos2::new(rect.min.x, rect.min.y),
                Pos2::new(rect.min.x + ui.available_width(), rect.min.y),
            ],
            egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
        );
        ui.add_space(4.0);
    }
}

fn render_code_block(ui: &mut egui::Ui, literal: &str) {
    let code = literal.trim_end();
    egui::Frame::new()
        .fill(theme::BG_ELEVATED)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(8.0)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
        .show(ui, |ui| {
            ui.label(RichText::new(code).font(FontId::monospace(13.0)).color(theme::FG_SOFT));
        });
    ui.add_space(6.0);
}

fn render_list<'a>(
    ui: &mut egui::Ui,
    node: &'a comrak::nodes::AstNode<'a>,
    list: &comrak::nodes::NodeList,
    indent_level: u32,
) {
    let mut counter = list.start;
    for child in node.children() {
        let bullet = if list.list_type == comrak::nodes::ListType::Ordered {
            let s = format!("{counter}. ");
            counter += 1;
            s
        } else {
            "\u{2022} ".to_string()
        };

        ui.horizontal(|ui| {
            ui.add_space((indent_level + 1) as f32 * 16.0);
            ui.label(RichText::new(bullet).size(FONT_SIZE).color(theme::FG_DIM));
            ui.vertical(|ui| {
                for item_child in child.children() {
                    render_node(ui, item_child, indent_level + 1);
                }
            });
        });
    }
    ui.add_space(4.0);
}

fn render_blockquote<'a>(ui: &mut egui::Ui, node: &'a comrak::nodes::AstNode<'a>, indent_level: u32) {
    let left_x = ui.cursor().min.x;
    let start_y = ui.cursor().min.y;

    ui.indent("bq", |ui| {
        ui.add_space(4.0);
        for child in node.children() {
            render_node(ui, child, indent_level);
        }
    });

    let end_y = ui.cursor().min.y;
    ui.painter().rect_filled(
        Rect::from_min_size(Pos2::new(left_x + 4.0, start_y), Vec2::new(3.0, end_y - start_y)),
        CornerRadius::same(1),
        theme::ACCENT,
    );
}

fn render_thematic_break(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let rect = ui.cursor();
    ui.painter().line_segment(
        [
            Pos2::new(rect.min.x, rect.min.y),
            Pos2::new(rect.min.x + ui.available_width(), rect.min.y),
        ],
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
    );
    ui.add_space(8.0);
}

/// Build a `LayoutJob` from the inline children of a block-level node,
/// handling bold, italic, code, links, etc.
fn build_inline_layout<'a>(node: &'a comrak::nodes::AstNode<'a>) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    collect_inline_job(node, &mut job, false, false);
    job
}

fn collect_inline_job<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    job: &mut egui::text::LayoutJob,
    bold: bool,
    italic: bool,
) {
    for child in node.children() {
        let data = child.data.borrow();
        match &data.value {
            NodeValue::Text(text) => {
                let font_id = if bold {
                    FontId::new(FONT_SIZE, egui::FontFamily::Proportional)
                } else {
                    FontId::proportional(FONT_SIZE)
                };
                let format = egui::TextFormat {
                    font_id,
                    color: theme::FG,
                    italics: italic,
                    ..Default::default()
                };
                job.append(text, 0.0, format);
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                job.append(
                    "\n",
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::proportional(FONT_SIZE),
                        color: theme::FG,
                        ..Default::default()
                    },
                );
            }
            NodeValue::Code(code) => {
                job.append(
                    &code.literal,
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::monospace(13.0),
                        color: theme::FG_SOFT,
                        background: theme::BG_ELEVATED,
                        ..Default::default()
                    },
                );
            }
            NodeValue::Emph => {
                drop(data);
                collect_inline_job(child, job, bold, true);
            }
            NodeValue::Strong => {
                drop(data);
                collect_inline_job(child, job, true, italic);
            }
            NodeValue::Link(_link) => {
                let link_text = collect_text(child);
                let format = egui::TextFormat {
                    font_id: FontId::proportional(FONT_SIZE),
                    color: theme::ACCENT,
                    underline: egui::Stroke::new(1.0, theme::ACCENT),
                    italics: italic,
                    ..Default::default()
                };
                job.append(&link_text, 0.0, format);
            }
            _ => {
                drop(data);
                collect_inline_job(child, job, bold, italic);
            }
        }
    }
}

/// Recursively collect plain text from a node and its children.
fn collect_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let mut text = String::new();
    collect_text_inner(node, &mut text);
    text
}

fn collect_text_inner<'a>(node: &'a comrak::nodes::AstNode<'a>, buf: &mut String) {
    let data = node.data.borrow();
    if let NodeValue::Text(t) = &data.value {
        buf.push_str(t);
    } else if let NodeValue::Code(c) = &data.value {
        buf.push_str(&c.literal);
    } else if matches!(data.value, NodeValue::SoftBreak | NodeValue::LineBreak) {
        buf.push(' ');
    }
    drop(data);
    for child in node.children() {
        collect_text_inner(child, buf);
    }
}
