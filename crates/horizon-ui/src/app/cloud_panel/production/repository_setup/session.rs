//! The setup agent's session: a hidden panel the dialog starts, samples, writes to
//! and closes, and the commit of the files it wrote.
use super::prompt::prompt;
use super::run::{self, Action, Run};
use super::set_configuration;
use crate::app::cloud_panel::production::{HorizonApp, PanelKind, PanelOptions};
use crate::theme;
use egui::{Frame, RichText, Stroke, Vec2};
use horizon_core::cloud_runtime::repository::launch::Configuration;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The least time an agent gets to start before its instructions are pasted.
const PROMPT_SETTLE: Duration = Duration::from_secs(3);
/// How long the agent's screen must stay still before the paste.
const PROMPT_QUIET: Duration = Duration::from_millis(1200);

impl HorizonApp {
    /// Starts the setup agent in a hidden panel; the dialog shows its progress and
    /// passes the person's messages to it.
    pub(in crate::app::cloud_panel::production) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
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
        let status = status_path();
        let mode = std::mem::take(&mut self.cloud_prototype.production.setup_mode).resolve(&repository);
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
                let mut run = Run::new(panel, kind, repository, status, mode);
                run.agent.clone_from(&preset.name);
                run.pending_prompt = pending_prompt;
                self.cloud_prototype.production.setup_run = Some(run);
                self.cloud_prototype.error = None;
                ctx.request_repaint();
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
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
            let log = run.status_path.with_extension(format!("proof-{request}.log"));
            run.proof = Some(super::proof::Proof::start(
                &run.repository,
                (request, spec),
                log,
                run.host_restore,
                ctx.clone(),
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
            run.stopped = !run.finished();
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
                let path = run.status_path.clone();
                self.cloud_prototype.production.setup_run = None;
                if self.board.panel(panel).is_some() {
                    self.close_panel(panel);
                }
                remove_run_folder(&path);
            }
            Action::Use => self.use_cloud_repository_setup(ctx),
        }
    }

    /// Commits the files the agent wrote and reads them as the repository's settings.
    /// The agent stops first, so nothing writes while the files are checked against the
    /// proof once more and committed; files that changed after the proof are refused.
    fn use_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(panel) = self.cloud_prototype.production.setup_run.as_ref().map(|run| run.panel) else {
            return;
        };
        if self.board.panel(panel).is_some() {
            self.close_panel(panel);
        }
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        run.refresh_inputs_now();
        if !run.finished() {
            run.stopped = true;
            self.cloud_prototype.error = Some(
                "The .horizon files changed after Horizon's proof, so they are not committed. Start the setup again to prove them."
                    .into(),
            );
            return;
        }
        let run = &*run;
        let repository = run.repository.clone();
        let message = run.mode.commit_message();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(args)
                .output()
                .map_err(|error| error.to_string())
                .and_then(|output| {
                    if output.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
                    }
                })
        };
        // An update that changed nothing has nothing to commit; the committed setup is read.
        let unchanged = || {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["diff", "--cached", "--quiet", "--", ".horizon"])
                .status()
                .is_ok_and(|status| status.success())
        };
        let committed = git(&["add", "--", ".horizon"]).and_then(|()| {
            if unchanged() {
                Ok(())
            } else {
                git(&["commit", "--quiet", "-m", message, "--", ".horizon"])
            }
        });
        if let Err(error) = committed {
            self.cloud_prototype.error = Some(format!("Could not commit .horizon: {error}"));
            return;
        }
        if let Some(run) = self.cloud_prototype.production.setup_run.take() {
            remove_run_folder(&run.status_path);
        }
        set_configuration(&mut self.cloud_prototype.production, Configuration::Committed);
        self.read_cloud_profiles(ctx);
    }
}

/// What the agent hears when it wrote a summary but no proof Horizon could run.
const MISSING_PROOF: &str = "Horizon has no proof to run: put a valid proof in the status file (a workflow with at least one job, or commands) and set proof_request to 1. Use this setup waits for Horizon's proof.";

/// What the agent hears when the setup changed after Horizon's proof passed.
const STALE_PROOF: &str = "The .horizon files or the proof changed after Horizon's proof passed, so it no longer proves this setup. Increase proof_request by one so Horizon proves it again.";

/// The name of a setup agent's hidden panel.
const PANEL_NAME: &str = "Cloud repository setup";

impl HorizonApp {
    /// Closes hidden setup panels that no run follows, such as one restored after a
    /// restart: its saved arguments hold the prompt, so it would set up again unseen.
    fn close_orphaned_setup_panels(&mut self) {
        let current = self.cloud_prototype.production.setup_run.as_ref().map(|run| run.panel);
        let orphans: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| !panel.visible && panel.title == PANEL_NAME && Some(panel.id) != current)
            .map(|panel| panel.id)
            .collect();
        for panel in orphans {
            self.close_panel(panel);
        }
    }
}

/// A private file for one run's status, outside the repository.
/// The run's status file in a folder of its own that only this user may read: the
/// proof logs and screenshots beside it can hold a private repository's output.
fn status_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let folder = std::env::temp_dir().join(format!("horizon-cloud-setup-{}-{stamp}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    let _ = builder.recursive(true).create(&folder);
    folder.join("status.json")
}

/// Removes a finished run's folder: its status, proof logs and screenshots.
fn remove_run_folder(status: &Path) {
    if let Some(folder) = status.parent().filter(|folder| {
        folder
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("horizon-cloud-setup-"))
    }) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// The `.horizon` files the agent changed or added, as the finished card lists them.
fn changed_files(repository: &Path) -> String {
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["status", "--porcelain", "--untracked-files=all", "--", ".horizon"])
        .output()
    else {
        return String::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (state, path) = line.split_at_checked(3)?;
            let name = path.trim().trim_start_matches(".horizon/");
            Some(if state.trim() == "??" {
                format!("{name} new")
            } else {
                format!("{name} changed")
            })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
