//! The provider, agent and workspace cards of the Cloud settings page.
use super::dashboard::{self, Key, Readiness, Tone, caption, field, header, label, saved_key, secret, surface};
use crate::{app::util::chrome_button, theme};
use egui::{Align, Layout, RichText, Ui, vec2};
use horizon_core::cloud_runtime::setup::{Agent, Authentication, Draft};

/// Which saved keys are being replaced; a saved key is otherwise shown masked.
#[derive(Default)]
pub(super) struct Edits {
    runpod: bool,
    hetzner: bool,
    /// Codex, then Claude.
    agents: [bool; 2],
}

pub(super) fn providers(ui: &mut Ui, draft: &mut Draft, edits: &mut Edits) {
    runpod(ui, draft, edits);
    hetzner(ui, draft, edits);
}

fn runpod(ui: &mut Ui, draft: &mut Draft, edits: &mut Edits) {
    let key = dashboard::runpod_key(draft);
    let optional = draft.hetzner.enabled;
    surface(ui, |ui| {
        let status = match key {
            Key::Saved => (Tone::Ready, "Key saved"),
            Key::Unsaved => (Tone::Attention, "Unsaved key"),
            Key::Missing if optional => (Tone::Idle, "Optional"),
            Key::Missing => (Tone::Attention, "Needs a key"),
        };
        header(ui, "RunPod", "CPU and GPU workspaces", Some(status));
        let saved = draft.saved_credentials.contains(&draft.settings.runpod_key_file);
        if saved && key == Key::Saved && !edits.runpod {
            edits.runpod = saved_key(ui);
        } else {
            secret(ui, "compute-key", &mut draft.runpod_key, saved, "Paste RunPod API key");
            if saved && keep_saved(ui) {
                draft.runpod_key.clear();
                edits.runpod = false;
            }
        }
        caption(
            ui,
            "Stored privately on this computer. Used only when you deploy or manage a worker. \
             Optional when Hetzner is on: clouds then run on Hetzner only.",
        );
    });
}

fn hetzner(ui: &mut Ui, draft: &mut Draft, edits: &mut Edits) {
    let key = dashboard::hetzner_key(draft);
    surface(ui, |ui| {
        let status = match key {
            Key::Saved => (Tone::Ready, "Key saved"),
            Key::Unsaved => (Tone::Attention, "Unsaved key"),
            Key::Missing if draft.hetzner.enabled => (Tone::Attention, "Needs a key"),
            Key::Missing => (Tone::Idle, "Off"),
        };
        header(ui, "Hetzner Cloud", "CPU workspaces · optional", Some(status));
        ui.checkbox(
            &mut draft.hetzner.enabled,
            RichText::new("Use Hetzner Cloud for CPU clouds")
                .size(13.0)
                .color(theme::FG()),
        );
        if !draft.hetzner.enabled {
            return;
        }
        let saved = draft
            .settings
            .hetzner
            .as_ref()
            .is_some_and(|provider| draft.saved_credentials.contains(&provider.token_file));
        if saved && key == Key::Saved && !edits.hetzner {
            edits.hetzner = saved_key(ui);
        } else {
            secret(
                ui,
                "hetzner-token",
                &mut draft.hetzner.token,
                saved,
                "Paste Hetzner API token",
            );
            if saved && keep_saved(ui) {
                draft.hetzner.token.clear();
                edits.hetzner = false;
            }
        }
        caption(
            ui,
            "A read and write token for a project used only by Horizon. It covers the whole project, \
             so it stays on this computer and never reaches a worker.",
        );
        ui.collapsing(
            RichText::new("Placement preferences")
                .size(12.0)
                .color(theme::FG_SOFT()),
            |ui| {
                field(
                    ui,
                    "Server types, in order of preference",
                    &mut draft.hetzner.server_types,
                    "cx43, cx33",
                );
                field(
                    ui,
                    "Locations, in order of preference",
                    &mut draft.hetzner.locations,
                    "hel1, nbg1",
                );
                caption(
                    ui,
                    "New cloud compares all x86 server types in these locations. The preferred types apply only when no worker is chosen.",
                );
                caption(
                    ui,
                    "Prices are in euros, net of VAT. A stopped Hetzner cloud keeps only its workspace volume.",
                );
            },
        );
    });
}

/// Drops what was typed over a saved key. Returns whether it was clicked.
fn keep_saved(ui: &mut Ui) -> bool {
    let mut clicked = false;
    // A centered row placed straight in the card would fill all the height left in the modal.
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        clicked = ui
            .add(chrome_button("Keep saved key").min_size(vec2(120.0, 28.0)))
            .clicked();
    });
    clicked
}

pub(super) fn agents(
    ui: &mut Ui,
    draft: &mut Draft,
    edits: &mut Edits,
    fixed_agents: bool,
    chatgpt: &mut super::chatgpt::Card,
) {
    let status = dashboard::agents_status(draft, fixed_agents);
    surface(ui, |ui| {
        header(
            ui,
            "Coding agents",
            "Choose one or both. New panels share the cloud checkout.",
            Some(status),
        );
        let selected_agents = draft.selected_agents().to_vec();
        let root = draft.root().to_owned();
        let [codex, claude] = &mut edits.agents;
        for (agent, name, mode, value, saved, replacing) in [
            (
                Agent::Codex,
                "Codex",
                &mut draft.openai_auth,
                &mut draft.openai_key,
                draft
                    .settings
                    .openai_api_key_file
                    .as_ref()
                    .is_some_and(|path| draft.saved_credentials.contains(path)),
                codex,
            ),
            (
                Agent::Claude,
                "Claude",
                &mut draft.anthropic_auth,
                &mut draft.anthropic_key,
                draft
                    .settings
                    .anthropic_api_key_file
                    .as_ref()
                    .is_some_and(|path| draft.saved_credentials.contains(path)),
                claude,
            ),
        ] {
            ui.push_id(name, |ui| {
                let mut selected = selected_agents.contains(&agent);
                let changed = ui
                    .add_enabled(
                        !fixed_agents,
                        egui::Checkbox::new(&mut selected, RichText::new(name).size(13.0).color(theme::FG())),
                    )
                    .changed();
                if changed {
                    draft.settings.default_agents.retain(|item| *item != agent);
                    if selected {
                        draft.settings.default_agents.push(agent);
                    }
                }
                if agent == Agent::Codex {
                    // The ChatGPT card ticks in every mode and while unselected, so a
                    // flow under way always lands and its busy state always ends.
                    super::chatgpt::Card::tick(chatgpt, ui, &mut draft.chatgpt);
                }
                if !selected {
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(mode, Authentication::ApiKey, "API key");
                    // Codex signs in through a ChatGPT account; the other agents
                    // keep the worker-terminal login.
                    if agent == Agent::Codex {
                        ui.selectable_value(mode, Authentication::ChatGpt, "ChatGPT plan");
                    } else {
                        ui.selectable_value(mode, Authentication::Subscription, "Subscription login");
                    }
                });
                if *mode == Authentication::ChatGpt {
                    if agent == Agent::Codex {
                        super::chatgpt::row(ui, &mut draft.chatgpt, &root, chatgpt);
                    } else {
                        caption(ui, "Claude does not support ChatGPT sign-in. Choose another option.");
                    }
                    return;
                }
                if *mode == Authentication::ApiKey {
                    if saved && value.is_empty() && !*replacing {
                        *replacing = saved_key(ui);
                    } else {
                        secret(ui, "agent-key", value, saved, "Paste API key");
                        if saved && keep_saved(ui) {
                            value.clear();
                            *replacing = false;
                        }
                    }
                } else {
                    caption(
                        ui,
                        "Sign in through the agent’s own terminal after the worker is ready. \
                         Your login stays on that worker.",
                    );
                }
            });
        }
    });
}

/// What these settings add up to, and the options few people need.
pub(super) fn workspace(ui: &mut Ui, draft: &mut Draft, ssh_ready: Option<bool>, readiness: &Readiness) {
    surface(ui, |ui| {
        let status = match readiness.tone {
            Tone::Ready => (Tone::Ready, "Complete"),
            tone => (tone, "Incomplete"),
        };
        header(ui, "Your workspace", "Defaults for every new cloud", Some(status));
        let compute = match (
            dashboard::runpod_key(draft) != Key::Missing,
            dashboard::hetzner_key(draft) != Key::Missing,
        ) {
            (true, true) => "RunPod and Hetzner",
            (true, false) => "RunPod",
            (false, true) => "Hetzner",
            (false, false) => "None yet",
        };
        let agents = draft
            .selected_agents()
            .iter()
            .map(|agent| dashboard::agent_name(*agent))
            .collect::<Vec<_>>()
            .join(" + ");
        let ssh = match ssh_ready {
            Some(true) => "Ready",
            Some(false) if draft.settings.ssh_identity_file.exists() => "Needs repair",
            Some(false) => "Made when you save",
            None => "Checking…",
        };
        let rows = [
            ("Compute", compute.to_owned()),
            (
                "Agents",
                if agents.is_empty() {
                    "None chosen".to_owned()
                } else {
                    agents
                },
            ),
            ("SSH identity", ssh.to_owned()),
        ];
        for (index, (name, value)) in rows.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                label(ui, name);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(value).size(13.0).color(theme::FG()));
                });
            });
        }
        ui.separator();
        ui.collapsing(
            RichText::new("Advanced options").size(12.0).color(theme::FG_SOFT()),
            |ui| {
                caption(
                    ui,
                    "A dedicated SSH identity is created automatically. Existing registry, Git and \
                     remote-browser bindings are preserved.",
                );
                let mut endpoint = draft.settings.docker_host.clone().unwrap_or_default();
                // Only an edit rewrites the saved value; opening the section changes nothing.
                if field(ui, "Docker endpoint (blank uses the local default)", &mut endpoint, "").changed() {
                    draft.settings.docker_host = (!endpoint.trim().is_empty()).then(|| endpoint.trim().to_owned());
                }
            },
        );
        caption(ui, "Choose a size and data center for each cloud in New cloud.");
    });
}
