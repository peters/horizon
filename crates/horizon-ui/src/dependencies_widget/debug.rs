//! Hands the worker's diagnosis to an agent on this computer. Only health and policy
//! revisions are shared; prompts, logs and credentials stay on the worker.
use super::widgets;
use crate::theme;
use egui::RichText;
use horizon_core::{PanelKind, agent_definition, all_agent_kinds, maintenance::Endpoint};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

pub(crate) struct Launch {
    pub kind: PanelKind,
    pub prompt: String,
    pub cwd: PathBuf,
}

#[derive(Default)]
pub(super) struct State {
    opened: bool,
    selected: Option<PanelKind>,
    endpoint: Option<Endpoint>,
    prompt: String,
    summary: String,
    error: Option<String>,
    pending: Option<Receiver<Result<String, String>>>,
}

impl State {
    pub(super) fn open(&mut self, endpoint: Endpoint, ctx: &egui::Context) {
        self.opened = true;
        self.error = None;
        self.summary = "Collecting the worker's health and policy revisions…".into();
        self.selected = all_agent_kinds()
            .iter()
            .copied()
            .find(|kind| agent_definition(*kind).is_some_and(|agent| available(agent.default_command)))
            .or_else(|| all_agent_kinds().first().copied());
        self.prompt = endpoint.debug_prompt("No diagnostic reply received yet.");
        let (sender, receiver) = channel();
        self.pending = Some(receiver);
        let ctx = ctx.clone();
        let worker = endpoint.clone();
        std::thread::spawn(move || {
            let _ = sender.send(worker.diagnose());
            ctx.request_repaint();
        });
        self.endpoint = Some(endpoint);
    }

    fn poll(&mut self) {
        let Some(receiver) = &self.pending else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The diagnostic request ended without a reply.".into()),
        };
        self.pending = None;
        match result {
            Ok(summary) => self.summary = summary,
            Err(error) => {
                self.error = Some(error);
                self.summary = "SSH diagnostics unavailable. Agent health is unknown.".into();
            }
        }
        if let Some(endpoint) = &self.endpoint {
            self.prompt = endpoint.debug_prompt(&self.summary);
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context) -> Option<Launch> {
        self.poll();
        if !self.opened {
            return None;
        }
        let mut close = false;
        let mut launch = None;
        let id = egui::Id::new("maintenance-local-debug");
        let response = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
            .frame(widgets::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(240.0, 680.0));
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                self.heading(ui);
                ui.add_space(12.0);
                self.observed(ui);
                ui.add_space(12.0);
                let installed = self.agent(ui);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            installed && !self.prompt.is_empty(),
                            widgets::primary_button("Open local agent"),
                        )
                        .clicked()
                        && let Some(kind) = self.selected
                    {
                        launch = Some(Launch {
                            kind,
                            prompt: self.prompt.clone(),
                            cwd: self
                                .endpoint
                                .as_ref()
                                .map_or_else(PathBuf::new, |endpoint| endpoint.workdir().to_path_buf()),
                        });
                    }
                    if ui
                        .add_enabled(!self.prompt.is_empty(), widgets::secondary_button("Copy context"))
                        .clicked()
                    {
                        ctx.copy_text(self.prompt.clone());
                    }
                    if ui.add(widgets::secondary_button("Close")).clicked() {
                        close = true;
                    }
                });
            });
        self.opened = !(close || response.should_close()) && launch.is_none();
        launch
    }

    fn heading(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Debug the worker").size(24.0).strong().color(theme::FG()));
        ui.label(
            RichText::new("Diagnose this worker from an agent on this computer. No credentials are copied.")
                .size(13.5)
                .color(theme::FG_SOFT()),
        );
        if let Some(endpoint) = &self.endpoint {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(
                    RichText::new(endpoint.address())
                        .monospace()
                        .size(13.0)
                        .color(theme::FG()),
                );
                let route = if endpoint.synthetic() {
                    "Test worker · strict loopback SSH"
                } else {
                    "Strict SSH"
                };
                ui.label(RichText::new(route).size(12.5).color(theme::FG_SOFT()));
            });
        }
    }

    fn observed(&self, ui: &mut egui::Ui) {
        widgets::caption(ui, "Observed health and policy", None);
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Reading worker diagnostics over SSH…")
                        .size(13.0)
                        .color(theme::FG_SOFT()),
                );
            });
        }
        if let Some(error) = &self.error {
            egui::Frame::new()
                .fill(theme::alpha(theme::PALETTE_YELLOW(), 24))
                .stroke(egui::Stroke::new(1.0, theme::alpha(theme::PALETTE_YELLOW(), 90)))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(error).size(13.0).color(theme::PALETTE_YELLOW()));
                });
        }
        widgets::well().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(facts) = facts(&self.summary) else {
                ui.label(RichText::new(&self.summary).size(13.0).color(theme::FG_SOFT()));
                return;
            };
            egui::Grid::new("maintenance-debug-facts")
                .num_columns(2)
                .spacing([28.0, 8.0])
                .show(ui, |ui| {
                    for (label, value) in facts {
                        ui.label(RichText::new(label).size(13.0).color(theme::FG_SOFT()));
                        ui.label(RichText::new(value).size(13.5).color(theme::FG()));
                        ui.end_row();
                    }
                });
        });
    }

    /// The local agent choice. Returns true when the chosen agent is installed.
    fn agent(&mut self, ui: &mut egui::Ui) -> bool {
        widgets::caption(ui, "Local agent", None);
        let label = self
            .selected
            .and_then(agent_definition)
            .map_or("Choose an agent", |agent| agent.display_name);
        widgets::field(ui, |ui| {
            egui::ComboBox::from_id_salt("maintenance-debug-agent")
                .selected_text(RichText::new(label).size(13.5).color(theme::FG()))
                .width(320.0)
                .show_ui(ui, |ui| {
                    for &kind in all_agent_kinds() {
                        if let Some(agent) = agent_definition(kind) {
                            let installed = available(agent.default_command);
                            let label = format!(
                                "{} · {}",
                                agent.display_name,
                                if installed { "available" } else { "setup required" }
                            );
                            ui.selectable_value(&mut self.selected, Some(kind), label);
                        }
                    }
                });
        });
        let installed = self
            .selected
            .and_then(agent_definition)
            .is_some_and(|agent| available(agent.default_command));
        let (note, color) = if installed {
            (
                "The local agent starts when you open it. It may ask you to sign in; its diagnosis is separate from this transport demo.",
                theme::FG_SOFT(),
            )
        } else {
            (
                "Agent setup required on this desktop. Copy the context into a local agent instead; this private desktop does not inherit sign-in.",
                theme::PALETTE_YELLOW(),
            )
        };
        ui.label(RichText::new(note).size(12.5).color(color));
        egui::CollapsingHeader::new(RichText::new("Debug context").size(13.5).color(theme::FG())).show(ui, |ui| {
            widgets::well().show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("maintenance-debug-context")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&self.prompt)
                                .monospace()
                                .size(12.0)
                                .color(theme::FG_SOFT()),
                        );
                    });
            });
        });
        installed
    }
}

/// The diagnostic summary as labelled facts, or `None` while it is a status sentence.
fn facts(summary: &str) -> Option<Vec<(&'static str, String)>> {
    let value: serde_json::Value = serde_json::from_str(summary).ok()?;
    let object = value.as_object()?;
    let number = |key: &str| object.get(key).and_then(serde_json::Value::as_u64);
    let mut facts = Vec::new();
    if let Some(state) = object.get("agent_state").and_then(serde_json::Value::as_str) {
        let state = state.replace('_', " ");
        let mut letters = state.chars();
        let state = letters
            .next()
            .map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect());
        facts.push(("Agent", state));
    }
    let process = match (object.get("alive").and_then(serde_json::Value::as_bool), number("pid")) {
        (Some(true), Some(pid)) => format!("Running · pid {pid}"),
        (Some(true), None) => "Running".to_owned(),
        (Some(false), _) => "Not running".to_owned(),
        (None, _) => "Unknown".to_owned(),
    };
    facts.push(("Process", process));
    if let Some(age) = object.get("heartbeat_age_seconds").and_then(serde_json::Value::as_f64) {
        facts.push(("Heartbeat", horizon_core::maintenance::portfolio::ago(age)));
    }
    match (number("configured_revision"), number("applied_revision")) {
        (Some(saved), Some(applied)) if applied >= saved => {
            facts.push(("Instructions", format!("Revision {saved} applied")));
        }
        (Some(saved), applied) => facts.push((
            "Instructions",
            format!("Revision {saved} saved · {} applied", applied.unwrap_or(0)),
        )),
        (None, _) => {}
    }
    if let Some(count) = number("repository_count") {
        facts.push(("Repositories", count.to_string()));
    }
    if let (Some(queued), Some(completed)) = (number("queued_count"), number("completed_count")) {
        facts.push(("Pull requests", format!("{queued} queued · {completed} verified")));
    }
    Some(facts)
}

fn available(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| {
            let path = directory.join(command);
            let Ok(metadata) = path.metadata() else { return false };
            if !metadata.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
    })
}
