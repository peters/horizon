use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::{Arena, Options, parse_document};
use egui::{Align, Color32, CornerRadius, FontId, Key, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{MarkdownEditor, Panel, PreviewMode};

use crate::theme;

const FONT_SIZE: f32 = 14.0;
const MODE_BAR_HEIGHT: f32 = 28.0;

pub struct MarkdownEditorView<'a> {
    panel: &'a mut Panel,
    preview_cache: Option<&'a mut MarkdownPreviewCache>,
}

#[derive(Clone, Default)]
pub(crate) struct MarkdownPreviewCache {
    source: String,
    pixels_per_point_bits: u32,
    document: PreviewDocument,
}

#[derive(Clone, Default)]
struct PreviewDocument {
    blocks: Vec<PreviewBlock>,
}

#[derive(Clone)]
enum PreviewBlock {
    Heading {
        galley: Arc<egui::Galley>,
        level: u8,
    },
    Paragraph {
        galley: Arc<egui::Galley>,
        indent_level: u32,
    },
    CodeBlock(Arc<egui::Galley>),
    ListItem {
        bullet_galley: Arc<egui::Galley>,
        indent_level: u32,
        blocks: Vec<PreviewBlock>,
    },
    BlockQuote(Vec<PreviewBlock>),
    ThematicBreak,
}

impl<'a> MarkdownEditorView<'a> {
    pub fn new(panel: &'a mut Panel, preview_cache: Option<&'a mut MarkdownPreviewCache>) -> Self {
        Self { panel, preview_cache }
    }

    /// Renders the editor panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, _is_active_panel: bool) -> bool {
        let Some(editor) = self.panel.content.editor_mut() else {
            return false;
        };

        let clicked = ui.rect_contains_pointer(ui.max_rect());
        let mode_rect = render_mode_bar(ui, editor);
        let preview_cache = self.preview_cache.take();

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

        render_body(ui, self.panel, body_rect, mode, preview_cache);
        clicked
    }
}

impl MarkdownPreviewCache {
    fn document(&mut self, ctx: &egui::Context, markdown: &str) -> &PreviewDocument {
        let pixels_per_point_bits = ctx.pixels_per_point().to_bits();
        if self.source != markdown || self.pixels_per_point_bits != pixels_per_point_bits {
            self.source.clear();
            self.source.push_str(markdown);
            self.pixels_per_point_bits = pixels_per_point_bits;
            self.document = build_preview_document(ctx, markdown);
        }

        &self.document
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
        PreviewMode::Preview => {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| render_preview_pane(ui, panel, preview_cache),
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

            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(right)
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

fn render_preview_pane(ui: &mut egui::Ui, panel: &Panel, preview_cache: Option<&mut MarkdownPreviewCache>) {
    let Some(editor) = panel.content.editor() else {
        return;
    };
    let fallback_document;
    let document = if let Some(cache) = preview_cache {
        cache.document(ui.ctx(), &editor.text)
    } else {
        fallback_document = build_preview_document(ui.ctx(), &editor.text);
        &fallback_document
    };

    ScrollArea::vertical().id_salt("editor_preview").show(ui, |ui| {
        ui.add_space(4.0);
        render_preview_document(ui, document);
        ui.add_space(8.0);
    });
}

#[profiling::function]
fn build_preview_document(ctx: &egui::Context, markdown: &str) -> PreviewDocument {
    let arena = Arena::new();
    let options = Options::default();
    let root = parse_document(&arena, markdown, &options);
    ctx.fonts_mut(|fonts| {
        let mut blocks = Vec::new();
        for node in root.children() {
            collect_preview_blocks(fonts, node, 0, &mut blocks);
        }
        PreviewDocument { blocks }
    })
}

#[profiling::function]
fn render_preview_document(ui: &mut egui::Ui, document: &PreviewDocument) {
    for block in &document.blocks {
        render_preview_block(ui, block);
    }
}

fn collect_preview_blocks<'a>(
    fonts: &mut egui::epaint::text::FontsView<'_>,
    node: &'a comrak::nodes::AstNode<'a>,
    indent_level: u32,
    blocks: &mut Vec<PreviewBlock>,
) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Heading(heading) => {
            blocks.push(PreviewBlock::Heading {
                galley: fonts.layout_job(heading_layout_job(&collect_text(node), heading.level)),
                level: heading.level,
            });
        }
        NodeValue::Paragraph => {
            drop(data);
            blocks.push(PreviewBlock::Paragraph {
                galley: fonts.layout_job(build_inline_layout(node)),
                indent_level,
            });
        }
        NodeValue::CodeBlock(code_block) => {
            blocks.push(PreviewBlock::CodeBlock(code_block_galley(fonts, &code_block.literal)));
        }
        NodeValue::List(list) => {
            let list_type = list.list_type;
            let mut counter = list.start;
            drop(data);
            for child in node.children() {
                let bullet_text = if list_type == comrak::nodes::ListType::Ordered {
                    let bullet = format!("{counter}. ");
                    counter += 1;
                    bullet
                } else {
                    "\u{2022} ".to_string()
                };
                let mut item_blocks = Vec::new();
                for item_child in child.children() {
                    collect_preview_blocks(fonts, item_child, indent_level + 1, &mut item_blocks);
                }
                blocks.push(PreviewBlock::ListItem {
                    bullet_galley: bullet_galley(fonts, &bullet_text),
                    indent_level,
                    blocks: item_blocks,
                });
            }
        }
        NodeValue::BlockQuote => {
            drop(data);
            let mut quoted_blocks = Vec::new();
            for child in node.children() {
                collect_preview_blocks(fonts, child, indent_level, &mut quoted_blocks);
            }
            blocks.push(PreviewBlock::BlockQuote(quoted_blocks));
        }
        NodeValue::ThematicBreak => blocks.push(PreviewBlock::ThematicBreak),
        _ => {
            drop(data);
            for child in node.children() {
                collect_preview_blocks(fonts, child, indent_level, blocks);
            }
        }
    }
}

fn render_preview_block(ui: &mut egui::Ui, block: &PreviewBlock) {
    match block {
        PreviewBlock::Heading { galley, level } => render_heading_galley(ui, galley, *level),
        PreviewBlock::Paragraph { galley, indent_level } => {
            if *indent_level > 0 {
                ui.indent(*indent_level, |ui| {
                    ui.label(galley.clone());
                });
            } else {
                ui.label(galley.clone());
            }
            ui.add_space(4.0);
        }
        PreviewBlock::CodeBlock(galley) => render_code_block(ui, galley),
        PreviewBlock::ListItem {
            bullet_galley,
            indent_level,
            blocks,
        } => render_list_item(ui, bullet_galley, *indent_level, blocks),
        PreviewBlock::BlockQuote(blocks) => render_blockquote_blocks(ui, blocks),
        PreviewBlock::ThematicBreak => render_thematic_break(ui),
    }
}

fn heading_layout_job(text: &str, level: u8) -> egui::text::LayoutJob {
    let size = match level {
        1 => 22.0,
        2 => 18.0,
        3 => 16.0,
        _ => 14.0,
    };

    egui::text::LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), theme::FG)
}

fn render_heading_galley(ui: &mut egui::Ui, galley: &Arc<egui::Galley>, level: u8) {
    ui.add_space(4.0);
    ui.label(galley.clone());
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

fn code_block_galley(fonts: &mut egui::epaint::text::FontsView<'_>, literal: &str) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(
        literal.trim_end(),
        0.0,
        egui::TextFormat {
            font_id: FontId::monospace(13.0),
            color: theme::FG_SOFT,
            ..Default::default()
        },
    );
    fonts.layout_job(job)
}

fn render_code_block(ui: &mut egui::Ui, galley: &Arc<egui::Galley>) {
    egui::Frame::new()
        .fill(theme::BG_ELEVATED)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(8.0)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
        .show(ui, |ui| {
            ui.label(galley.clone());
        });
    ui.add_space(6.0);
}

fn bullet_galley(fonts: &mut egui::epaint::text::FontsView<'_>, bullet: &str) -> Arc<egui::Galley> {
    fonts.layout_job(egui::text::LayoutJob::simple_singleline(
        bullet.to_string(),
        FontId::proportional(FONT_SIZE),
        theme::FG_DIM,
    ))
}

fn render_list_item(ui: &mut egui::Ui, bullet_galley: &Arc<egui::Galley>, indent_level: u32, blocks: &[PreviewBlock]) {
    ui.horizontal(|ui| {
        let indent = u16::try_from(indent_level.saturating_add(1)).unwrap_or(u16::MAX);
        ui.add_space(f32::from(indent) * 16.0);
        ui.label(bullet_galley.clone());
        ui.vertical(|ui| {
            for block in blocks {
                render_preview_block(ui, block);
            }
        });
    });
    ui.add_space(4.0);
}

fn render_blockquote_blocks(ui: &mut egui::Ui, blocks: &[PreviewBlock]) {
    let left_x = ui.cursor().min.x;
    let start_y = ui.cursor().min.y;

    ui.indent("bq", |ui| {
        ui.add_space(4.0);
        for block in blocks {
            render_preview_block(ui, block);
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
