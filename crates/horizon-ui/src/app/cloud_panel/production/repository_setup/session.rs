//! The setup agent's session: a hidden panel the dialog starts, samples, writes to
//! and closes, and the commit of the files it wrote.
use super::prompt::prompt;
use super::run::{self, Action, Run};
use crate::app::cloud_panel::production::{HorizonApp, PanelKind, PanelOptions};
use crate::theme;
use egui::{Frame, RichText, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The least time an agent gets to start before its instructions are pasted.
const PROMPT_SETTLE: Duration = Duration::from_secs(3);
/// How long the agent's screen must stay still before the paste.
const PROMPT_QUIET: Duration = Duration::from_millis(1200);

/// A setup agent about to start: what it starts with, while the files outside
/// `.horizon` are read for its run.
pub(in crate::app::cloud_panel::production) struct Starting {
    preset: horizon_core::PresetConfig,
    args: Vec<String>,
    pending_prompt: Option<String>,
    repository: PathBuf,
    status: PathBuf,
    mode: super::Mode,
    chosen_mode: super::Mode,
    receiver: std::sync::mpsc::Receiver<(super::commit::Outside, bool)>,
}

impl HorizonApp {
    /// Starts the setup agent in a hidden panel; the dialog shows its progress and
    /// passes the person's messages to it.
    pub(in crate::app::cloud_panel::production) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        if cfg!(windows) {
            self.cloud_prototype.error = Some(super::WINDOWS_NOTE.into());
            return;
        }
        // One setup at a time. A run still working for another repository is not stopped
        // behind the person's back; a finished or stopped one is cleared.
        if let Some(run) = self.cloud_prototype.production.setup_run.as_ref() {
            let same = std::fs::canonicalize(&self.cloud_prototype.production.repository)
                .is_ok_and(|repository| repository == run.repository);
            // The setup choices say so and offer nothing to click; this guards the action.
            if !same && !run.finished() && !run.stopped {
                return;
            }
            self.cloud_repository_setup_action(ctx, Action::Stop);
        }
        let presets = super::setup_choices(&self.cloud_prototype.production);
        let preset = super::chosen_preset(&self.cloud_prototype.production, &presets)
            .cloned()
            // A machine account's agent without a preset of its own still starts plainly.
            .or_else(|| {
                self.cloud_prototype
                    .production
                    .setup_agent
                    .map(|kind| horizon_core::PresetConfig {
                        name: kind.display_name().to_owned(),
                        alias: None,
                        kind,
                        command: None,
                        args: Vec::new(),
                        resume: horizon_core::PanelResume::default(),
                        ssh_connection: None,
                    })
            });
        let Some(preset) = preset else {
            return;
        };
        let kind = preset.kind;
        let repository = match std::fs::canonicalize(&self.cloud_prototype.production.repository) {
            Ok(path) if path.is_dir() => path,
            _ => {
                self.cloud_prototype.error = Some("Choose an existing local repository directory".into());
                return;
            }
        };
        let status = match status_path() {
            Ok(status) => status,
            Err(error) => {
                self.cloud_prototype.error = Some(error);
                return;
            }
        };
        let chosen_mode = std::mem::take(&mut self.cloud_prototype.production.setup_mode);
        let mode = chosen_mode.clone().resolve(&repository);
        let instructions = prompt(&self.cloud_prototype.production.setup_agents, &status, &mode);
        // Claude and Codex take the instructions as their starting prompt; other agents get
        // them pasted once their session settles.
        let takes_prompt = matches!(kind, PanelKind::Claude | PanelKind::Codex);
        let mut args = preset.args.clone();
        let mut pending_prompt = None;
        if takes_prompt {
            args.push(instructions);
        } else {
            pending_prompt = Some(instructions);
        }
        // The files outside .horizon are read before the agent starts, so nothing it
        // changes becomes the baseline, and on a thread, since a repository with many
        // ignored files takes a while; the agent starts once they are read.
        if self.cloud_prototype.production.setup_starting.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let scan = repository.clone();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send((super::commit::outside(&scan), super::proof::uses_nuget(&scan)));
            wake.request_repaint();
        });
        self.cloud_prototype.production.setup_starting = Some(Starting {
            preset,
            args,
            pending_prompt,
            repository,
            status,
            mode,
            chosen_mode,
            receiver,
        });
        self.cloud_prototype.error = None;
    }

    /// Starts the agent once the files outside `.horizon` were read for its run.
    fn poll_cloud_setup_start(&mut self, ctx: &egui::Context) {
        let Some(starting) = self.cloud_prototype.production.setup_starting.as_ref() else {
            return;
        };
        let scan = match starting.receiver.try_recv() {
            Ok(scan) => scan,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(starting) = self.cloud_prototype.production.setup_starting.take() {
                    run::remove_run_folder(&starting.status);
                    self.cloud_prototype.production.setup_mode = starting.chosen_mode;
                }
                self.cloud_prototype.error = Some("Could not read the repository before the setup agent starts".into());
                return;
            }
        };
        if let Some(starting) = self.cloud_prototype.production.setup_starting.take() {
            self.launch_cloud_repository_setup(ctx, starting, scan);
        }
    }

    /// Creates the agent's hidden panel and the run that follows it.
    fn launch_cloud_repository_setup(
        &mut self,
        ctx: &egui::Context,
        starting: Starting,
        (outside, dotnet): (super::commit::Outside, bool),
    ) {
        let Starting {
            preset,
            args,
            pending_prompt,
            repository,
            status,
            mode,
            chosen_mode,
            receiver: _,
        } = starting;
        let kind = preset.kind;
        let workspace = self.board.ensure_local_workspace("Cloud setup");
        // Deliberately local: config preparation must not inherit a focused cloud.
        let result = self.board.create_panel(
            PanelOptions {
                name: Some(PANEL_NAME.into()),
                name_is_custom: Some(true),
                kind,
                command: preset.command.clone(),
                cwd: Some(repository.clone()),
                args,
                transcript_root: self.transcript_root.clone(),
                visible: false,
                rows: 48,
                cols: 160,
                ..PanelOptions::default()
            },
            workspace,
        );
        match result {
            Ok(panel) => {
                // Never saved, so a restart cannot run the setup again unseen.
                if let Some(created) = self.board.panel_mut(panel) {
                    created.transient = true;
                }
                let mut run = Run::new(panel, kind, repository, status, mode);
                run.agent_process = self.agent_process_of(panel);
                run.agent.clone_from(&preset.name);
                run.pending_prompt = pending_prompt;
                run.outside = outside;
                run.dotnet = dotnet;
                self.cloud_prototype.production.setup_run = Some(run);
                self.cloud_prototype.error = None;
                ctx.request_repaint();
            }
            Err(error) => {
                // A retry after fixing the agent keeps the Update or Fix it was asked for.
                self.cloud_prototype.production.setup_mode = chosen_mode;
                run::remove_run_folder(&status);
                self.cloud_prototype.error = Some(error.to_string());
            }
        }
    }

    /// Offers the agent presets of the New Workspace menu for the setup agent, read again
    /// each frame so a preset changed in Settings shows at once; a running setup keeps
    /// the preset it started with.
    pub(in crate::app::cloud_panel::production) fn offer_setup_presets(&mut self) {
        self.cloud_prototype.production.setup_presets = self
            .presets
            .iter()
            .filter(|preset| preset.kind.is_agent() && preset.ssh_connection.is_none())
            .cloned()
            .collect();
    }

    /// Samples the agent's screen, delivers a pending Enter, lists the files of a
    /// finished run and notices a setup panel that closed.
    pub(in crate::app::cloud_panel::production) fn sync_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        self.close_orphaned_setup_panels();
        self.poll_cloud_setup_start(ctx);
        // Use waits for the agent's processes on a thread; it goes on once they ended.
        if self.poll_cloud_setup_use(ctx) {
            return;
        }
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        run.poll();
        if run.finished() && run.files.is_none() {
            run.files = Some(changed_files(&run.repository));
        }
        // The agent asked for a (new) proof: Horizon builds and runs it itself.
        if let Some((request, spec)) = run.proof_wanted() {
            run.proved_request = request;
            run.proof_stale_reported = false;
            run.forget_inputs();
            // The finished card lists the files of the proof that passes, not an earlier one.
            run.files = None;
            let log = run.status_path.with_extension(format!("proof-{request}.log"));
            let id = super::proof::run_id(&run.status_path);
            // The proof it replaces stops first, so the two never build the same image.
            let after = run.proof.as_mut().and_then(super::proof::Proof::stop);
            run.proof = Some(super::proof::Proof::start(
                (&run.repository, &id),
                (request, spec),
                log,
                run.host_restore,
                (after, ctx.clone()),
            ));
        }
        let report = run.proof.as_mut().and_then(|proof| {
            proof.poll();
            if proof.running() {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            std::mem::take(&mut proof.unreported).then(|| proof.report())
        });
        let Some(panel) = self.board.panel_mut(run.panel) else {
            // The agent's panel closed. A setup that had not finished ends here: its
            // agent's processes end and its proof gives back its containers, image and
            // volume, as Stop does.
            if !run.finished() {
                if !run.stopped {
                    run.abandon();
                }
                run.stopped = true;
            }
            return;
        };
        if run.sampled.is_none_or(|at| at.elapsed() >= run::OUTPUT_EVERY) {
            // The unfair try-lock never waits on the PTY reader; a busy grid keeps the last rows.
            if let Some(text) = panel
                .terminal()
                .and_then(|terminal| terminal.try_last_lines_text(run::OUTPUT_ROWS))
            {
                run.output = run::readable_rows(&text);
                run.sampled = Some(Instant::now());
            }
            ctx.request_repaint_after(run::OUTPUT_EVERY);
        }
        // A summary without a proof request, or over a setup that changed after its proof,
        // gets one reminder: Use needs Horizon's proof of the files as they are.
        let report = report
            .or_else(|| run.take_summary_without_proof().then(|| MISSING_PROOF.to_owned()))
            .or_else(|| {
                (run.summary_over_stale_proof() && !std::mem::replace(&mut run.proof_stale_reported, true))
                    .then(|| STALE_PROOF.to_owned())
            });
        if let Some(report) = report {
            run::paste(panel, &mut run.submit_at, &report);
        }
        // An agent without a prompt argument gets the instructions once it has drawn its
        // screen and gone quiet.
        if run.pending_prompt.is_some()
            && run.started.elapsed() >= PROMPT_SETTLE
            && !panel.had_recent_output_within(PROMPT_QUIET)
            && let Some(instructions) = run.pending_prompt.take()
        {
            run::paste(panel, &mut run.submit_at, &instructions);
        }
        if let Some(at) = run.submit_at {
            let now = Instant::now();
            if now >= at {
                panel.write_input(b"\r");
                run.submit_at = None;
            } else {
                ctx.request_repaint_after(at - now);
            }
        }
    }

    /// The setup agent's own terminal, without panel chrome, in the dialog's right
    /// column. Clicking it gives it the keyboard, so the person talks to the agent here.
    pub(in crate::app::cloud_panel::production) fn show_cloud_setup_terminal(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
    ) {
        let Some((panel_id, agent)) = self
            .cloud_prototype
            .production
            .setup_run
            .as_ref()
            .map(|run| (run.panel, run.agent.clone()))
        else {
            return;
        };
        let events = ui.ctx().input(|input| input.events.clone());
        let events = crate::input::terminal_input_events(&events, Vec::new());
        Frame::new()
            .fill(theme::PANEL_BG())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(agent).size(14.0).strong().color(theme::FG()));
                    ui.label(
                        RichText::new("live · click to type to the agent")
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                    );
                });
                ui.add_space(6.0);
                let size = Vec2::new(ui.available_width(), (height - 44.0).max(120.0));
                let mut reconnect = false;
                if let Some(panel) = self.board.panel_mut(panel_id) {
                    ui.allocate_ui(size, |ui| {
                        ui.set_min_size(size);
                        crate::terminal_widget::TerminalView::new(panel, None).show(
                            ui,
                            false,
                            true,
                            &mut self.terminal_selection_drag,
                            crate::terminal_widget::TerminalKeyboardContext {
                                keyboard_events: &events,
                                primary_selection: &self.primary_selection,
                                local_ssh_reconnect_enabled: false,
                                reconnect_requested: &mut reconnect,
                            },
                        );
                    });
                }
            });
    }

    pub(in crate::app::cloud_panel::production) fn cloud_repository_setup_action(
        &mut self,
        ctx: &egui::Context,
        action: Action,
    ) {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        let panel = run.panel;
        match action {
            Action::Steer(text) => {
                if let Some(panel) = self.board.panel_mut(panel) {
                    run::paste(panel, &mut run.submit_at, &text);
                    ctx.request_repaint_after(run::SUBMIT_DELAY);
                }
            }
            Action::Show => {
                self.board.set_panel_visible(panel, true);
                let workspace = self.board.panel(panel).map(|panel| panel.workspace_id);
                self.close_cloud_creation();
                if let Some(workspace) = workspace {
                    self.reveal_new_panel(ctx, workspace, panel);
                }
            }
            Action::Stop => {
                if self.board.panel(panel).is_some() {
                    self.close_panel(panel);
                }
                if let Some(run) = self.cloud_prototype.production.setup_run.take() {
                    run.discard();
                }
            }
            Action::Use => self.use_cloud_repository_setup(ctx),
        }
    }
}

/// What the agent hears when it wrote a summary but no proof Horizon could run.
const MISSING_PROOF: &str = "Horizon has no proof to run: put a valid proof in the status file (a workflow with at least one job) and set proof_request to 1. Use this setup waits for Horizon's proof.";

/// What the agent hears when the setup changed after Horizon's proof passed.
const STALE_PROOF: &str = "The .horizon files or the proof changed after Horizon's proof passed, so it no longer proves this setup. Increase proof_request by one so Horizon proves it again.";

/// The name of a setup agent's hidden panel.
const PANEL_NAME: &str = "Cloud repository setup";

impl HorizonApp {
    /// The process the agent's panel started, which leads its own process group.
    fn agent_process_of(&self, panel: horizon_core::PanelId) -> Option<super::agent::AgentProcess> {
        self.board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .and_then(horizon_core::Terminal::child_pid)
            .map(super::agent::AgentProcess::of)
    }

    /// Closes hidden setup panels that no run follows. Setup panels are transient and
    /// never saved; this closes one an older Horizon saved before that.
    fn close_orphaned_setup_panels(&mut self) {
        let current = self.cloud_prototype.production.setup_run.as_ref().map(|run| run.panel);
        let orphans: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.transient && panel.title == PANEL_NAME && Some(panel.id) != current)
            .map(|panel| panel.id)
            .collect();
        for panel in orphans {
            self.close_panel(panel);
        }
    }
}

/// The run's status file in a new folder that only this user may read, with a random
/// name made atomically: the proof logs beside it can hold a private repository's output.
fn status_path() -> Result<PathBuf, String> {
    let folder = tempfile::Builder::new()
        .prefix("horizon-cloud-setup-")
        .tempdir()
        .map_err(|error| format!("Could not make a private folder for the setup: {error}"))?;
    Ok(folder.keep().join("status.json"))
}

/// The `.horizon` files the agent changed, added or removed, as the finished card lists them.
fn changed_files(repository: &Path) -> String {
    super::commit::changes(repository).join(" · ")
}
