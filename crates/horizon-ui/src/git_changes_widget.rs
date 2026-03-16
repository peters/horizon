use egui::{Align, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{DiffLineKind, FileStatus, GitStatus, Panel};

use crate::theme;

const SECTION_LABEL_SIZE: f32 = 10.0;
const FILE_ROW_SIZE: f32 = 11.0;
const DIFF_FONT_SIZE: f32 = 11.0;
const SUMMARY_SIZE: f32 = 11.0;
const HEADER_HEIGHT: f32 = 28.0;

const DIFF_ADD_BG: Color32 = Color32::from_rgba_premultiplied(166, 227, 161, 18);
const DIFF_DEL_BG: Color32 = Color32::from_rgba_premultiplied(243, 139, 168, 18);

pub struct GitChangesView<'a> {
    panel: &'a mut Panel,
}

impl<'a> GitChangesView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Renders the git changes panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, _is_focused: bool) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());

        let Some(viewer) = self.panel.content.git_changes() else {
            return clicked;
        };

        let status = viewer.status.clone();

        match status {
            None => render_scanning(ui),
            Some(ref status) if status.changes.is_empty() => render_clean(ui, status),
            Some(ref status) => {
                render_header(ui, status);
                render_summary(ui, status);
                render_file_list(ui, self.panel, status);
            }
        }

        clicked
    }
}

fn render_scanning(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(RichText::new("Scanning repository...").size(12.0).color(theme::FG_DIM));
    });
}

fn render_clean(ui: &mut egui::Ui, status: &GitStatus) {
    render_header(ui, status);
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(RichText::new("Working tree clean").size(13.0).color(theme::FG_DIM));
    });
}

fn render_header(ui: &mut egui::Ui, status: &GitStatus) {
    let header_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), HEADER_HEIGHT));
    ui.allocate_rect(header_rect, egui::Sense::hover());

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(header_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add_space(12.0);

            // Branch badge
            if let Some(branch) = &status.branch {
                let badge = egui::Frame::new()
                    .fill(theme::alpha(Color32::from_rgb(203, 166, 247), 20))
                    .corner_radius(CornerRadius::same(4));
                badge.show(ui, |ui| {
                    ui.label(
                        RichText::new(branch)
                            .font(FontId::monospace(10.0))
                            .color(Color32::from_rgb(203, 166, 247)),
                    );
                });
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                // File count badge
                let count = status.file_count();
                if count > 0 {
                    let badge = egui::Frame::new()
                        .fill(theme::PANEL_BG_ALT)
                        .corner_radius(CornerRadius::same(8));
                    badge.show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{count} files"))
                                .font(FontId::monospace(10.0))
                                .color(theme::FG_DIM),
                        );
                    });
                }
            });
        },
    );

    // Separator
    let sep_y = header_rect.max.y;
    ui.painter().line_segment(
        [Pos2::new(header_rect.min.x, sep_y), Pos2::new(header_rect.max.x, sep_y)],
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
    );
}

fn render_summary(ui: &mut egui::Ui, status: &GitStatus) {
    let summary_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 24.0));
    ui.allocate_rect(summary_rect, egui::Sense::hover());

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(summary_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!("+{}", status.total_insertions))
                    .font(FontId::monospace(SUMMARY_SIZE))
                    .color(theme::PALETTE_GREEN),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("\u{2212}{}", status.total_deletions))
                    .font(FontId::monospace(SUMMARY_SIZE))
                    .color(theme::PALETTE_RED),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("across {} files", status.file_count()))
                    .font(FontId::monospace(SUMMARY_SIZE))
                    .color(theme::FG_DIM),
            );
        },
    );

    // Thin separator
    let sep_y = summary_rect.max.y;
    ui.painter().line_segment(
        [
            Pos2::new(summary_rect.min.x, sep_y),
            Pos2::new(summary_rect.max.x, sep_y),
        ],
        egui::Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 128)),
    );
}

fn render_file_list(ui: &mut egui::Ui, panel: &mut Panel, status: &GitStatus) {
    // Collect toggle actions so we can apply them after borrowing status.
    let mut toggle_path: Option<String> = None;

    ScrollArea::vertical()
        .id_salt(("git_changes_files", panel.id.0))
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Group files by status.
            let mut current_status: Option<FileStatus> = None;
            for change in &status.changes {
                if current_status != Some(change.status) {
                    current_status = Some(change.status);
                    render_section_label(ui, change.status, &status.changes);
                }

                let is_expanded = panel.content.git_changes().is_some_and(|v| v.is_expanded(&change.path));

                if render_file_row(ui, change, is_expanded) {
                    toggle_path = Some(change.path.clone());
                }

                if is_expanded && let Some(diff) = status.diffs.get(&change.path) {
                    render_inline_diff(ui, diff, panel.id.0);
                }
            }

            // Timestamp footer
            ui.add_space(8.0);
            let elapsed = status.timestamp.elapsed().as_secs();
            let ago = if elapsed < 2 {
                "just now".to_string()
            } else {
                format!("{elapsed}s ago")
            };
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new(format!("Last scan {ago}"))
                        .font(FontId::monospace(9.5))
                        .color(theme::FG_DIM),
                );
            });
            ui.add_space(4.0);
        });

    if let Some(path) = toggle_path
        && let Some(viewer) = panel.content.git_changes_mut()
    {
        viewer.toggle_file(&path);
    }
}

fn render_section_label(ui: &mut egui::Ui, status: FileStatus, changes: &[horizon_core::FileChange]) {
    let count = changes.iter().filter(|c| c.status == status).count();
    let (label, color) = match status {
        FileStatus::Modified => ("MODIFIED", theme::ACCENT),
        FileStatus::Added => ("ADDED", theme::PALETTE_GREEN),
        FileStatus::Deleted => ("DELETED", theme::PALETTE_RED),
        FileStatus::Renamed => ("RENAMED", Color32::from_rgb(249, 226, 175)),
    };

    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add_space(0.0); // force spacing
        ui.label(
            RichText::new(format!("{label} \u{00b7} {count}"))
                .size(SECTION_LABEL_SIZE)
                .color(theme::alpha(color, 160))
                .strong(),
        );
    });
    ui.add_space(2.0);
}

fn render_file_row(ui: &mut egui::Ui, change: &horizon_core::FileChange, is_expanded: bool) -> bool {
    let mut clicked = false;

    let row_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 22.0));

    let response = ui.allocate_rect(row_rect, egui::Sense::click());
    if response.clicked() {
        clicked = true;
    }

    // Hover highlight
    if response.hovered() {
        ui.painter()
            .rect_filled(row_rect, CornerRadius::ZERO, theme::alpha(theme::FG, 6));
    }
    if is_expanded {
        ui.painter()
            .rect_filled(row_rect, CornerRadius::ZERO, theme::alpha(theme::ACCENT, 8));
    }

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(row_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add_space(14.0);

            // Expand arrow
            let arrow = if is_expanded { "\u{25bc}" } else { "\u{25b6}" };
            ui.label(
                RichText::new(arrow)
                    .size(7.0)
                    .color(if is_expanded { theme::ACCENT } else { theme::FG_DIM }),
            );
            ui.add_space(4.0);

            // Status indicator
            let (indicator, color) = match change.status {
                FileStatus::Modified => ("M", theme::ACCENT),
                FileStatus::Added => ("A", theme::PALETTE_GREEN),
                FileStatus::Deleted => ("D", theme::PALETTE_RED),
                FileStatus::Renamed => ("R", Color32::from_rgb(249, 226, 175)),
            };
            ui.label(
                RichText::new(indicator)
                    .font(FontId::monospace(FILE_ROW_SIZE))
                    .color(color)
                    .strong(),
            );
            ui.add_space(6.0);

            // File path (dir in dim, filename in normal)
            let (dir, file) = split_path(&change.path);
            if !dir.is_empty() {
                ui.label(
                    RichText::new(dir)
                        .font(FontId::monospace(FILE_ROW_SIZE))
                        .color(theme::FG_DIM),
                );
            }
            ui.label(
                RichText::new(file)
                    .font(FontId::monospace(FILE_ROW_SIZE))
                    .color(theme::FG_SOFT),
            );

            // +/- stats on the right
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                if change.deletions > 0 {
                    ui.label(
                        RichText::new(format!("\u{2212}{}", change.deletions))
                            .font(FontId::monospace(10.0))
                            .color(theme::PALETTE_RED),
                    );
                }
                if change.insertions > 0 {
                    ui.label(
                        RichText::new(format!("+{}", change.insertions))
                            .font(FontId::monospace(10.0))
                            .color(theme::PALETTE_GREEN),
                    );
                }
            });
        },
    );

    clicked
}

fn render_inline_diff(ui: &mut egui::Ui, diff: &horizon_core::FileDiff, panel_id: u64) {
    let frame = egui::Frame::new()
        .fill(theme::BG_ELEVATED)
        .corner_radius(CornerRadius::same(6))
        .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
        .inner_margin(egui::Margin::same(0));

    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.vertical(|ui| {
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
                    render_diff_hunk(ui, hunk);

                    // Add gap between hunks (but not after the last one).
                    let _ = (panel_id, hunk_idx); // suppress unused warnings
                }
            });
        });
    });

    ui.add_space(4.0);
}

fn render_diff_hunk(ui: &mut egui::Ui, hunk: &horizon_core::DiffHunk) {
    // Hunk header
    let hunk_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 18.0));
    ui.allocate_rect(hunk_rect, egui::Sense::hover());
    ui.painter()
        .rect_filled(hunk_rect, CornerRadius::ZERO, theme::alpha(theme::ACCENT, 12));
    ui.painter().text(
        Pos2::new(hunk_rect.min.x + 10.0, hunk_rect.center().y),
        egui::Align2::LEFT_CENTER,
        &hunk.header,
        FontId::monospace(10.0),
        theme::FG_DIM,
    );

    // Separator below hunk header
    let sep_y = hunk_rect.max.y;
    ui.painter().line_segment(
        [Pos2::new(hunk_rect.min.x, sep_y), Pos2::new(hunk_rect.max.x, sep_y)],
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
    );

    // Diff lines
    let max_lines = 50;
    let lines_to_show = hunk.lines.len().min(max_lines);

    for line in hunk.lines.iter().take(lines_to_show) {
        render_diff_line(ui, line);
    }

    if hunk.lines.len() > max_lines {
        let more = hunk.lines.len() - max_lines;
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new(format!("... {more} more lines"))
                    .font(FontId::monospace(10.0))
                    .color(theme::FG_DIM),
            );
        });
    }
}

fn render_diff_line(ui: &mut egui::Ui, line: &horizon_core::DiffLine) {
    let line_height = 16.0;
    let line_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), line_height));
    ui.allocate_rect(line_rect, egui::Sense::hover());

    // Background
    let bg = match line.kind {
        DiffLineKind::Add => DIFF_ADD_BG,
        DiffLineKind::Delete => DIFF_DEL_BG,
        DiffLineKind::Context => Color32::TRANSPARENT,
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(line_rect, CornerRadius::ZERO, bg);
    }

    // Line number
    let lineno = match line.kind {
        DiffLineKind::Delete => line.old_lineno,
        DiffLineKind::Add | DiffLineKind::Context => line.new_lineno,
    };
    let lineno_text = lineno.map_or_else(String::new, |n| format!("{n:>4}"));
    let lineno_color = match line.kind {
        DiffLineKind::Add => theme::alpha(theme::PALETTE_GREEN, 100),
        DiffLineKind::Delete => theme::alpha(theme::PALETTE_RED, 100),
        DiffLineKind::Context => theme::alpha(theme::FG_DIM, 100),
    };
    ui.painter().text(
        Pos2::new(line_rect.min.x + 4.0, line_rect.center().y),
        egui::Align2::LEFT_CENTER,
        &lineno_text,
        FontId::monospace(DIFF_FONT_SIZE),
        lineno_color,
    );

    // Content
    let prefix = match line.kind {
        DiffLineKind::Add => "+",
        DiffLineKind::Delete => "\u{2212}",
        DiffLineKind::Context => " ",
    };
    let content_color = match line.kind {
        DiffLineKind::Add => theme::PALETTE_GREEN,
        DiffLineKind::Delete => theme::PALETTE_RED,
        DiffLineKind::Context => theme::FG_DIM,
    };
    let display = format!("{prefix}{}", line.content.trim_end_matches('\n'));
    ui.painter().text(
        Pos2::new(line_rect.min.x + 40.0, line_rect.center().y),
        egui::Align2::LEFT_CENTER,
        &display,
        FontId::monospace(DIFF_FONT_SIZE),
        content_color,
    );
}

fn split_path(path: &str) -> (&str, &str) {
    if let Some(pos) = path.rfind('/') {
        (&path[..=pos], &path[pos + 1..])
    } else {
        ("", path)
    }
}
