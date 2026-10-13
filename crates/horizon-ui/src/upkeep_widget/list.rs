//! Filter chips and the pull request rows, grouped by where each one stands.
use egui::{
    Align, CornerRadius, CursorIcon, Frame, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use horizon_core::upkeep::time::OffsetDateTime;
use horizon_core::upkeep::{PrState, PullRequest, relative};

use super::{Filter, caption, checks_look, pill, state_color};
use crate::theme;

const ROW_HEIGHT: f32 = 34.0;
const NOTE_HEIGHT: f32 = 16.0;
const REPO_WIDTH: f32 = 168.0;
const CHECKS_WIDTH: f32 = 118.0;
const AGE_WIDTH: f32 = 64.0;
/// Merged rows shown before "and N more".
const MERGED_ROWS: usize = 6;

pub(super) fn chips(ui: &mut Ui, shown: &[&PullRequest], filter: &mut Filter) {
    let count = |admit: &dyn Fn(PrState) -> bool| shown.iter().filter(|pr| admit(pr.state)).count();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        chip(ui, filter, Filter::Open, "Open", count(&|s| s != PrState::Merged), None);
        chip(
            ui,
            filter,
            Filter::NeedsPerson,
            "Needs you",
            count(&PrState::needs_person),
            Some(theme::PALETTE_YELLOW()),
        );
        for state in PrState::ORDER {
            if state.needs_person() {
                continue;
            }
            let label = if state == PrState::Merged {
                "Merged, 7 days"
            } else {
                state.label()
            };
            chip(
                ui,
                filter,
                Filter::State(state),
                label,
                count(&|s| s == state),
                Some(state_color(state)),
            );
        }
    });
}

/// A choice chip: the count in monospace, the label, and a dot in the state color.
fn chip(ui: &mut Ui, filter: &mut Filter, value: Filter, label: &str, count: usize, dot: Option<egui::Color32>) {
    let selected = *filter == value;
    let fill = if selected {
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22)
    } else {
        theme::PANEL_BG_ALT()
    };
    let stroke = if selected {
        Stroke::new(1.0, theme::ACCENT())
    } else {
        Stroke::new(1.0, theme::BORDER_SUBTLE())
    };
    let response = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(color) = dot {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
            }
            ui.add(
                egui::Label::new(
                    RichText::new(count.to_string())
                        .size(12.5)
                        .strong()
                        .monospace()
                        .color(theme::FG()),
                )
                .selectable(false),
            );
            ui.add(egui::Label::new(RichText::new(label).size(12.0).color(theme::FG_SOFT())).selectable(false));
        })
        .response;
    let response = ui
        .interact(response.rect, ui.id().with(("upkeep-chip", label)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    if response.hovered() && !selected {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(8),
            Stroke::new(1.0, theme::FG_DIM()),
            StrokeKind::Inside,
        );
    }
    if response.clicked() {
        *filter = value;
    }
}

/// The rows the filter admits, grouped by state. Returns the page of a clicked row.
pub(super) fn show(ui: &mut Ui, shown: &[&PullRequest], filter: Filter, now: OffsetDateTime) -> Option<String> {
    let mut opened = None;
    let mut any = false;
    for state in PrState::ORDER {
        if !filter.admits(state) {
            continue;
        }
        let rows: Vec<_> = shown.iter().filter(|pr| pr.state == state).collect();
        if rows.is_empty() {
            continue;
        }
        any = true;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            pill(ui, state.label(), state_color(state));
            ui.label(
                RichText::new(rows.len().to_string())
                    .size(11.0)
                    .monospace()
                    .color(theme::FG_DIM()),
            );
        });
        let limit = if state == PrState::Merged {
            MERGED_ROWS
        } else {
            usize::MAX
        };
        for pr in rows.iter().take(limit) {
            if let Some(url) = row(ui, pr, now) {
                opened = Some(url);
            }
        }
        if rows.len() > limit {
            ui.label(
                RichText::new(format!("and {} more", rows.len() - limit))
                    .size(11.5)
                    .color(theme::FG_SOFT()),
            );
        }
    }
    if !any {
        ui.add_space(8.0);
        let line = if filter == Filter::NeedsPerson {
            "Nothing needs you. Upkeep handles the rest."
        } else {
            "No pull requests here."
        };
        caption(ui, line);
    }
    opened
}

/// One pull request: repository, title, checks and age. The whole row opens it.
fn row(ui: &mut Ui, pr: &PullRequest, now: OffsetDateTime) -> Option<String> {
    let has_note = !pr.note.is_empty();
    let height = if has_note { ROW_HEIGHT + NOTE_HEIGHT } else { ROW_HEIGHT };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    // A row out of view only takes its space: a long list costs nothing per frame.
    if !ui.is_rect_visible(rect) {
        return None;
    }
    // The page and the texts are made only when the row is pointed at or clicked.
    let url = (response.hovered() || response.clicked())
        .then(|| pr.github_url())
        .flatten();
    let response = if url.is_some() {
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_ui(|ui| {
            ui.label(format!("Open #{} in a browser panel", pr.number));
        })
    } else {
        response
    };
    response.widget_info(|| {
        let name = format!("{}#{} {}", pr.repository, pr.number, pr.title);
        egui::WidgetInfo::labeled(egui::WidgetType::Button, pr.github_url().is_some(), name)
    });
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            theme::blend(theme::PANEL_BG(), theme::FG(), 0.05),
        );
    }
    let inner = rect.shrink2(Vec2::new(10.0, 0.0));
    ui.scope_builder(
        UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.set_height(ROW_HEIGHT);
                ui.spacing_mut().item_spacing.x = 10.0;
                let repo = pr
                    .repository
                    .split_once('/')
                    .map_or(pr.repository.as_str(), |(_, name)| name);
                ui.allocate_ui_with_layout(
                    Vec2::new(REPO_WIDTH, ROW_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_width(REPO_WIDTH);
                        ui.add(
                            egui::Label::new(RichText::new(repo).size(11.5).monospace().color(theme::FG_SOFT()))
                                .truncate()
                                .selectable(false),
                        );
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let age = pr.updated_at.map(|at| relative(at, now)).unwrap_or_default();
                    ui.add_sized(
                        [AGE_WIDTH, ROW_HEIGHT],
                        egui::Label::new(RichText::new(age).size(11.0).monospace().color(theme::FG_DIM()))
                            .selectable(false),
                    );
                    checks(ui, pr);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("#{}", pr.number))
                                    .size(11.5)
                                    .monospace()
                                    .color(theme::ACCENT()),
                            )
                            .selectable(false),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&pr.title).size(13.0).color(theme::FG()))
                                .truncate()
                                .selectable(false),
                        );
                    });
                });
            });
            if has_note {
                ui.horizontal(|ui| {
                    ui.add_space(REPO_WIDTH + 10.0);
                    ui.add(
                        egui::Label::new(RichText::new(&pr.note).size(11.5).color(theme::FG_SOFT()))
                            .truncate()
                            .selectable(false),
                    );
                });
            }
        },
    );
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE(), 180)),
    );
    response.clicked().then_some(url).flatten()
}

fn checks(ui: &mut Ui, pr: &PullRequest) {
    let (color, text) = checks_look(pr.checks);
    ui.allocate_ui_with_layout(
        Vec2::new(CHECKS_WIDTH, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, color);
            ui.add(egui::Label::new(RichText::new(text).size(11.5).color(theme::FG_SOFT())).selectable(false));
        },
    );
}
