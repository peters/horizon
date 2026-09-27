use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::setup::{Agent, Authentication, Draft};

pub(super) fn render(ui: &mut Ui, draft: &mut Draft) {
    render_profile(ui, draft, false);
}

pub(super) fn render_profile(ui: &mut Ui, draft: &mut Draft, fixed_agents: bool) {
    // Settings saved for Hetzner alone hold no RunPod key to keep.
    let saved_compute = draft.has_saved_settings() && draft.settings.runpod_configured();
    ui.label(RichText::new("Compute account").size(16.0).strong());
    ui.label("RunPod API key");
    secret(ui, "compute-key", &mut draft.runpod_key, saved_compute);
    ui.small(
        "Stored privately on this computer. Used only when you deploy or manage a worker. \
         Optional when Hetzner is on: clouds then run on Hetzner only.",
    );
    ui.add_space(12.0);
    render_hetzner(ui, draft);
    ui.add_space(18.0);
    ui.label(RichText::new("Coding agents").size(16.0).strong());
    ui.label(RichText::new("Choose one or both. Each agent gets its own worktree.").color(theme::FG_SOFT()));
    let selected_agents = draft.selected_agents().to_vec();
    for (agent, label, mode, value, saved) in [
        (
            Agent::Codex,
            "Codex",
            &mut draft.openai_auth,
            &mut draft.openai_key,
            draft.settings.openai_api_key_file.is_some(),
        ),
        (
            Agent::Claude,
            "Claude",
            &mut draft.anthropic_auth,
            &mut draft.anthropic_key,
            draft.settings.anthropic_api_key_file.is_some(),
        ),
    ] {
        ui.push_id(label, |ui| {
            ui.add_space(10.0);
            let mut selected = selected_agents.contains(&agent);
            if ui.add_enabled(!fixed_agents, egui::Checkbox::new(&mut selected, RichText::new(label).strong())).changed() {
                draft.settings.default_agents.retain(|item| *item != agent);
                if selected { draft.settings.default_agents.push(agent); }
            }
            if selected {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(mode, Authentication::ApiKey, "API key");
                    ui.selectable_value(mode, Authentication::Subscription, "Subscription login");
                });
                if *mode == Authentication::ApiKey {
                    secret(ui, "agent-key", value, saved);
                } else {
                    ui.small("Sign in through the agent’s own terminal after the worker is ready. Your login stays on that worker.");
                }
            }
        });
    }
    ui.add_space(12.0);
    ui.collapsing("Advanced", |ui| {
        ui.small("A dedicated SSH identity is created automatically. Existing registry, Git and remote-browser bindings are preserved.");
        ui.label("Docker endpoint (blank uses the local default)");
        let mut endpoint = draft.settings.docker_host.clone().unwrap_or_default();
        if ui.text_edit_singleline(&mut endpoint).changed() {
            draft.settings.docker_host = (!endpoint.trim().is_empty()).then(|| endpoint.trim().to_owned());
        }
    });
}

/// Hetzner as an optional second provider for CPU clouds.
fn render_hetzner(ui: &mut Ui, draft: &mut Draft) {
    let saved = draft.settings.hetzner.is_some();
    let hetzner = &mut draft.hetzner;
    ui.checkbox(&mut hetzner.enabled, RichText::new("Hetzner Cloud (CPU only)").strong());
    if !hetzner.enabled {
        return;
    }
    ui.label("Hetzner Cloud API token");
    secret(ui, "hetzner-token", &mut hetzner.token, saved);
    ui.small("A read and write token for a project used only by Horizon. It covers the whole project, so it stays on this computer and never reaches a worker.");
    ui.label("Server types, in order of preference");
    list(ui, "hetzner-server-types", &mut hetzner.server_types);
    ui.label("Locations, in order of preference");
    list(ui, "hetzner-locations", &mut hetzner.locations);
    ui.small("Prices are in euros, net of VAT. A stopped Hetzner cloud keeps only its workspace volume.");
}

/// A comma-separated list, as wide as the key fields.
fn list(ui: &mut Ui, id: &str, value: &mut String) {
    ui.add_sized(
        [ui.available_width(), 38.0],
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .margin(egui::vec2(12.0, 10.0)),
    );
}

fn secret(ui: &mut Ui, id: &str, value: &mut String, saved: bool) {
    ui.add_sized(
        [ui.available_width(), 38.0],
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .password(true)
            .margin(egui::vec2(12.0, 10.0))
            .hint_text(if saved {
                "Enter a key, or leave blank to keep the saved binding"
            } else {
                "Paste API key"
            }),
    );
}
