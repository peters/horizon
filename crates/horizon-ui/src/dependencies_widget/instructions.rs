//! Global and repository instructions, edited locally and saved to the worker over SSH.

use egui::{FontId, RichText, Stroke, Vec2};
use serde_json::{Map, Value};

use horizon_core::maintenance::portfolio::text;

use super::{
    portfolio::{Action, State},
    widgets,
};
use crate::theme;

pub(super) struct Draft {
    pub(super) global: String,
    pub(super) repositories: Map<String, Value>,
    pub(super) selected: Option<String>,
}

pub(super) fn open(state: &mut State, status: &Value, selected: Option<String>) {
    if state.save_pending {
        return;
    }
    let draft = state.editor.get_or_insert_with(|| Draft {
        global: text(status, "global_prompt", "").to_owned(),
        repositories: status
            .get("repos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|repo| {
                Some((
                    repo.get("repository")?.as_str()?.to_owned(),
                    Value::String(text(repo, "prompt", "").to_owned()),
                ))
            })
            .collect(),
        selected: None,
    });
    draft.selected = selected;
    state.save_feedback = None;
}

pub(super) fn sheet(ctx: &egui::Context, state: &mut State, status: &Value) -> Option<Action> {
    let draft = state.editor.as_mut()?;
    let mut close = false;
    let mut action = None;
    let id = egui::Id::new("maintenance-instructions");
    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
        .frame(widgets::dialog_frame())
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 64.0).clamp(240.0, 760.0));
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
            heading(ui, status);
            ui.add_space(12.0);
            ui.add_enabled_ui(!state.save_pending, |ui| fields(ui, draft, status));
            ui.add_space(8.0);
            if let Some(Err(error)) = &state.save_feedback {
                error_frame(ui, error);
            } else {
                ui.label(
                    RichText::new(
                        "Saved to the worker over SSH. The worker applies changes when it starts its next task.",
                    )
                    .size(12.5)
                    .color(theme::FG_SOFT()),
                );
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!state.save_pending, widgets::primary_button("Save to worker"))
                    .clicked()
                {
                    state.save_pending = true;
                    action = Some(Action::SaveInstructions {
                        global: draft.global.clone(),
                        repositories: draft.repositories.clone(),
                    });
                }
                if ui
                    .add_enabled(!state.save_pending, widgets::secondary_button("Cancel"))
                    .clicked()
                {
                    close = true;
                }
                if state.save_pending {
                    ui.label(
                        RichText::new("Saving to the worker…")
                            .size(13.0)
                            .color(theme::FG_SOFT()),
                    );
                    ui.spinner();
                }
            });
        });
    if (close || response.should_close()) && !state.save_pending {
        state.editor = None;
    }
    action
}

fn heading(ui: &mut egui::Ui, status: &Value) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Instructions").size(24.0).strong().color(theme::FG()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            revision(ui, status);
        });
    });
    ui.label(
        RichText::new("Global instructions apply to every repository. Repository instructions add to them.")
            .size(13.5)
            .color(theme::FG_SOFT()),
    );
}

/// Saved versus applied revision, so a pending change is never mistaken for an active one.
fn revision(ui: &mut egui::Ui, status: &Value) {
    let Some(configured) = status.get("configured_revision").and_then(Value::as_u64) else {
        return;
    };
    let applied = status.get("applied_revision").and_then(Value::as_u64).unwrap_or(0);
    let (label, color) = if applied >= configured {
        (format!("Revision {configured} · applied"), theme::PALETTE_GREEN())
    } else {
        (
            format!("Revision {configured} saved · {applied} applied"),
            theme::PALETTE_YELLOW(),
        )
    };
    widgets::pill(ui, &label, color);
}

fn fields(ui: &mut egui::Ui, draft: &mut Draft, status: &Value) {
    let total = draft.repositories.len();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if widgets::segment(
            ui,
            draft.selected.is_none(),
            &format!("Global · all {total} repositories"),
        )
        .clicked()
        {
            draft.selected = None;
        }
        if ui
            .add_enabled_ui(total > 0, |ui| {
                widgets::segment(ui, draft.selected.is_some(), "One repository")
            })
            .inner
            .clicked()
            && draft.selected.is_none()
        {
            draft.selected = draft.repositories.keys().next().cloned();
        }
    });
    ui.add_space(6.0);
    let Some(selected) = &mut draft.selected else {
        ui.add_sized(
            Vec2::new(ui.available_width(), 240.0),
            editor(&mut draft.global).hint_text("Instructions for every repository"),
        );
        return;
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("Repository").size(13.0).color(theme::FG_SOFT()));
        widgets::field(ui, |ui| {
            egui::ComboBox::from_id_salt("instruction-repository")
                .selected_text(RichText::new(selected.as_str()).size(13.5).color(theme::FG()))
                .width(380.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for repository in draft.repositories.keys() {
                        ui.selectable_value(selected, repository.clone(), repository);
                    }
                });
        });
    });
    let mut prompt = draft
        .repositories
        .get(selected.as_str())
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if ui
        .add_sized(
            Vec2::new(ui.available_width(), 200.0),
            editor(&mut prompt).hint_text("Additional instructions for this repository"),
        )
        .changed()
    {
        draft.repositories.insert(selected.clone(), Value::String(prompt));
    }
    if let Some(repo) = status
        .get("repos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|repo| text(repo, "repository", "") == selected.as_str())
    {
        egui::CollapsingHeader::new(
            RichText::new("Required checks from AGENTS.md")
                .size(13.5)
                .color(theme::FG()),
        )
        .show(ui, |ui| {
            widgets::well().show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    ui.label(
                        RichText::new(text(repo, "instructions", "No requirements reported."))
                            .monospace()
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                    );
                });
            });
        });
    }
}

fn editor(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text)
        .font(FontId::monospace(13.5))
        .text_color(theme::FG())
        .frame(
            egui::Frame::new()
                .fill(theme::BG())
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                .corner_radius(10)
                .inner_margin(16),
        )
}

fn error_frame(ui: &mut egui::Ui, error: &str) {
    egui::Frame::new()
        .fill(theme::alpha(theme::PALETTE_RED(), 24))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_RED(), 90)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Not saved")
                    .size(13.5)
                    .strong()
                    .color(theme::PALETTE_RED()),
            );
            ui.label(
                RichText::new(format!(
                    "{error}. Your draft is kept; try again when the worker is reachable."
                ))
                .size(12.5)
                .color(theme::FG_SOFT()),
            );
        });
}
