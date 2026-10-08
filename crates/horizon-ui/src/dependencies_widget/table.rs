//! The repository table: one row per repository, selected rows open the detail pane.

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use horizon_core::maintenance::portfolio::{Repository, text};

use super::{portfolio::State, tone, widgets};
use crate::theme;

pub(super) const HEADER_HEIGHT: f32 = 30.0;
pub(super) const ROW_HEIGHT: f32 = 28.0;
const SCROLL_GUTTER: f32 = 14.0;
const CELL_INSET: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Repository,
    Ecosystems,
    Groups,
    OpenPrs,
    Status,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Self::Repository => "Repository",
            Self::Ecosystems => "Ecosystems",
            Self::Groups => "Dependabot groups",
            Self::OpenPrs => "Open PRs",
            Self::Status => "Status",
        }
    }
}

/// Column fractions of the row width; the detail pane leaves no room for groups.
fn columns(compact: bool) -> &'static [(Column, f32, f32)] {
    if compact {
        &[
            (Column::Repository, 0.0, 0.43),
            (Column::Ecosystems, 0.43, 0.68),
            (Column::OpenPrs, 0.68, 0.79),
            (Column::Status, 0.79, 1.0),
        ]
    } else {
        &[
            (Column::Repository, 0.0, 0.31),
            (Column::Ecosystems, 0.31, 0.53),
            (Column::Groups, 0.53, 0.75),
            (Column::OpenPrs, 0.75, 0.84),
            (Column::Status, 0.84, 1.0),
        ]
    }
}

/// Returns true when the person asked to clear the search and filter.
pub(super) fn show(ui: &mut egui::Ui, repositories: &[&Repository<'_>], state: &mut State, height: f32) -> bool {
    let width = ui.available_width();
    let compact = width < 1000.0;
    let row_width = width - SCROLL_GUTTER;
    let (header, _) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::hover());
    for (column, start, end) in columns(compact) {
        let (left, right) = (header.left() + start * row_width, header.left() + end * row_width);
        let title = widgets::line(
            ui.painter(),
            &column.title().to_uppercase(),
            &FontId::proportional(11.0),
            theme::FG_DIM(),
            right - left - 2.0 * CELL_INSET,
        );
        let (anchor, align) = if *column == Column::OpenPrs {
            (pos2(right - CELL_INSET, header.center().y), Align2::RIGHT_CENTER)
        } else {
            (pos2(left + CELL_INSET, header.center().y), Align2::LEFT_CENTER)
        };
        widgets::paint_line(ui.painter(), anchor, align, title);
    }
    ui.painter().line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
    );
    let mut clear = false;
    let body = height - HEADER_HEIGHT - 4.0;
    ui.add_space(4.0);
    widgets::solid_scroll_area(ui)
        .id_salt("repository-table")
        .max_height(body)
        .min_scrolled_height(body)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if repositories.is_empty() {
                clear = empty(ui, body);
            }
            for repo in repositories {
                row(ui, row_width, compact, repo, state);
            }
        });
    clear
}

fn row(ui: &mut egui::Ui, width: f32, compact: bool, repo: &Repository<'_>, state: &mut State) {
    let selected = state.selected.as_deref() == Some(repo.name);
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, repo.name)
    });
    let painter = ui.painter();
    let fill = if selected {
        theme::alpha(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.28), 200)
    } else if response.hovered() {
        theme::alpha(theme::PANEL_BG_ALT(), 160)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 8, fill);
    if selected {
        painter.rect_filled(
            Rect::from_min_size(rect.min + vec2(0.0, 5.0), vec2(3.0, rect.height() - 10.0)),
            2,
            theme::ACCENT(),
        );
    } else if !response.hovered() {
        painter.line_segment(
            [
                pos2(rect.left() + CELL_INSET, rect.bottom()),
                pos2(rect.right() - CELL_INSET, rect.bottom()),
            ],
            Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE(), 140)),
        );
    }
    if response.has_focus() {
        painter.rect_stroke(rect, 8, Stroke::new(1.5, theme::FG()), StrokeKind::Inside);
    }
    for (column, start, end) in columns(compact) {
        let cell = Rect::from_x_y_ranges(
            (rect.left() + start * width)..=(rect.left() + end * width),
            rect.y_range(),
        );
        paint_cell(painter, cell, *column, repo, selected);
    }
    if response.clicked() {
        state.selected = if selected { None } else { Some(repo.name.to_owned()) };
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(text(repo.data, "status_reason", repo.name));
}

fn paint_cell(painter: &egui::Painter, cell: Rect, column: Column, repo: &Repository<'_>, selected: bool) {
    let left = pos2(cell.left() + CELL_INSET, cell.center().y);
    let room = cell.width() - 2.0 * CELL_INSET;
    match column {
        Column::Repository => {
            let (owner, name) = repo.owner_and_name();
            let mut job = crate::text::single_line_job(room);
            if !owner.is_empty() {
                job.append(
                    &format!("{owner}/"),
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::proportional(13.0),
                        color: if selected { theme::FG_SOFT() } else { theme::FG_DIM() },
                        ..Default::default()
                    },
                );
            }
            job.append(
                name,
                0.0,
                egui::TextFormat {
                    font_id: FontId::proportional(14.0),
                    color: theme::FG(),
                    ..Default::default()
                },
            );
            widgets::paint_line(painter, left, Align2::LEFT_CENTER, painter.layout_job(job));
        }
        Column::Ecosystems => tags(painter, left, room, &repo.ecosystems),
        Column::Groups => {
            let text = if repo.groups.is_empty() {
                "—".to_owned()
            } else {
                repo.groups.join(", ")
            };
            let color = if repo.groups.is_empty() {
                theme::FG_DIM()
            } else {
                theme::FG_SOFT()
            };
            let galley = widgets::line(painter, &text, &FontId::proportional(13.0), color, room);
            widgets::paint_line(painter, left, Align2::LEFT_CENTER, galley);
        }
        Column::OpenPrs => {
            let open = repo.open_prs();
            let (text, color) = if open == 0 {
                ("—".to_owned(), theme::FG_DIM())
            } else {
                (open.to_string(), theme::FG())
            };
            painter.text(
                pos2(cell.right() - CELL_INSET, cell.center().y),
                Align2::RIGHT_CENTER,
                text,
                FontId::monospace(13.5),
                color,
            );
        }
        Column::Status => {
            widgets::paint_pill(painter, left, repo.status.label(), tone::color(repo.status.tone()));
        }
    }
}

/// Ecosystem tags left to right; the ones that do not fit are counted.
fn tags(painter: &egui::Painter, left: egui::Pos2, room: f32, ecosystems: &[&str]) {
    if ecosystems.is_empty() {
        painter.text(
            left,
            Align2::LEFT_CENTER,
            "—",
            FontId::proportional(13.0),
            theme::FG_DIM(),
        );
        return;
    }
    let mut x = left.x;
    for (index, ecosystem) in ecosystems.iter().enumerate() {
        let width = widgets::tag_width(painter, ecosystem);
        let remaining = ecosystems.len() - index - 1;
        let reserve = if remaining > 0 { 34.0 } else { 0.0 };
        if x + width + reserve > left.x + room {
            painter.text(
                pos2(x, left.y),
                Align2::LEFT_CENTER,
                format!("+{}", ecosystems.len() - index),
                FontId::monospace(12.0),
                theme::FG_DIM(),
            );
            return;
        }
        x = widgets::paint_tag(painter, pos2(x, left.y), ecosystem).right() + 6.0;
    }
}

fn empty(ui: &mut egui::Ui, height: f32) -> bool {
    let mut clear = false;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), height * 0.6),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.add_space(height * 0.18);
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
            ui.painter().rect_filled(badge, 12, theme::PANEL_BG_ALT());
            widgets::magnifier(ui.painter(), badge.center(), theme::FG_DIM());
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("No repositories match")
                    .size(15.0)
                    .strong()
                    .color(theme::FG()),
            );
            ui.label(
                egui::RichText::new("Try another name, ecosystem or pull request title.")
                    .size(13.0)
                    .color(theme::FG_SOFT()),
            );
            ui.add_space(8.0);
            clear = ui.add(widgets::chrome_button("Clear search and filter")).clicked();
        },
    );
    clear
}
