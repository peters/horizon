//! Machine account form; filesystem work and key generation run outside rendering.
mod fields;
mod registry;
#[cfg(all(test, unix))]
mod tests;

use super::HorizonApp;
use crate::theme;
use egui::{Context, Id, RichText};
use horizon_core::cloud_runtime::setup::Draft;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

enum Completion {
    Loaded(Box<Draft>, bool),
    Saved(Vec<horizon_core::cloud_runtime::setup::Agent>),
    Failed(String),
    Invalid(Box<Draft>, String),
    Registry(std::result::Result<String, String>),
}

#[derive(Default)]
pub(in crate::app::cloud_panel) struct State {
    pub(in crate::app::cloud_panel) open: bool,
    continue_creation: bool,
    required_agents: Option<Vec<horizon_core::cloud_runtime::setup::Agent>>,
    receiver: Option<Receiver<Completion>>,
    draft: Option<Box<Draft>>,
    error: Option<String>,
    registry_status: Option<String>,
    registry_cancel: Option<horizon_core::cloud_runtime::Cancellation>,
}

impl State {
    /// Whether these settings resume a cloud creation once they are saved.
    pub(in crate::app::cloud_panel) fn resumes_creation(&self) -> bool {
        self.open && self.continue_creation
    }

    fn render_fields(&mut self, ui: &mut egui::Ui) -> Option<horizon_core::cloud_runtime::registry::Action> {
        let state = self;
        let mut registry_action = None;
        if let Some(draft) = &mut state.draft {
            ui.add_enabled_ui(state.receiver.is_none(), |ui| {
                if state.required_agents.is_some() {
                    fields::render_profile(ui, draft, true);
                } else {
                    fields::render(ui, draft);
                }
                registry_action = registry::render(ui, draft);
            });
        }
        if let Some(status) = &state.registry_status {
            ui.label(status);
        }
        if let Some(error) = &state.error {
            ui.colored_label(theme::PALETTE_RED(), error);
        }
        if state.receiver.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing cloud settings…");
            });
            if let Some(cancel) = &state.registry_cancel
                && ui.button("Cancel registry operation").clicked()
            {
                cancel.cancel();
            }
        }
        registry_action
    }
}

impl HorizonApp {
    pub(in crate::app) fn cloud_launch_ready(&self) -> bool {
        self.cloud_prototype.ready
    }

    pub(in crate::app) fn render_cloud_menu(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(self.cloud_launch_ready(), egui::Button::new("New cloud…"))
            .clicked()
        {
            let workspace = self.board.ensure_workspace();
            self.open_cloud_for_workspace(ui.ctx(), workspace);
            ui.close();
        }
        if ui.button("Cloud settings…").clicked() {
            self.open_cloud_accounts(ui.ctx(), false);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !self.cloud_prototype.groups.0.is_empty(),
                egui::Button::new("Fit all clouds"),
            )
            .clicked()
        {
            self.cloud_overview(ui.ctx());
            ui.close();
        }
    }

    pub(in crate::app) fn open_cloud_for_workspace(&mut self, ctx: &Context, workspace: horizon_core::WorkspaceId) {
        if !self.cloud_launch_ready() {
            return;
        }
        if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some() {
            self.add_mock_cloud_in_workspace(ctx, workspace);
        } else {
            self.open_workspace_cloud(ctx, workspace);
        }
    }

    pub(in crate::app) fn open_cloud_accounts(&mut self, ctx: &Context, continue_creation: bool) {
        let root = self
            .cloud_prototype
            .root
            .clone()
            .unwrap_or_else(|| horizon_core::HorizonHome::resolve().root().join("cloud"));
        let required_agents = continue_creation
            .then(|| {
                let form = &self.cloud_prototype.production;
                form.profiles
                    .as_ref()?
                    .profiles
                    .get(&form.selected_profile)
                    .map(|profile| profile.capabilities.agents.iter().copied().collect::<Vec<_>>())
            })
            .flatten();
        self.cloud_prototype.production.creating = false;
        self.cloud_prototype.production.pending_creation = None;
        let (sender, receiver) = channel();
        self.cloud_prototype.production.setup = State {
            open: true,
            continue_creation,
            required_agents: required_agents.clone(),
            receiver: Some(receiver),
            ..State::default()
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let completion = match Draft::load(&root) {
                Ok(mut draft) => {
                    if let Some(agents) = required_agents {
                        draft.select_profile_agents(agents);
                    }
                    let configured =
                        draft.validate().is_ok() && cloud_runtime_ssh_valid(&draft.settings.ssh_identity_file);

                    Completion::Loaded(Box::new(draft), configured)
                }
                Err(error) => Completion::Failed(error.to_string()),
            };
            let _ = sender.send(completion);
            ctx.request_repaint();
        });
    }

    fn poll_cloud_accounts(&mut self, ctx: &Context) {
        let state = &mut self.cloud_prototype.production.setup;
        let result = state.receiver.as_ref().map(Receiver::try_recv);
        let completion = match result {
            Some(Ok(completion)) => completion,
            Some(Err(TryRecvError::Disconnected)) => {
                Completion::Failed("Cloud settings operation interrupted. Reopen settings to retry.".into())
            }
            _ => return,
        };
        state.receiver = None;
        state.registry_cancel = None;
        let agents = match &completion {
            Completion::Loaded(draft, _) => Some(draft.settings.default_agents.clone()),
            Completion::Saved(agents) => Some(agents.clone()),
            _ => None,
        };
        let proceed = match completion {
            Completion::Registry(result) => {
                match result {
                    Ok(status) => state.registry_status = Some(status),
                    Err(error) => state.error = Some(error),
                }
                false
            }
            Completion::Loaded(draft, configured) => {
                state.draft = Some(draft);
                configured && state.continue_creation
            }
            Completion::Saved(_) => {
                state.open = false;
                state.continue_creation
            }
            Completion::Failed(error) => {
                state.error = Some(error);
                false
            }
            Completion::Invalid(draft, error) => {
                state.draft = Some(draft);
                state.error = Some(error);
                false
            }
        };
        if proceed {
            *state = State::default();
            self.cloud_prototype.production.launch.accounts_checked = true;
            self.add_mock_cloud(ctx);
        }
        if let Some(agents) = agents {
            let form = &mut self.cloud_prototype.production;
            form.setup_agent = agents.first().and_then(|agent| match agent {
                horizon_core::cloud_runtime::setup::Agent::Codex => Some(super::PanelKind::Codex),
                horizon_core::cloud_runtime::setup::Agent::Claude => Some(super::PanelKind::Claude),
                horizon_core::cloud_runtime::setup::Agent::Grok => None,
            });
            form.setup_agents = agents;
        }
    }

    pub(in crate::app::cloud_panel) fn render_cloud_accounts(&mut self, ctx: &Context) {
        self.poll_cloud_accounts(ctx);
        let state = &mut self.cloud_prototype.production.setup;
        if !state.open {
            return;
        }
        let mut save = false;
        let mut cancel = false;
        let mut registry_action = None;
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let id = Id::new("cloud-accounts");
        let response = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(16)
                    .inner_margin(24),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(240.0, 580.0));
                ui.label(
                    RichText::new(if state.continue_creation {
                        "Your first cloud"
                    } else {
                        "Cloud settings"
                    })
                    .size(26.0)
                    .strong(),
                );
                ui.label(RichText::new("Connect your account. Choose who you work with.").color(theme::FG_SOFT()));
                ui.add_space(16.0);
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 240.0).max(100.0))
                    .show(ui, |ui| {
                        registry_action = state.render_fields(ui);
                    });
                ui.add_space(16.0);
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 40.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        save = ui
                            .add_enabled(
                                state.draft.is_some() && state.receiver.is_none(),
                                egui::Button::new(if state.continue_creation {
                                    "Save and start"
                                } else {
                                    "Save settings"
                                })
                                .min_size(egui::vec2(148.0, 40.0))
                                .corner_radius(10)
                                .fill(theme::blend(
                                    theme::PANEL_BG_ALT(),
                                    theme::ACCENT(),
                                    0.35,
                                )),
                            )
                            .clicked();
                        cancel = ui
                            .add_enabled(
                                state.receiver.is_none(),
                                egui::Button::new("Cancel").min_size(egui::vec2(80.0, 40.0)),
                            )
                            .clicked();
                    },
                );
            });
        ctx.move_to_top(response.response.layer_id);
        // Once Save starts, retain its completion and do not imply cancellation of writes.
        if (cancel || response.should_close()) && state.receiver.is_none() {
            self.cancel_cloud_accounts();
            if escape {
                self.consume_navigation_key(
                    ctx,
                    horizon_core::ShortcutBinding::new(
                        horizon_core::ShortcutModifiers::NONE,
                        horizon_core::ShortcutKey::Escape,
                    ),
                );
            }
        } else if save {
            self.save_cloud_accounts(ctx);
        } else if let Some(action) = registry_action {
            self.manage_cloud_registry(ctx, action);
        }
    }

    fn manage_cloud_registry(&mut self, ctx: &Context, action: horizon_core::cloud_runtime::registry::Action) {
        let state = &mut self.cloud_prototype.production.setup;
        let Some(draft) = &state.draft else { return };
        if !draft.runpod_key.is_empty() {
            state.error = Some("Clear or save the unsaved compute key before managing provider access.".into());
            return;
        }
        let settings = draft.settings.clone();
        let cancellation = horizon_core::cloud_runtime::Cancellation::default();
        state.registry_cancel = Some(cancellation.clone());
        state.error = None;
        state.registry_status = None;
        let (sender, receiver) = channel();
        state.receiver = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = horizon_core::cloud_runtime::registry::manage(&settings, &action, &cancellation)
                .map(|status| {
                    let state = match status.state {
                        horizon_core::cloud_runtime::registry::State::Prepared => "Not prepared",
                        horizon_core::cloud_runtime::registry::State::Requested { .. } => {
                            "Creation uncertain; reconcile before retrying"
                        }
                        horizon_core::cloud_runtime::registry::State::Bound(_) => "Active",
                        horizon_core::cloud_runtime::registry::State::Revoking(_) => {
                            "Revocation pending; reconcile again"
                        }
                        horizon_core::cloud_runtime::registry::State::Revoked => "Revoked",
                    };
                    status.validation.map_or_else(
                        || {
                            format!(
                                "{state}. Image access has not been validated. Configured pull expiry: {}.",
                                status.configured_pull_expiry.as_deref().unwrap_or("Unknown")
                            )
                        },
                        |validation| {
                            format!(
                                "{state}. Last verified image: {}. Scope: {}. Expiry: {}.",
                                validation.image,
                                validation.scope,
                                validation.expires_at.as_deref().unwrap_or("Unknown")
                            )
                        },
                    )
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(Completion::Registry(result));
            ctx.request_repaint();
        });
    }

    fn cancel_cloud_accounts(&mut self) {
        let resume_creation = self.cloud_prototype.production.setup.continue_creation
            && self.cloud_prototype.production.launch.workspace.is_some();
        self.cloud_prototype.production.setup = State::default();
        if resume_creation {
            let form = &mut self.cloud_prototype.production;
            form.launch.submitted = false;
            form.launch.accounts_checked = false;
            form.creating = true;
            form.focus_title_on_open = true;
        }
    }

    fn save_cloud_accounts(&mut self, ctx: &Context) {
        let state = &mut self.cloud_prototype.production.setup;
        let Some(mut draft) = state.draft.take() else { return };
        if let Some(agents) = &state.required_agents {
            draft.select_profile_agents(agents.clone());
        }
        state.error = None;
        let (sender, receiver) = channel();
        state.receiver = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Err(error) = draft.validate() {
                let _ = sender.send(Completion::Invalid(draft, error.to_string()));
                ctx.request_repaint();
                return;
            }
            if draft.settings.ssh_identity_file.exists() && !cloud_runtime_ssh_valid(&draft.settings.ssh_identity_file)
            {
                let _ = sender.send(Completion::Invalid(draft, "SSH keypair is incomplete or invalid. Restore the matching .pub file beside the configured private key, then retry.".into()));
                ctx.request_repaint();
                return;
            }
            let completion = match draft.clone().save() {
                Ok(settings) => Completion::Saved(settings.default_agents),
                Err(error) => Completion::Invalid(draft, format!("{error}. Check the settings and retry.")),
            };
            let _ = sender.send(completion);
            ctx.request_repaint();
        });
    }
}

fn cloud_runtime_ssh_valid(path: &std::path::Path) -> bool {
    horizon_core::cloud_runtime::repository::launch::ssh_ready(path)
}
