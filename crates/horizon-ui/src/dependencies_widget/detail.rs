//! The selected repository: state, pull requests, Dependabot configuration and instructions.

use egui::{RichText, Sense, Stroke, Vec2, vec2};
use serde_json::Value;

use horizon_core::maintenance::portfolio::{
    PrStage, Repository, RepositoryStatus, clock, github_pull_request_url, strings, text,
};

use super::{tone, widgets};
use crate::theme;

pub(super) enum Request {
    Close,
    EditInstructions,
}

pub(super) fn show(ui: &mut egui::Ui, repo: &Repository<'_>, status: &Value, height: f32) -> Option<Request> {
    let mut request = None;
    widgets::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_height(height - 34.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        if heading(ui, repo) {
            request = Some(Request::Close);
        }
        ui.add_space(4.0);
        widgets::solid_scroll_area(ui)
            .id_salt(("repository-detail", repo.name))
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.set_width(ui.available_width() - 12.0);
                overview(ui, repo);
                ui.add_space(4.0);
                if ui
                    .add(widgets::accent_text_button("Edit repository instructions"))
                    .clicked()
                {
                    request = Some(Request::EditInstructions);
                }
                ui.add_space(14.0);
                pull_requests(ui, repo);
                ui.add_space(14.0);
                dependabot(ui, repo);
                ui.add_space(14.0);
                instructions(ui, repo, status);
            });
    });
    request
}

/// Owner, name and a close control. Returns true when closed.
fn heading(ui: &mut egui::Ui, repo: &Repository<'_>) -> bool {
    let (owner, name) = repo.owner_and_name();
    let mut closed = false;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if !owner.is_empty() {
                ui.label(RichText::new(format!("{owner} /")).size(12.5).color(theme::FG_SOFT()));
            }
            ui.add(egui::Label::new(RichText::new(name).size(19.0).strong().color(theme::FG())).truncate());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            closed = close_button(ui).clicked();
        });
    });
    closed
}

fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Close details"));
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, 8, theme::blend(theme::PANEL_BG_ALT(), theme::FG(), 0.06));
    }
    let color = if response.hovered() {
        theme::FG()
    } else {
        theme::FG_SOFT()
    };
    widgets::cross_mark(ui.painter(), rect.center(), 14.0, color);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Back to all repositories")
}

fn overview(ui: &mut egui::Ui, repo: &Repository<'_>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        widgets::pill(ui, repo.status.label(), tone::color(repo.status.tone()));
        let activity = text(repo.data, "last_activity", "");
        if !activity.is_empty() {
            ui.label(
                RichText::new(format!("Last activity {}", clock(activity)))
                    .size(12.0)
                    .color(theme::FG_SOFT()),
            );
        }
    });
    let reason = text(repo.data, "status_reason", "");
    if !reason.is_empty() {
        ui.label(RichText::new(reason).size(13.0).color(theme::FG_SOFT()));
    }
    if repo.status == RepositoryStatus::Disabled {
        ui.label(
            RichText::new("Maintenance is paused for this repository by policy.")
                .size(12.5)
                .color(theme::FG_SOFT()),
        );
    }
}

fn pull_requests(ui: &mut egui::Ui, repo: &Repository<'_>) {
    let note = match (repo.prs.len(), repo.open_prs()) {
        (0, _) => String::new(),
        (total, open) => format!("{total} · {open} open"),
    };
    widgets::caption(
        ui,
        "Pull requests",
        Some(&note).filter(|note| !note.is_empty()).map(String::as_str),
    );
    if repo.prs.is_empty() {
        ui.label(
            RichText::new("Dependabot has no open pull requests here.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
        return;
    }
    for pr in &repo.prs {
        pull_request(ui, pr);
    }
}

fn pull_request(ui: &mut egui::Ui, pr: &Value) {
    let stage = PrStage::of(pr);
    egui::Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.horizontal(|ui| {
                let number = pr.get("number").and_then(Value::as_u64).unwrap_or(0);
                ui.label(
                    RichText::new(format!("#{number}"))
                        .monospace()
                        .size(13.0)
                        .color(theme::FG_SOFT()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::pill(ui, text(pr, "status", "Queued"), tone::color(stage.tone()));
                });
            });
            ui.label(
                RichText::new(text(pr, "title", "Dependency update"))
                    .size(14.0)
                    .color(theme::FG()),
            );
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for tag in [text(pr, "ecosystem", ""), text(pr, "group", "")] {
                    if !tag.is_empty() {
                        widgets::tag(ui, tag);
                    }
                }
            });
            let detail = text(pr, "detail", "");
            if !detail.is_empty() {
                ui.label(RichText::new(detail).size(12.5).color(theme::FG_SOFT()));
            }
            checks(ui, pr);
            link(ui, pr);
        });
}

/// The latest check run on the PR head, as a mark and words.
fn checks(ui: &mut egui::Ui, pr: &Value) {
    let runs = pr.get("checks").and_then(Value::as_array);
    let Some(last) = runs.and_then(|runs| runs.last()) else {
        return;
    };
    let passed = last.get("passed").and_then(Value::as_bool) == Some(true);
    let head = text(last, "head", "");
    let runs = runs.map_or(0, Vec::len);
    let verdict = if passed { "Checks passed" } else { "Checks failed" };
    let on = if head.is_empty() {
        String::new()
    } else {
        format!(" on {head}")
    };
    let repeated = if runs > 1 {
        format!(" · {runs} runs")
    } else {
        String::new()
    };
    let words = format!("{verdict}{on}{repeated}");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (mark, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        let color = if passed {
            theme::PALETTE_GREEN()
        } else {
            theme::PALETTE_RED()
        };
        ui.painter().circle_filled(mark.center(), 8.0, theme::alpha(color, 36));
        if passed {
            widgets::check_mark(ui.painter(), mark.center(), 10.0, color);
        } else {
            widgets::cross_mark(ui.painter(), mark.center(), 10.0, color);
        }
        ui.label(RichText::new(words).size(12.5).color(theme::FG_SOFT()));
    });
}

fn link(ui: &mut egui::Ui, pr: &Value) {
    match github_pull_request_url(pr) {
        Some(url) => {
            let response = ui.add(
                egui::Button::new(RichText::new("Open in GitHub ↗").size(13.0).color(theme::ACCENT())).frame(false),
            );
            if response.on_hover_text(url).clicked()
                && let Err(error) = horizon_core::open_url(url)
            {
                tracing::warn!(%error, "Could not open maintenance pull request");
            }
        }
        None => {
            ui.label(
                RichText::new("GitHub link unavailable")
                    .size(12.5)
                    .color(theme::FG_DIM()),
            );
        }
    }
}

/// Each `updates` entry of `.github/dependabot.yml`, as the worker parsed it.
fn dependabot(ui: &mut egui::Ui, repo: &Repository<'_>) {
    widgets::caption(ui, "Dependabot configuration", None);
    let updates: Vec<_> = repo
        .data
        .get("updates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    if updates.is_empty() {
        ui.label(
            RichText::new("The worker reported no update entries.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
        return;
    }
    for (index, update) in updates.iter().enumerate() {
        if index > 0 {
            ui.add_space(2.0);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            widgets::tag(ui, text(update, "package-ecosystem", "unknown"));
            let interval = update
                .get("schedule")
                .map_or("", |schedule| text(schedule, "interval", ""));
            let place = text(update, "directory", "/");
            if !interval.is_empty() {
                ui.label(
                    RichText::new(format!("Checked {interval}"))
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
            }
            ui.label(RichText::new("in").size(12.5).color(theme::FG_DIM()));
            ui.label(RichText::new(place).monospace().size(12.5).color(theme::FG_SOFT()));
        });
        for (name, group) in update.get("groups").and_then(Value::as_object).into_iter().flatten() {
            let kinds = strings(group.get("update-types"));
            let scope = if kinds.is_empty() {
                "all updates".to_owned()
            } else {
                kinds.join(", ")
            };
            detail_line(ui, "Group", &format!("{name} · {scope}"));
        }
        for rule in update.get("ignore").and_then(Value::as_array).into_iter().flatten() {
            detail_line(ui, "Ignores", &ignore_rule(rule));
        }
    }
}

fn ignore_rule(rule: &Value) -> String {
    let name = text(rule, "dependency-name", "*");
    let kinds: Vec<_> = strings(rule.get("update-types"))
        .into_iter()
        .map(|kind| match kind {
            "version-update:semver-major" => "major",
            "version-update:semver-minor" => "minor",
            "version-update:semver-patch" => "patch",
            other => other,
        })
        .collect();
    match (name, kinds.is_empty()) {
        ("*", true) => "every dependency".to_owned(),
        ("*", false) => format!("{} versions", kinds.join(", ")),
        (name, true) => name.to_owned(),
        (name, false) => format!("{name} · {} versions", kinds.join(", ")),
    }
}

fn detail_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(4.0);
        let (bar, _) = ui.allocate_exact_size(vec2(2.0, 16.0), Sense::hover());
        ui.painter().rect_filled(bar, 1, theme::BORDER_STRONG());
        ui.label(RichText::new(label).size(12.5).color(theme::FG_DIM()));
        ui.label(RichText::new(value).size(12.5).color(theme::FG_SOFT()));
    });
}

fn instructions(ui: &mut egui::Ui, repo: &Repository<'_>, status: &Value) {
    widgets::caption(ui, "Instructions", None);
    ui.push_id(repo.name, |ui| {
        section(
            ui,
            "Repository prompt",
            text(repo.data, "prompt", "Uses the global instructions only."),
            false,
            true,
        );
        section(
            ui,
            "AGENTS.md",
            text(repo.data, "instructions", "The worker reported no instructions."),
            true,
            false,
        );
        section(
            ui,
            "Global instructions",
            text(
                status,
                "global_prompt",
                "Follow trusted repository instructions and preserve Dependabot configuration.",
            ),
            false,
            false,
        );
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: &str, code: bool, open: bool) {
    egui::CollapsingHeader::new(RichText::new(title).size(13.5).color(theme::FG()))
        .default_open(open)
        .show(ui, |ui| {
            widgets::well().show(ui, |ui| {
                ui.set_width(ui.available_width());
                let text = RichText::new(body).size(12.5).color(theme::FG_SOFT());
                ui.label(if code { text.monospace() } else { text });
            });
        });
}
