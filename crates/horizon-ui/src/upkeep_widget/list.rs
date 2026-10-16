//! Filter chips and the pull request rows, grouped by where each one stands.
use egui::{
    Align, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2,
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
pub(super) fn chips(ui: &mut Ui, shown: &[&PullRequest], filter: &mut Filter) {
    let count = |value: Filter| shown.iter().filter(|pr| value.admits(pr)).count();
    // The Other chip leaves with its last row; its filter goes with it.
    if *filter == Filter::State(PrState::Other) && count(Filter::State(PrState::Other)) == 0 {
        *filter = Filter::Open;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        chip(ui, filter, Filter::Open, "Open", count(Filter::Open), None);
        chip(
            ui,
            filter,
            Filter::NeedsPerson,
            "Needs you",
            count(Filter::NeedsPerson),
            Some(theme::PALETTE_YELLOW()),
        );
        chip(
            ui,
            filter,
            Filter::NeedsTooling,
            "Needs tooling",
            count(Filter::NeedsTooling),
            Some(state_color(PrState::Blocked)),
        );
        for state in PrState::ORDER {
            // "Other" appears only for a state this Horizon does not know.
            if state.needs_person() || (state == PrState::Other && count(Filter::State(state)) == 0) {
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
                count(Filter::State(state)),
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
    // A chip that does not fit moves to the next row whole: the wrapped row would break its label.
    let text = |text: String, font: FontId| ui.painter().layout_no_wrap(text, font, theme::FG()).size().x;
    let width = 2.0 * 10.0
        + dot.map_or(0.0, |_| 7.0 + 6.0)
        + text(count.to_string(), FontId::monospace(12.5))
        + 6.0
        + text(label.to_owned(), FontId::proportional(12.0));
    if width > ui.available_size_before_wrap().x && ui.cursor().min.x > ui.max_rect().min.x {
        ui.end_row();
    }
    let response = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            // One centered line: the dot stays level with the text on every row.
            ui.horizontal(|ui| {
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
                ui.add(
                    egui::Label::new(RichText::new(label).size(12.0).color(theme::FG_SOFT()))
                        .selectable(false)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
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
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            CornerRadius::same(10),
            Stroke::new(1.5, theme::FG()),
            StrokeKind::Outside,
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
        let admits = |pr: &&&PullRequest| pr.state == state && filter.admits(pr);
        // Counted and then drawn from the same slice: no list is built per frame.
        let count = shown.iter().filter(admits).count();
        if count == 0 {
            continue;
        }
        any = true;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            pill(ui, state.label(), state_color(state));
            ui.label(
                RichText::new(count.to_string())
                    .size(11.0)
                    .monospace()
                    .color(theme::FG_DIM()),
            );
        });
        for pr in shown.iter().filter(admits) {
            if let Some(url) = row(ui, pr, now) {
                opened = Some(url);
            }
        }
    }
    if !any {
        ui.add_space(8.0);
        let line = match filter {
            Filter::NeedsPerson => "Nothing needs you. Upkeep handles the rest.",
            Filter::NeedsTooling => "Nothing waits for tooling that the worker lacks.",
            _ => "No pull requests here.",
        };
        caption(ui, line);
    }
    opened
}

/// One pull request: repository, title, checks and age. The whole row opens it.
fn row(ui: &mut Ui, pr: &PullRequest, now: OffsetDateTime) -> Option<String> {
    let has_note = !pr.note.is_empty();
    let height = if has_note { ROW_HEIGHT + NOTE_HEIGHT } else { ROW_HEIGHT };
    // Only a row with a page takes clicks and keyboard focus.
    let opens = pr.has_page();
    let sense = if opens { Sense::click() } else { Sense::hover() };
    // The id follows the pull request, not its place in the list, so focus stays on it
    // when a new status reorders the rows.
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), height));
    // Scoped to this board's list, so two boards showing one pull request stay apart.
    let response = ui.interact(rect, ui.id().with(("upkeep-pr", &pr.repository, pr.number)), sense);
    // A row out of view only takes its space: a long list costs nothing per frame.
    if !ui.is_rect_visible(rect) {
        return None;
    }
    // The page and the texts are made only when the row is pointed at or clicked.
    let url = (opens && (response.hovered() || response.clicked()))
        .then(|| pr.github_url())
        .flatten();
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(8),
            Stroke::new(1.5, theme::FG()),
            StrokeKind::Inside,
        );
    }
    let response = if url.is_some() {
        response.on_hover_cursor(CursorIcon::PointingHand).on_hover_ui(|ui| {
            ui.label(format!("Open {}#{} in a browser panel", pr.repository, pr.number));
        })
    } else {
        response
    };
    response.widget_info(|| {
        let name = format!("{}#{} {}", pr.repository, pr.number, pr.title);
        egui::WidgetInfo::labeled(egui::WidgetType::Button, opens, name)
    });
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            theme::blend(theme::PANEL_BG(), theme::FG(), 0.05),
        );
    }
    row_body(ui, rect.shrink2(Vec2::new(10.0, 0.0)), pr, now, has_note);
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE(), 180)),
    );
    response.clicked().then_some(url).flatten()
}

/// The repository, number, title, checks, age and note of one row, inside `inner`.
fn row_body(ui: &mut Ui, inner: Rect, pr: &PullRequest, now: OffsetDateTime, has_note: bool) {
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
