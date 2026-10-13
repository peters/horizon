//! The repository picker of New Upkeep: one tab per owner, a filter, and a checkbox per
//! repository with its open pull requests from the chosen bots.
use egui::{
    Align, CornerRadius, CursorIcon, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextEdit, Ui, UiBuilder, Vec2,
};
use horizon_core::upkeep::CatalogRepository;

use super::dialog::{link, segment};
use super::form::Form;
use crate::theme;

const ROW_HEIGHT: f32 = 32.0;
const LIST_HEIGHT: f32 = 248.0;

pub(super) fn show(ui: &mut Ui, form: &mut Form) {
    if form.finding.is_some() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT()));
            ui.label(
                RichText::new("Finding your repositories and their open pull requests...")
                    .size(12.5)
                    .color(theme::FG_SOFT()),
            );
        });
        if form.catalog.repositories.is_empty() {
            return;
        }
    }
    let problem = form
        .find_error
        .as_ref()
        .map(|error| format!("Could not search GitHub: {error}"))
        .or_else(|| {
            form.catalog
                .search_error
                .as_ref()
                .map(|error| format!("Could not count the open pull requests: {error}"))
        });
    if let Some(problem) = problem
        && form.finding.is_none()
    {
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(problem).size(12.5).color(theme::PALETTE_YELLOW())).wrap());
            if link(ui, "Try again").clicked() {
                form.retry();
            }
        });
    }
    let owners: Vec<String> = form.catalog.owners().into_iter().map(str::to_owned).collect();
    if owners.is_empty() {
        if form.finding.is_none() {
            ui.label(
                RichText::new("No repositories. Connect GitHub in Cloud settings and install the app on them.")
                    .size(12.5)
                    .color(theme::FG_SOFT()),
            );
        }
        return;
    }
    // The tab is kept by name: new counts can change the order of the owners.
    let owner = form
        .owner
        .clone()
        .filter(|owner| owners.contains(owner))
        .unwrap_or_else(|| owners[0].clone());
    tabs(ui, form, &owners, &owner);
    let owner = form
        .owner
        .clone()
        .filter(|owner| owners.contains(owner))
        .unwrap_or(owner);
    list(ui, form, &owner);
}

/// One tab per owner, with how many of its repositories are chosen, and the filter.
fn tabs(ui: &mut Ui, form: &mut Form, owners: &[String], current: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [ui.available_width().min(320.0), 30.0],
            TextEdit::singleline(&mut form.search)
                .id(Id::new("upkeep-repository-filter"))
                .font(egui::FontId::proportional(13.0))
                .margin(Vec2::new(10.0, 7.0))
                .hint_text("Filter repositories"),
        );
    });
    // One pass over the chosen names, not one over the catalog for each owner.
    let mut chosen_by_owner = std::collections::HashMap::<&str, usize>::new();
    for name in &form.selected {
        let owner = name.split_once('/').map_or(name.as_str(), |(owner, _)| owner);
        *chosen_by_owner.entry(owner).or_default() += 1;
    }
    let chosen_by_owner: Vec<usize> = owners
        .iter()
        .map(|owner| chosen_by_owner.get(owner.as_str()).copied().unwrap_or(0))
        .collect();
    // Many or long owner names wrap to more rows rather than leave the dialog.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for (owner, chosen) in owners.iter().zip(chosen_by_owner) {
            let label = if chosen == 0 {
                owner.clone()
            } else {
                format!("{owner}  {chosen}")
            };
            if segment(ui, owner == current, &label).clicked() {
                form.owner = Some(owner.clone());
            }
        }
    });
}

/// The repositories of `owner` that the filter shows, with Select all and Clear.
fn list(ui: &mut Ui, form: &mut Form, owner: &str) {
    let needle = form.search.trim().to_lowercase();
    // Without counts the filter would hide every repository.
    let only_open = form.only_open && form.catalog.search_error.is_none();
    let shown: Vec<&CatalogRepository> = form
        .catalog
        .repositories
        .iter()
        .filter(|repository| repository.owner() == owner)
        // A chosen repository stays in view, so it can be cleared after its count fell to 0.
        .filter(|repository| !only_open || repository.open > 0 || form.selected.contains(&repository.name))
        .filter(|repository| needle.is_empty() || repository.name.contains(&needle))
        .collect();
    let mut select = None;
    ui.horizontal(|ui| {
        checkbox(ui, &mut form.only_open, "Only with open pull requests");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if link(ui, "Clear").clicked() {
                select = Some(false);
            }
            if link(ui, "Select all shown").clicked() {
                select = Some(true);
            }
        });
    });
    let mut toggled = Vec::new();
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(4, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if shown.is_empty() {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(if only_open && form.search.trim().is_empty() {
                        "No open pull requests from these bots. Clear Only with open pull requests to see every repository."
                    } else {
                        "No repositories match."
                    })
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
                ui.add_space(10.0);
                return;
            }
            // Only the rows in view are laid out: an owner can have thousands.
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical()
                .id_salt(("upkeep-repositories", owner))
                .auto_shrink([false, true])
                .max_height(LIST_HEIGHT)
                .show_rows(ui, ROW_HEIGHT, shown.len(), |ui, range| {
                    for repository in &shown[range] {
                        if row(ui, repository, form.selected.contains(&repository.name)) {
                            toggled.push(repository.name.clone());
                        }
                    }
                });
        });
    let names: Vec<String> = shown.iter().map(|repository| repository.name.clone()).collect();
    match select {
        Some(true) => form.selected.extend(names),
        Some(false) => names.iter().for_each(|name| {
            form.selected.remove(name);
        }),
        None => {}
    }
    for name in toggled {
        if !form.selected.remove(&name) {
            form.selected.insert(name);
        }
    }
}

/// One repository; the whole row toggles it. Returns true when it was clicked.
fn row(ui: &mut Ui, repository: &CatalogRepository, chosen: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, chosen, &repository.name));
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(6),
            theme::blend(theme::PANEL_BG(), theme::FG(), 0.05),
        );
    }
    let square = Rect::from_center_size(Pos2::new(rect.min.x + 18.0, rect.center().y), Vec2::splat(16.0));
    paint_box(ui, square, chosen, response.hovered());
    let inner = Rect::from_min_max(
        Pos2::new(square.max.x + 10.0, rect.min.y),
        Pos2::new(rect.max.x - 10.0, rect.max.y),
    );
    // The count is laid out first, so a long name is cut instead of the count.
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            if repository.open > 0 {
                crate::upkeep_widget::pill(ui, &format!("{} open", repository.open), theme::PALETTE_GREEN());
            } else {
                ui.add(
                    egui::Label::new(RichText::new("no open pull requests").size(11.0).color(theme::FG_DIM()))
                        .selectable(false),
                );
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(repository.short_name())
                            .size(12.5)
                            .monospace()
                            .color(theme::FG()),
                    )
                    .truncate()
                    .selectable(false),
                );
            });
        },
    );
    response.clicked()
}

fn paint_box(ui: &Ui, square: Rect, checked: bool, hovered: bool) {
    let painter = ui.painter();
    if checked {
        painter.rect_filled(square, 4, theme::ACCENT());
        let points = vec![
            Pos2::new(square.left() + 3.5, square.center().y),
            Pos2::new(square.left() + 6.5, square.bottom() - 4.0),
            Pos2::new(square.right() - 3.5, square.top() + 4.0),
        ];
        painter.line(points, Stroke::new(2.0, theme::BG()));
    } else {
        let edge = if hovered { theme::FG_SOFT() } else { theme::FG_DIM() };
        painter.rect_stroke(square, 4, Stroke::new(1.5, edge), StrokeKind::Inside);
    }
}

fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) {
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(12.5), theme::FG());
    let size = Vec2::new(16.0 + 8.0 + text.size().x, 16.0_f32.max(text.size().y));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *checked = !*checked;
    }
    let square = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 8.0), Vec2::splat(16.0));
    paint_box(ui, square, *checked, response.hovered());
    ui.painter().galley(
        Pos2::new(square.right() + 8.0, rect.center().y - text.size().y / 2.0),
        text,
        theme::FG(),
    );
    let value = *checked;
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, value, label));
}
