//! The Cloud settings page: a readiness banner above cards for each thing a cloud needs.
//! Every status shown is something Horizon knows from this machine's settings or from a saved
//! validation result; provider accounts are checked when a cloud starts.
use super::{State, fields, registry};
use crate::{app::util::chrome_button, theme};
use egui::{Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use horizon_core::cloud_runtime::{
    registry::{Action, Validation},
    setup::{Agent, Authentication, Draft},
};
use std::collections::BTreeMap;

const GAP: f32 = 16.0;
const STACKED_BELOW: f32 = 760.0;
const FIELD_HEIGHT: f32 = 36.0;

/// Repositories whose pull access has a saved validation, by name.
pub(super) type Verified = BTreeMap<String, Validation>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tone {
    Ready,
    Attention,
    Idle,
}

impl Tone {
    fn color(self) -> Color32 {
        match self {
            Self::Ready => theme::PALETTE_GREEN(),
            Self::Attention => theme::PALETTE_YELLOW(),
            Self::Idle => theme::FG_DIM(),
        }
    }
}

/// Whether a provider's key is on this computer, typed and waiting for Save, or absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Key {
    Saved,
    Unsaved,
    Missing,
}

pub(super) fn runpod_key(draft: &Draft) -> Key {
    if !draft.runpod_key.is_empty() {
        Key::Unsaved
    } else if draft.saved_credentials.contains(&draft.settings.runpod_key_file) {
        Key::Saved
    } else {
        Key::Missing
    }
}

/// `Missing` also when Hetzner is switched off: there is no key to speak of.
pub(super) fn hetzner_key(draft: &Draft) -> Key {
    if !draft.hetzner.enabled {
        Key::Missing
    } else if !draft.hetzner.token.is_empty() {
        Key::Unsaved
    } else if draft
        .settings
        .hetzner
        .as_ref()
        .is_some_and(|provider| draft.saved_credentials.contains(&provider.token_file))
    {
        Key::Saved
    } else {
        Key::Missing
    }
}

/// The key a selected agent needs, or `None` when it signs in with its own subscription.
pub(super) fn agent_key(draft: &Draft, agent: Agent) -> Option<Key> {
    let (mode, typed, file) = match agent {
        Agent::Codex => (
            draft.openai_auth,
            !draft.openai_key.is_empty(),
            draft.settings.openai_api_key_file.as_ref(),
        ),
        Agent::Claude => (
            draft.anthropic_auth,
            !draft.anthropic_key.is_empty(),
            draft.settings.anthropic_api_key_file.as_ref(),
        ),
        Agent::Grok => return None,
    };
    (mode == Authentication::ApiKey).then(|| {
        if typed {
            Key::Unsaved
        } else if file.is_some_and(|path| draft.saved_credentials.contains(path)) {
            Key::Saved
        } else {
            Key::Missing
        }
    })
}

pub(super) fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude",
        Agent::Grok => "Grok",
    }
}

/// The selected agents' keys, in order, for the agents that use one.
fn agent_keys(draft: &Draft) -> Vec<(Agent, Key)> {
    draft
        .selected_agents()
        .iter()
        .filter_map(|agent| agent_key(draft, *agent).map(|key| (*agent, key)))
        .collect()
}

/// What the agents card says about the selected agents.
pub(super) fn agents_status(draft: &Draft, fixed_agents: bool) -> (Tone, &'static str) {
    let keys = agent_keys(draft);
    if draft.selected_agents().is_empty() {
        // A profile that runs no agent fixes the choice to none; there is nothing to pick.
        if fixed_agents {
            (Tone::Ready, "None required")
        } else {
            (Tone::Attention, "Choose one")
        }
    } else if keys.iter().any(|(_, key)| *key == Key::Missing) {
        (Tone::Attention, "Needs a key")
    } else if keys.iter().any(|(_, key)| *key == Key::Unsaved) {
        (Tone::Attention, "Unsaved key")
    } else {
        (Tone::Ready, "Ready")
    }
}

/// What the banner says, in the order a person would fix things.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Readiness {
    pub tone: Tone,
    pub title: &'static str,
    pub cause: String,
}

impl Readiness {
    /// `fixed_agents`: the agents are fixed by the profile being started, so none may be required.
    pub(super) fn of(draft: &Draft, ssh_ready: Option<bool>, verified: &Verified, fixed_agents: bool) -> Self {
        let attention = |cause: String| Self {
            tone: Tone::Attention,
            title: "Almost ready to launch",
            cause,
        };
        let (runpod, hetzner) = (runpod_key(draft), hetzner_key(draft));
        if runpod == Key::Missing && hetzner == Key::Missing {
            return attention("Paste a RunPod or Hetzner key so clouds have somewhere to run.".into());
        }
        if hetzner == Key::Missing && draft.hetzner.enabled {
            return attention("Paste the Hetzner token, or switch Hetzner off.".into());
        }
        if draft.selected_agents().is_empty() && !fixed_agents {
            return attention("Choose a coding agent.".into());
        }
        let agents = agent_keys(draft);
        if let Some((agent, _)) = agents.iter().find(|(_, key)| *key == Key::Missing) {
            return attention(format!(
                "Paste the {} API key, or choose subscription login.",
                agent_name(*agent)
            ));
        }
        if runpod == Key::Unsaved || hetzner == Key::Unsaved || agents.iter().any(|(_, key)| *key == Key::Unsaved) {
            return attention("Save settings to keep the new key on this computer.".into());
        }
        match ssh_ready {
            Some(false) if draft.settings.ssh_identity_file.exists() => {
                return attention("The SSH identity is incomplete or invalid; restore its .pub file.".into());
            }
            Some(false) => return attention("Save settings and Horizon makes your SSH identity.".into()),
            _ => {}
        }
        for registry in &draft.registries {
            match &registry.original {
                None => return attention("Save the image repository, then validate its pull access.".into()),
                Some(binding) if !verified.contains_key(&binding.repository) => {
                    return attention(format!("Validate pull access for {}.", binding.repository));
                }
                Some(_) => {}
            }
        }
        Self {
            tone: Tone::Ready,
            title: "Settings complete",
            cause: "Horizon checks the provider account when you start a cloud.".into(),
        }
    }
}

/// A status dot and its word together, so the color never stands alone. Laid out for a
/// right-to-left parent: the word is added first, so the dot ends up before it.
pub(super) fn chip(ui: &mut Ui, tone: Tone, text: &str) {
    ui.spacing_mut().item_spacing.x = 6.0;
    let label = ui.label(RichText::new(text).size(12.0).color(tone.color()));
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    let center = egui::pos2(rect.center().x, label.rect.center().y);
    ui.painter().circle_filled(center, 4.0, tone.color());
}

fn dot(ui: &mut Ui, tone: Tone, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, tone.color());
}

pub(super) fn surface(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            content(ui);
        });
}

pub(super) fn header(ui: &mut Ui, title: &str, note: &str, status: Option<(Tone, &str)>) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).size(15.0).strong().color(theme::FG()));
            ui.label(RichText::new(note).size(12.0).color(theme::FG_DIM()));
        });
        if let Some((tone, text)) = status {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| chip(ui, tone, text));
        }
    });
    ui.add_space(4.0);
}

pub(super) fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::FG_DIM()));
}

pub(super) fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::FG_SOFT()));
}

/// A plain single-line field under its label.
pub(super) fn field(ui: &mut Ui, title: &str, value: &mut String, hint: &str) -> egui::Response {
    label(ui, title);
    ui.add_sized(
        [ui.available_width(), FIELD_HEIGHT],
        egui::TextEdit::singleline(value)
            .id_salt(title)
            .hint_text(hint)
            .margin(vec2(12.0, 9.0)),
    )
}

/// A secret entry. A saved credential is kept unless something is typed over it.
pub(super) fn secret(ui: &mut Ui, id: &str, value: &mut String, saved: bool, empty_hint: &str) {
    ui.add_sized(
        [ui.available_width(), FIELD_HEIGHT],
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .password(true)
            .margin(vec2(12.0, 9.0))
            .hint_text(if saved { super::SAVED_SECRET_HINT } else { empty_hint }),
    )
    .on_hover_text(if saved {
        "Enter a replacement, or leave empty to keep the saved credential."
    } else {
        "Enter a credential to save on this computer."
    });
}

/// A masked saved key in a well. Returns whether Replace was clicked.
pub(super) fn saved_key(ui: &mut Ui) -> bool {
    let mut replace = false;
    Frame::new()
        .fill(theme::BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("••••••••••••").size(13.0).color(theme::FG_SOFT()));
                ui.label(
                    RichText::new("Saved on this computer")
                        .size(12.0)
                        .color(theme::FG_DIM()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    replace = ui.add(chrome_button("Replace").min_size(vec2(72.0, 28.0))).clicked();
                });
            });
        });
    replace
}

pub(super) fn banner(ui: &mut Ui, readiness: &Readiness) {
    let color = readiness.tone.color();
    Frame::new()
        .fill(theme::alpha(color, 24))
        .stroke(Stroke::new(1.0, theme::alpha(color, 90)))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                dot(ui, readiness.tone, 10.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(readiness.title).size(16.0).strong().color(theme::FG()));
                    ui.label(RichText::new(&readiness.cause).size(13.0).color(theme::FG_SOFT()));
                });
            });
        });
}

/// The banner for the settings being edited.
pub(super) fn readiness_banner(ui: &mut Ui, state: &State) {
    if let Some(draft) = &state.draft {
        banner(
            ui,
            &Readiness::of(draft, state.ssh_ready, &state.verified, state.required_agents.is_some()),
        );
    }
}

/// Every card, in two columns when there is room. Returns a registry action a button asked for.
pub(super) fn page(ui: &mut Ui, state: &mut State) -> Option<Action> {
    let State {
        draft,
        edits,
        verified,
        ssh_ready,
        required_agents,
        ..
    } = state;
    let draft = draft.as_deref_mut()?;
    let fixed_agents = required_agents.is_some();
    let readiness = Readiness::of(draft, *ssh_ready, verified, fixed_agents);
    let mut action = None;
    ui.spacing_mut().item_spacing = vec2(GAP, GAP);
    if ui.available_width() < STACKED_BELOW {
        fields::providers(ui, draft, edits);
        fields::agents(ui, draft, edits, fixed_agents);
        action = registry::card(ui, draft, verified);
        fields::workspace(ui, draft, *ssh_ready, &readiness);
    } else {
        ui.columns(2, |columns| {
            columns[0].spacing_mut().item_spacing.y = GAP;
            columns[1].spacing_mut().item_spacing.y = GAP;
            fields::providers(&mut columns[0], draft, edits);
            fields::agents(&mut columns[0], draft, edits, fixed_agents);
            action = registry::card(&mut columns[1], draft, verified);
            fields::workspace(&mut columns[1], draft, *ssh_ready, &readiness);
        });
    }
    action
}
