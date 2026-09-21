//! Machine account form; filesystem work and key generation run outside rendering.
mod fields;
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
}

#[derive(Default)]
pub(in crate::app::cloud_panel) struct State {
    pub(in crate::app::cloud_panel) open: bool,
    continue_creation: bool,
    receiver: Option<Receiver<Completion>>,
    draft: Option<Box<Draft>>,
    error: Option<String>,
}

impl HorizonApp {
    pub(in crate::app) fn render_cloud_menu(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(self.cloud_prototype.ready, egui::Button::new("New cloud…"))
            .clicked()
        {
            if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some() {
                self.add_mock_cloud(ui.ctx());
            } else {
                self.open_cloud_accounts(ui.ctx(), true);
            }
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

    pub(in crate::app) fn open_cloud_accounts(&mut self, ctx: &Context, continue_creation: bool) {
        let root = self
            .cloud_prototype
            .root
            .clone()
            .unwrap_or_else(|| horizon_core::HorizonHome::resolve().root().join("cloud"));
        self.cloud_prototype.production.creating = false;
        let (sender, receiver) = channel();
        self.cloud_prototype.production.setup = State {
            open: true,
            continue_creation,
            receiver: Some(receiver),
            ..State::default()
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let completion = match Draft::load(&root) {
                Ok(draft) => {
                    let configured = draft.validate().is_ok() && draft.settings.ssh_identity_file.is_file();
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
        let agents = match &completion {
            Completion::Loaded(draft, _) => Some(draft.settings.default_agents.clone()),
            Completion::Saved(agents) => Some(agents.clone()),
            _ => None,
        };
        let proceed = match completion {
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
                        if let Some(draft) = &mut state.draft {
                            fields::render(ui, draft);
                        }
                        if let Some(error) = &state.error {
                            ui.colored_label(theme::PALETTE_RED(), error);
                        }
                        if state.receiver.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Preparing cloud settings…");
                            });
                        }
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
                                    "Save and continue"
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
            *state = State::default();
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
        }
    }

    fn save_cloud_accounts(&mut self, ctx: &Context) {
        let state = &mut self.cloud_prototype.production.setup;
        let Some(draft) = state.draft.take() else { return };
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
            let completion = match draft.save() {
                Ok(settings) => Completion::Saved(settings.default_agents),
                Err(error) => Completion::Failed(format!("{error}. Reopen Cloud settings to try again.")),
            };
            let _ = sender.send(completion);
            ctx.request_repaint();
        });
    }
}
