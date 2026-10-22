//! Machine account form; filesystem work and key generation run outside rendering.
mod chatgpt;
mod dashboard;
mod docker_hosts;
mod fields;
mod github;
mod needed;
mod registry;
#[cfg(all(test, unix))]
mod tests;

use super::HorizonApp;
use crate::theme;
use egui::{Context, Id, RichText};
use horizon_core::cloud_runtime::setup::Draft;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

const SAVED_SECRET_HINT: &str = "••••••••  Saved credential";

/// Space kept between the settings dialog and each window edge.
const DIALOG_INSET: f32 = 32.0;
/// The tallest the dialog gets inside its frame; taller content scrolls.
const DIALOG_MAX_HEIGHT: f32 = 950.0;
const DIALOG_MARGIN: i8 = 24;
const DASHBOARD_MIN_HEIGHT: f32 = 100.0;
const FOOTER_GAP: f32 = 16.0;
const FOOTER_SEPARATOR: f32 = 6.0;
const FOOTER_HEIGHT: f32 = 40.0;

/// The settings dialog's height inside its frame, taken from the window alone.
///
/// The dialog lays out inside the rectangle it had on the previous frame, so a scroll area
/// sized by the room left there grows only a little each frame while the centered dialog moves
/// up to make room. Fixing the height from the window lets the first frame show the final size
/// and position, whether the settings are still loading or a card later grows.
fn dialog_height(viewport: egui::Rect) -> f32 {
    (viewport.height() - 2.0 * (DIALOG_INSET + f32::from(DIALOG_MARGIN))).min(DIALOG_MAX_HEIGHT)
}

/// Everything [`State::render_footer`] lays out below the dashboard, with its item spacing.
fn footer_height(ui: &egui::Ui) -> f32 {
    let spacing = ui.spacing().item_spacing.y;
    spacing + FOOTER_GAP + FOOTER_SEPARATOR + spacing + FOOTER_HEIGHT
}

/// Which footer button was clicked this frame.
#[derive(Default)]
struct Footer {
    save: bool,
    cancel: bool,
}

/// Everything the settings thread learns when the form opens.
struct Loaded {
    draft: Draft,
    /// Whether the settings are complete enough to go straight on to a first cloud.
    configured: bool,
    ssh_ready: bool,
    /// Saved validation results, read from each binding's journal without asking a provider.
    verified: dashboard::Verified,
    /// The image repository of the cloud these settings were opened for, and its state
    /// when no binding covers it.
    needed: Option<(String, Option<horizon_core::cloud_runtime::registry::Needed>)>,
}

/// What a registry action found, reduced to what the form shows.
struct RegistryOutcome {
    message: String,
    repository: String,
    generation: String,
    /// The action's saved host check or validation with active provider access.
    verified: Option<horizon_core::cloud_runtime::registry::Validation>,
    fresh: bool,
}

enum Completion {
    Loaded(Box<Loaded>),
    Saved(Vec<horizon_core::cloud_runtime::setup::Agent>),
    Failed(String),
    Invalid(Box<Draft>, String),
    Registry(std::result::Result<RegistryOutcome, String>),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Continuation {
    #[default]
    None,
    StartWhenReady,
    ReturnForReview,
}

#[derive(Default)]
pub(in crate::app::cloud_panel) struct State {
    pub(in crate::app::cloud_panel) open: bool,
    continuation: Continuation,
    edits: fields::Edits,
    verified: dashboard::Verified,
    refused_pulls: std::collections::BTreeSet<String>,
    ssh_ready: Option<bool>,
    required_agents: Option<Vec<horizon_core::cloud_runtime::setup::Agent>>,
    receiver: Option<Receiver<Completion>>,
    draft: Option<Box<Draft>>,
    error: Option<String>,
    registry_status: Option<String>,
    registry_cancel: Option<horizon_core::cloud_runtime::Cancellation>,
    /// Whether the dialog has been measured since it opened, in case the window changed size.
    measured: bool,
    github: github::Card,
    chatgpt: chatgpt::Card,
    docker_hosts: docker_hosts::Card,
    needed: needed::Needed,
}

impl State {
    pub(super) fn reset_form(&mut self) {
        *self = Self {
            refused_pulls: std::mem::take(&mut self.refused_pulls),
            ..Self::default()
        };
    }

    fn current_registry_generation(&self, repository: &str, generation: &str) -> bool {
        self.draft.as_ref().is_some_and(|draft| {
            draft.registries.iter().any(|registry| {
                registry
                    .original
                    .as_ref()
                    .is_some_and(|binding| binding.repository == repository && binding.generation == generation)
            })
        })
    }

    /// Shows what a registry action found and keeps the proof, or drops it, for that grant.
    fn record_registry(&mut self, outcome: RegistryOutcome) {
        self.registry_status = Some(outcome.message);
        let current = self.current_registry_generation(&outcome.repository, &outcome.generation);
        if !current {
            return;
        }
        if outcome.fresh && outcome.verified.is_some() {
            self.refused_pulls.remove(&outcome.repository);
        } else if self.refused_pulls.contains(&outcome.repository) {
            self.verified.remove(&outcome.repository);
            return;
        }
        match outcome.verified {
            Some(validation) => {
                self.verified.insert(outcome.repository, validation);
            }
            None => {
                self.verified.remove(&outcome.repository);
            }
        }
    }

    /// Shows the `ChatGPT` login that a background sync found, such as one that `OpenAI`
    /// refused, unless a sign-in or sign-out of this form is under way.
    pub(in crate::app::cloud_panel) fn refresh_chatgpt(
        &mut self,
        account: Option<horizon_core::cloud_runtime::codex_login::Account>,
    ) {
        if self.chatgpt.busy() {
            return;
        }
        if let Some(draft) = self.draft.as_deref_mut() {
            draft.chatgpt = account;
        }
    }

    /// Whether these settings resume a cloud creation once they are saved.
    pub(in crate::app::cloud_panel) fn resumes_creation(&self) -> bool {
        self.open && self.continuation != Continuation::None
    }

    fn render_fields(
        &mut self,
        ui: &mut egui::Ui,
        hosts: &mut super::prices::hosts::State,
    ) -> Option<registry::Request> {
        let state = self;
        let registry_action = ui
            .add_enabled_ui(state.receiver.is_none(), |ui| dashboard::page(ui, state, hosts))
            .inner;
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

    /// The separator and the Save and Cancel row, exactly [`footer_height`] tall.
    fn render_footer(&self, ui: &mut egui::Ui) -> Footer {
        ui.add_space(FOOTER_GAP);
        ui.add(egui::Separator::default().spacing(FOOTER_SEPARATOR));
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), FOOTER_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let save = ui
                    .add_enabled(
                        // A Connect GitHub flow under way saves the app itself, and a ChatGPT
                        // sign-in keeps its own login; Save would close the form before their
                        // outcome shows. Cancel ends them.
                        self.draft.is_some()
                            && self.receiver.is_none()
                            && !self.github.connecting()
                            && !self.chatgpt.busy()
                            && !self.docker_hosts.editing(),
                        egui::Button::new(match self.continuation {
                            Continuation::None => "Save settings",
                            Continuation::StartWhenReady => "Save and start",
                            Continuation::ReturnForReview => "Save and return",
                        })
                        .min_size(egui::vec2(148.0, FOOTER_HEIGHT))
                        .corner_radius(10)
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.35)),
                    )
                    .clicked();
                let cancel = ui
                    .add_enabled(
                        self.receiver.is_none(),
                        egui::Button::new("Cancel").min_size(egui::vec2(80.0, FOOTER_HEIGHT)),
                    )
                    .clicked();
                Footer { save, cancel }
            },
        )
        .inner
    }
}

impl HorizonApp {
    pub(in crate::app) fn cloud_launch_ready(&self) -> bool {
        self.cloud_prototype.ready
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
        self.open_cloud_settings(ctx, continue_creation, None);
    }

    pub(super) fn edit_creation_registry(&mut self, ctx: &Context, repository: Option<String>) {
        self.open_cloud_settings_for(
            ctx,
            Continuation::ReturnForReview,
            repository.map(|repository| (repository, false)),
        );
    }

    /// Opens the settings; with a cloud's image, Container registry shows its repository
    /// when no binding covers it yet. When the registry refused that cloud's pull, the
    /// repository's saved pull validation is not shown as verified.
    pub(super) fn open_cloud_settings(
        &mut self,
        ctx: &Context,
        continue_creation: bool,
        cloud: Option<(String, bool)>,
    ) {
        let continuation = if continue_creation {
            Continuation::StartWhenReady
        } else {
            Continuation::None
        };
        self.open_cloud_settings_for(ctx, continuation, cloud);
    }

    fn open_cloud_settings_for(&mut self, ctx: &Context, continuation: Continuation, cloud: Option<(String, bool)>) {
        let root = self
            .cloud_prototype
            .root
            .clone()
            .unwrap_or_else(|| horizon_core::HorizonHome::resolve().root().join("cloud"));
        let required_agents = (continuation == Continuation::StartWhenReady)
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
        self.cloud_prototype.production.publishing = super::creation::publishing::State::default();
        self.cloud_prototype.production.pending_creation = None;
        if continuation == Continuation::ReturnForReview {
            self.cloud_prototype.production.launch.watch = None;
            self.cloud_prototype.production.launch.watch_quote = None;
            self.cloud_prototype.production.launch.price_rose = None;
            self.cloud_prototype.production.launch.submitted = false;
            self.cloud_prototype.production.launch.invalidate_account_readiness();
        }
        let (sender, receiver) = channel();
        let state = &mut self.cloud_prototype.production.setup;
        if let Some(repository) = cloud
            .as_ref()
            .filter(|(_, pull)| *pull)
            .and_then(|(image, _)| horizon_core::cloud_runtime::registry::repository_of(image))
        {
            state.refused_pulls.insert(repository.to_owned());
        }
        let refused_pulls = std::mem::take(&mut state.refused_pulls);
        self.cloud_prototype.production.setup = State {
            refused_pulls,
            open: true,
            continuation,
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
                    let ssh_ready = cloud_runtime_ssh_valid(&draft.settings.ssh_identity_file);
                    let configured = draft.validate().is_ok() && ssh_ready;
                    let mut verified = saved_validations(&draft);
                    let needed =
                        cloud.and_then(|(image, pull)| needed::of_cloud(&draft.settings, &image, pull, &mut verified));
                    Completion::Loaded(Box::new(Loaded {
                        draft,
                        configured,
                        ssh_ready,
                        verified,
                        needed,
                    }))
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
            Completion::Loaded(loaded) => Some(loaded.draft.settings.default_agents.clone()),
            Completion::Saved(agents) => Some(agents.clone()),
            _ => None,
        };
        let return_for_review =
            matches!(completion, Completion::Saved(_)) && state.continuation == Continuation::ReturnForReview;
        let proceed = match completion {
            Completion::Registry(result) => {
                match result {
                    Ok(outcome) => state.record_registry(outcome),
                    Err(error) => state.error = Some(error),
                }
                false
            }
            Completion::Loaded(loaded) => {
                let Loaded {
                    draft,
                    configured,
                    ssh_ready,
                    verified,
                    needed,
                } = *loaded;
                state.draft = Some(Box::new(draft));
                state.ssh_ready = Some(ssh_ready);
                state.verified = verified;
                state
                    .verified
                    .retain(|repository, _| !state.refused_pulls.contains(repository));
                state.needed = needed::Needed::new(needed);
                configured && state.continuation == Continuation::StartWhenReady
            }
            Completion::Saved(_) => {
                state.open = false;
                // Prices answered to the credentials before these were saved prove nothing now.
                self.cloud_prototype.production.prices.restart();
                self.cloud_prototype.production.checks = super::creation::checks::State::default();
                state.continuation == Continuation::StartWhenReady
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
            state.reset_form();
            self.cloud_prototype.production.launch.accounts_checked = true;
            self.add_mock_cloud(ctx);
        } else if return_for_review {
            self.return_to_cloud_review();
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
        let production = &mut self.cloud_prototype.production;
        let (state, hosts) = (&mut production.setup, &mut production.prices.hosts);
        if !state.open {
            return;
        }
        // Saved hosts show what they have free, read again while the settings are open.
        hosts.poll();
        if let Some(draft) = &state.draft {
            hosts.request(draft.root(), ctx);
        }
        let mut footer = Footer::default();
        let mut registry_action = None;
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let id = Id::unique("cloud-accounts");
        // Measure on the opening frame, before the dialog is drawn, so a size remembered from a
        // window of another size cannot place the first visible frame.
        let sizing_pass = !std::mem::replace(&mut state.measured, true);
        let response = egui::Modal::new(id)
            .area(
                egui::Modal::default_area(id)
                    .order(egui::Order::Tooltip)
                    .sizing_pass(sizing_pass),
            )
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(16)
                    .inner_margin(DIALOG_MARGIN),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 2.0 * DIALOG_INSET).clamp(240.0, 1060.0));
                let bottom = ui.cursor().top() + dialog_height(ctx.content_rect());
                ui.label(
                    RichText::new(if state.continuation == Continuation::StartWhenReady {
                        "Your first cloud"
                    } else {
                        "Cloud settings"
                    })
                    .size(24.0)
                    .strong()
                    .color(theme::FG()),
                );
                ui.label(
                    RichText::new("What is connected, and what still needs you.")
                        .size(13.0)
                        .color(theme::FG_SOFT()),
                );
                ui.add_space(16.0);
                dashboard::readiness_banner(ui, state);
                ui.add_space(16.0);
                let dashboard_height = (bottom - ui.cursor().top() - footer_height(ui)).max(DASHBOARD_MIN_HEIGHT);
                super::super::runtime::solid_scroll_area(ui)
                    .id_salt("cloud-settings-dashboard")
                    .auto_shrink(false)
                    .min_scrolled_height(dashboard_height)
                    .max_height(dashboard_height)
                    .show(ui, |ui| {
                        registry_action = state.render_fields(ui, hosts);
                    });
                footer = state.render_footer(ui);
            });
        ctx.move_to_top(response.response.layer_id);
        // Once Save starts, retain its completion and do not imply cancellation of writes.
        if (footer.cancel || response.should_close()) && state.receiver.is_none() {
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
        } else if footer.save {
            self.save_cloud_accounts(ctx);
        } else if let Some(action) = registry_action {
            self.manage_cloud_registry(ctx, action);
        }
    }

    fn manage_cloud_registry(&mut self, ctx: &Context, request: registry::Request) {
        let registry::Request {
            action,
            repository,
            generation,
        } = request;
        let state = &mut self.cloud_prototype.production.setup;
        let Some(draft) = &state.draft else { return };
        if action.scope() == horizon_core::cloud_runtime::registry::Scope::Provider && !draft.runpod_key.is_empty() {
            state.error = Some("Clear or save the unsaved compute key before managing provider access.".into());
            return;
        }
        let settings = draft.settings.clone();
        state.error = None;
        state.registry_status = None;
        let current = state.current_registry_generation(&repository, &generation);
        match &action {
            horizon_core::cloud_runtime::registry::Action::Verify { image }
            | horizon_core::cloud_runtime::registry::Action::VerifyPull { image } => {
                if !current {
                    state.error = Some("The saved pull grant changed; reopen settings before validating.".into());
                    return;
                }
                state.verified.remove(&repository);
                state.refused_pulls.insert(repository.clone());
                if horizon_core::cloud_runtime::registry::repository_of(image) != Some(repository.as_str()) {
                    state.error =
                        Some("Enter a valid immutable image for this saved repository before validating.".into());
                    return;
                }
            }
            _ if current => {
                state.verified.remove(&repository);
            }
            _ => {}
        }
        let cancellation = horizon_core::cloud_runtime::Cancellation::default();
        state.registry_cancel = Some(cancellation.clone());
        let (sender, receiver) = channel();
        state.receiver = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = horizon_core::cloud_runtime::registry::manage(&settings, &action, &cancellation)
                .map(|status| {
                    let mut outcome = registry_outcome(&status, &action);
                    if !outcome.fresh && status.scope == horizon_core::cloud_runtime::registry::Scope::Provider {
                        outcome.verified =
                            match cached_pull_validation(&settings, &status.repository, &status.generation) {
                                Ok(Some(saved)) => Some(saved),
                                Ok(None) => outcome.verified,
                                Err(_) => {
                                    outcome
                                        .message
                                        .push_str(" Host pull history is unavailable; validate pull access again.");
                                    None
                                }
                            };
                    }
                    outcome
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(Completion::Registry(result));
            ctx.request_repaint();
        });
    }

    fn cancel_cloud_accounts(&mut self) {
        let continuation = self.cloud_prototype.production.setup.continuation;
        let resume_creation = continuation == Continuation::ReturnForReview
            || (continuation == Continuation::StartWhenReady
                && self.cloud_prototype.production.launch.workspace.is_some());
        self.cloud_prototype.production.setup.reset_form();
        if resume_creation {
            self.return_to_cloud_review();
        }
    }

    fn return_to_cloud_review(&mut self) {
        let form = &mut self.cloud_prototype.production;
        form.setup.reset_form();
        form.launch.submitted = false;
        form.launch.invalidate_account_readiness();
        form.open_creation();
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

/// The message a registry action leaves in the form, and the validation it proves.
fn registry_outcome(
    status: &horizon_core::cloud_runtime::registry::Status,
    action: &horizon_core::cloud_runtime::registry::Action,
) -> RegistryOutcome {
    use horizon_core::cloud_runtime::registry::{Action, Scope, State};
    let label = match (status.scope, &status.state) {
        (Scope::Host, State::Prepared) => {
            if status.validation.is_some() {
                "Pull check saved"
            } else {
                "Not checked"
            }
        }
        _ => provider_registry_label(&status.state),
    };
    let message = status.validation.as_ref().map_or_else(
        || {
            format!(
                "{label}. Image access has not been validated. Configured pull expiry: {}.",
                status.configured_pull_expiry.as_deref().unwrap_or("Unknown")
            )
        },
        |validation| {
            format!(
                "{label}. Last verified image: {}. Scope: {}. Expiry: {}.",
                validation.image,
                validation.scope,
                validation.expires_at.as_deref().unwrap_or("Unknown")
            )
        },
    );
    // A host check is historical pull validation; provider access additionally needs its binding.
    let verified = matches!(
        (status.scope, &status.state),
        (Scope::Host, State::Prepared) | (Scope::Provider, State::Bound(_))
    )
    .then(|| status.validation.clone())
    .flatten()
    .filter(|_| !matches!(action, Action::Revoke { .. }));
    let scope = if status.scope == Scope::Host {
        "Host pull access"
    } else {
        "Provider registry access"
    };
    RegistryOutcome {
        message: format!("{scope}: {message}"),
        repository: status.repository.clone(),
        generation: status.generation.clone(),
        verified,
        fresh: matches!(action, Action::Verify { .. } | Action::VerifyPull { .. }),
    }
}

fn provider_registry_label(state: &horizon_core::cloud_runtime::registry::State) -> &'static str {
    use horizon_core::cloud_runtime::registry::State;
    match state {
        State::Prepared => "Not prepared",
        State::Requested { .. } => "Creation uncertain; reconcile before retrying",
        State::Bound(_) => "Active",
        State::Revoking(_) => "Revocation pending; reconcile again",
        State::Revoked => "Revoked",
    }
}

/// Each binding's saved host pull validation; no compute credential is loaded.
fn saved_validations(draft: &Draft) -> dashboard::Verified {
    let mut found = dashboard::Verified::new();
    if let Some(config) = &draft.settings.registries {
        for binding in &config.bindings {
            if let Ok(Some(validation)) =
                cached_pull_validation(&draft.settings, &binding.repository, &binding.generation)
            {
                found.insert(binding.repository.clone(), validation);
            }
        }
    }
    found
}

// Host history never opens the provider journal or reads its compute credential.
// A fresh verification displays the action's own scoped result.
fn cached_pull_validation(
    settings: &horizon_core::cloud_runtime::settings::Settings,
    repository: &str,
    generation: &str,
) -> horizon_core::cloud_runtime::Result<Option<horizon_core::cloud_runtime::registry::Validation>> {
    use horizon_core::cloud_runtime::registry::{Action, manage};
    let cancellation = horizon_core::cloud_runtime::Cancellation::default();
    let host = Action::PullStatus {
        repository: repository.into(),
        generation: generation.into(),
    };
    let status = manage(settings, &host, &cancellation)?;
    Ok(registry_outcome(&status, &host).verified)
}

fn cloud_runtime_ssh_valid(path: &std::path::Path) -> bool {
    horizon_core::cloud_runtime::repository::launch::ssh_ready(path)
}
