//! Image destination review before a new cloud starts.
use super::state::State;
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::{github::Prompt, publishing::Selection};

pub(in crate::app::cloud_panel::production) fn section(ui: &mut Ui, state: &mut State) -> Option<String> {
    let mut setup = None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Publish images").strong());
        let side = ui.spacing().interact_size.y;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
        if state.busy() && state.prompt.is_none() {
            egui::Spinner::new().paint_at(ui, rect);
            response.on_hover_text("Preparing image destination");
        }
        if let Some(info) = &state.info {
            let label = match info.selection.as_ref() {
                None => "Choose a destination",
                Some(Selection::Recipe { .. }) => "From profile",
                Some(Selection::Registry { .. }) => "Registry",
                Some(Selection::Personal { .. } | Selection::Automatic { .. }) => "Automatic",
            };
            ui.label(RichText::new(label).color(theme::FG_SOFT()));
        }
        if ui.add_enabled(!state.busy(), egui::Button::new("Change")).clicked() {
            state.changing = !state.changing;
        }
    });
    let target = state.selected().and_then(Selection::repository).map(str::to_owned);
    if let Some(target) = &target {
        ui.add(egui::Label::new(RichText::new(target).monospace().size(12.0)).wrap());
    }
    if let Some(info) = &state.info {
        match info.selection.as_ref() {
            Some(Selection::Personal { account, .. }) => {
                ui.label(
                    RichText::new(format!("Publishing account: {}", account.login))
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
                if !info.pull_saved {
                    ui.label("Add separate read-only access so the worker can pull this private image.");
                    if ui.button("Set up worker pull access").clicked() {
                        setup.clone_from(&target);
                    }
                }
            }
            Some(Selection::Automatic { login, .. }) => {
                let label = login.as_ref().map_or_else(
                    || "Sign in to choose your destination".to_owned(),
                    |login| format!("Confirm publishing account: {login}"),
                );
                if ui.add_enabled(!state.busy(), egui::Button::new(label)).clicked() {
                    state.authenticate(false, ui.ctx());
                }
            }
            Some(Selection::Registry { .. }) => {
                if let Some(choice) = info
                    .choices
                    .iter()
                    .find(|choice| Some(&choice.selection) == info.selection.as_ref())
                {
                    ui.label(
                        RichText::new(format!("Publishing account: {}", choice.username))
                            .size(12.0)
                            .color(theme::FG_SOFT()),
                    );
                }
            }
            _ => {}
        }
    }
    if let Some(chosen) = choices(ui, state, target.as_deref()) {
        setup = Some(chosen);
    }
    prompt(ui, state);
    setup
}

pub(super) fn feedback(ui: &mut Ui, state: &mut State) {
    if let Some(error) = &state.open_error {
        ui.label(RichText::new(error).color(theme::PALETTE_RED()).size(12.0));
    }
    if let Some(notice) = &state.notice {
        ui.label(
            RichText::new(format!("Stock watch stopped. {notice}"))
                .color(theme::FG_SOFT())
                .size(12.0),
        );
    }
    if let Some(error) = &state.error {
        ui.label(RichText::new(error).color(theme::PALETTE_RED()).size(12.0));
        if ui
            .add_enabled(!state.busy(), egui::Button::new("Reload image destinations"))
            .clicked()
        {
            state.reload(ui.ctx());
        }
    }
}

fn choices(ui: &mut Ui, state: &mut State, target: Option<&str>) -> Option<String> {
    if !state.changing {
        return None;
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(!state.busy(), |ui| {
        let mut setup = None;
        if state.info.as_ref().is_some_and(|info| info.shown_override.is_some())
            && ui.button("Use profile default").clicked()
        {
            state.save(None, ui.ctx());
        }
        if ui.add_enabled(state.info.is_some(), egui::Button::new("Use another publishing account")).on_disabled_hover_text("Reload image destinations before choosing a publishing account").clicked() {
            state.authenticate(true, ui.ctx());
        }
        ui.label(RichText::new("Signing in changes the publishing account on this computer. Other clouds keep their destinations and may need their original account again.").size(12.0).color(theme::FG_DIM()));
        let mut chosen = None;
        if let Some(info) = &state.info {
            for choice in &info.choices {
                let label = format!("{} · {}", choice.selection.repository().unwrap_or_default(), choice.username);
                if ui.button(label).clicked() { chosen = Some(choice.selection.clone()); }
            }
        }
        if let Some(chosen) = chosen { state.save(Some(chosen), ui.ctx()); }
        if ui.button("Add a registry destination").clicked() {
            setup = Some(target.unwrap_or_default().to_owned());
        }
        ui.label(RichText::new("This choice stays on this computer for this repository and profile.").size(12.0).color(theme::FG_DIM()));
        setup
    }).inner
}

fn prompt(ui: &mut Ui, state: &mut State) {
    if let Some(Prompt::Publish {
        user_code,
        verification_uri,
        ..
    }) = &state.prompt
    {
        let (code, uri) = (user_code.clone(), verification_uri.clone());
        ui.label("Authorize image publishing");
        ui.horizontal(|ui| {
            ui.label(RichText::new(&code).monospace().strong());
            if ui.button("Copy code").clicked() {
                ui.ctx().copy_text(code.clone());
            }
            if ui
                .add_enabled(!state.opening_url(), egui::Button::new("Open GitHub"))
                .clicked()
            {
                state.open_sign_in_url(uri, ui.ctx());
            }
            if ui.button("Cancel sign-in").clicked() {
                state.skip_sign_in();
            }
        });
    }
}
