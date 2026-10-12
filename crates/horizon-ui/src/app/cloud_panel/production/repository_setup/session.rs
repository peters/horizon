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

/// A setup agent about to start: what it starts with, while a thread prepares its run.
pub(in crate::app::cloud_panel::production) struct Starting {
    preset: horizon_core::PresetConfig,
    chosen_mode: super::Mode,
    /// The repository as the person entered it.
    entered: String,
    receiver: std::sync::mpsc::Receiver<Result<Prepared, String>>,
}

/// What the thread that prepares a run read and made: the repository, its folder's
/// identity and the files outside `.horizon` before the agent starts, the run's status
/// file, the agent's instructions and the scope it starts in.
struct Prepared {
    repository: PathBuf,
    identity: super::untrusted::Identity,
    status: PathBuf,
    mode: super::Mode,
    instructions: String,
    unit: String,
    outside: super::commit::Outside,
    dotnet: bool,
    launcher: Option<Vec<String>>,
}

impl HorizonApp {
    /// Starts the setup agent in a hidden panel; the dialog shows its progress and
    /// passes the person's messages to it.
    pub(in crate::app::cloud_panel::production) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        if !cfg!(target_os = "linux") {
            self.cloud_prototype.error = Some(super::PLATFORM_NOTE.into());
            return;
        }
        // One setup at a time. A run still working for another repository is not stopped
        // behind the person's back; a finished or stopped one is cleared.
        let entered = self.cloud_prototype.production.repository.trim().to_owned();
        if let Some(run) = self.cloud_prototype.production.setup_run.as_ref() {
            let same = run.entered == entered || run.repository == Path::new(&entered);
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
                // The kind the button names, Claude when none was chosen.
                Some(self.cloud_prototype.production.setup_agent.unwrap_or(PanelKind::Claude)).map(|kind| {
                    horizon_core::PresetConfig {
                        name: kind.display_name().to_owned(),
                        alias: None,
                        kind,
                        command: None,
                        args: Vec::new(),
                        resume: horizon_core::PanelResume::default(),
                        ssh_connection: None,
                    }
                })
            });
        let Some(preset) = preset else {
            return;
        };
        if self.cloud_prototype.production.setup_starting.is_some() {
            return;
        }
        let chosen_mode = std::mem::take(&mut self.cloud_prototype.production.setup_mode);
        let agents = self.cloud_prototype.production.setup_agents.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let (folder, mode) = (entered.clone(), chosen_mode.clone());
        // Everything that reads or writes the disk happens on this thread: the repository's
        // folder, the run's status folder, and the files outside .horizon, read before the
        // agent starts so nothing it changes becomes the baseline; the agent starts once
        // they are read.
        std::thread::spawn(move || {
            let _ = sender.send(prepare(&folder, mode, &agents));
            wake.request_repaint();
        });
        self.cloud_prototype.production.setup_starting = Some(Starting {
            preset,
            chosen_mode,
            entered,
            receiver,
        });
        self.cloud_prototype.error = None;
    }

    /// Starts the agent once the files outside `.horizon` were read for its run.
    fn poll_cloud_setup_start(&mut self, ctx: &egui::Context) {
        let Some(starting) = self.cloud_prototype.production.setup_starting.as_ref() else {
            return;
        };
        let prepared = match starting.receiver.try_recv() {
            Ok(prepared) => prepared,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Could not read the repository before the setup agent starts".to_owned())
            }
        };
        let Some(starting) = self.cloud_prototype.production.setup_starting.take() else {
            return;
        };
        match prepared {
            Ok(prepared) => self.launch_cloud_repository_setup(ctx, starting, prepared),
            Err(error) => {
                self.cloud_prototype.production.setup_mode = starting.chosen_mode;
                self.cloud_prototype.error = Some(error);
            }
        }
    }

    /// Creates the agent's hidden panel and the run that follows it.
    fn launch_cloud_repository_setup(&mut self, ctx: &egui::Context, starting: Starting, prepared: Prepared) {
        let Starting {
            preset,
            chosen_mode,
            entered,
            receiver: _,
        } = starting;
        let Prepared {
            repository,
            identity,
            status,
            mode,
            instructions,
            unit,
            outside,
            dotnet,
            launcher,
        } = prepared;
        let kind = preset.kind;
        // Claude and Codex take the instructions as their starting prompt; other agents get
        // them pasted once their session settles.
        let mut args = preset.args.clone();
        let mut pending_prompt = None;
        if matches!(kind, PanelKind::Claude | PanelKind::Codex) {
            args.push(instructions);
        } else {
            pending_prompt = Some(instructions);
        }
        // An agent starts only inside a scope that holds all its descendants: without
        // one, a process it started could leave its group and keep writing while Use
        // reads and commits.
        let Some(launcher) = launcher.or_else(|| (!kind.is_agent()).then(Vec::new)) else {
            self.cloud_prototype.production.setup_mode = chosen_mode;
            run::remove_run_folder(&status);
            self.cloud_prototype.error = Some(NO_SCOPE.into());
            return;
        };
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
                // Started inside a scope of its own, which holds all its descendants.
                launch_wrapper: launcher,
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
                run.identity = identity;
                run.entered = entered;
                run.agent_process = self.agent_process_of(panel, &unit);
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
        self.close_orphaned_setup_panels(ctx);
        self.poll_cloud_setup_start(ctx);
        // Use waits for the agent's processes on a thread; it goes on once they ended.
        if self.poll_cloud_setup_use(ctx) {
            return;
        }
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        run.poll(ctx);
        // The finished card's list of changed files, made off the dialog's thread.
        if run.finished() && run.files.is_none() {
            if let Some(job) = &run.files_job {
                if let Ok(files) = job.try_recv() {
                    run.files = Some(files);
                    run.files_job = None;
                }
            } else {
                let (sender, receiver) = std::sync::mpsc::channel();
                let repository = run.repository.clone();
                let wake = ctx.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(changed_files(&repository));
                    wake.request_repaint();
                });
                run.files_job = Some(receiver);
            }
        }
        // The agent asked for a (new) proof: Horizon builds and runs it itself.
        if let Some((request, spec)) = run.proof_wanted() {
            run.proved_request = request;
            run.proof_stale_reported = false;
            run.forget_inputs();
            // The finished card lists the files of the proof that passes, not an earlier one.
            run.files = None;
            run.files_job = None;
            let log = run.status_path.with_extension(format!("proof-{request}.log"));
            let id = super::proof::run_id(&run.status_path);
            // The proof it replaces stops first, so the two never build the same image.
            let after = run.proof.as_mut().and_then(super::proof::Proof::stop);
            run.proof = Some(super::proof::Proof::start(
                (&run.repository, &id, run.identity),
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
            // Use cannot be cancelled once it started: Stop waits until it reported.
            Action::Stop
                if self
                    .cloud_prototype
                    .production
                    .setup_run
                    .as_ref()
                    .is_some_and(Run::ending) => {}
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

/// Why the setup agent did not start on a Linux computer without a systemd user manager.
const NO_SCOPE: &str = "The setup agent needs a systemd user session (systemd-run --user), so Horizon can end everything it starts before it commits. Start one, or use Quick start or write .horizon yourself.";

/// The longest Horizon's exit waits for setup runs to end; the window does not draw meanwhile.
const EXIT_WAIT: Duration = Duration::from_secs(20);

/// The name of a setup agent's hidden panel.
const PANEL_NAME: &str = "Cloud repository setup";

impl HorizonApp {
    /// Ends the setup run, and a setup about to start, when the board they belong to
    /// goes: a session switch restarts panel ids, so a kept id could name another panel.
    pub(in crate::app) fn end_cloud_repository_setup(&mut self) {
        if let Some(run) = self.cloud_prototype.production.setup_run.take() {
            run.discard();
        }
        if let Some(starting) = self.cloud_prototype.production.setup_starting.take() {
            // The thread preparing it may still make the run's status folder, which goes
            // once it reports. Not one of the endings a new setup waits for, since that
            // thread waits for those itself.
            std::thread::spawn(move || {
                if let Ok(Ok(prepared)) = starting.receiver.recv() {
                    run::remove_run_folder_now(&prepared.status);
                }
            });
        }
    }

    /// Ends the setup run as Horizon exits, and waits up to [`EXIT_WAIT`] until its agent
    /// and proof have ended and what they made is removed, with every run already
    /// ending. Whatever is not shown removed by then, the run's containers, images and
    /// volume among them, is kept for Horizon's next run to remove; an agent process that
    /// outlived its kills is logged and goes when the system ends it.
    pub(in crate::app) fn end_cloud_repository_setup_for_exit(&mut self) {
        self.end_cloud_repository_setup();
        super::proof::retry_unremoved();
        if !run::wait_for_ended_runs_within(Some(EXIT_WAIT)) {
            tracing::warn!(
                "Horizon exits while a setup run still ends; its Docker resources are kept for the next start"
            );
        }
        super::proof::keep_unremoved();
    }

    /// The process the agent's panel started, which leads its own process group.
    fn agent_process_of(&self, panel: horizon_core::PanelId, unit: &str) -> Option<super::agent::AgentProcess> {
        let id = self
            .board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .and_then(horizon_core::Terminal::child_pid)?;
        Some(super::agent::AgentProcess::in_scope(id, unit))
    }

    /// Closes hidden setup panels that no run follows. Setup panels are transient and
    /// never saved; this closes one an older Horizon saved before that.
    /// It looks only when the panels or the run changed since it last looked, so a frame
    /// without such a change costs nothing: a hidden setup panel appears only as panels
    /// are added or a session's board loads, and stops being the run's only as the run
    /// ends.
    fn close_orphaned_setup_panels(&mut self, ctx: &egui::Context) {
        let current = self.cloud_prototype.production.setup_run.as_ref().map(|run| run.panel);
        let looked = egui::Id::new("cloud-setup-orphan-scan");
        let now = (
            self.board.panels.len(),
            self.board.panels.last().map(|panel| panel.id),
            current,
        );
        if ctx.data(|data| data.get_temp(looked)) == Some(now) {
            return;
        }
        ctx.data_mut(|data| data.insert_temp(looked, now));
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

/// Prepares a run on a thread, for the repository the person entered as `folder`: its
/// folder must exist, the run gets its status folder, and the files outside `.horizon` are
/// read once any earlier run has ended.
fn prepare(
    folder: &str,
    chosen_mode: super::Mode,
    agents: &[horizon_core::cloud_runtime::setup::Agent],
) -> Result<Prepared, String> {
    let repository = std::fs::canonicalize(folder)
        .ok()
        .filter(|path| path.is_dir())
        .ok_or("Choose an existing local repository directory")?;
    let status = status_path()?;
    let mode = chosen_mode.resolve(&repository);
    let instructions = prompt(agents, (&repository, &status), &mode);
    let unit = format!(
        "horizon-setup-agent-{}-{}.scope",
        super::proof::run_id(&status),
        std::process::id()
    );
    // One setup at a time: an earlier run still ending, as after Stop, ends first, with
    // what an earlier cleanup could not remove.
    super::proof::retry_unremoved();
    run::wait_for_ended_runs();
    Ok(Prepared {
        identity: super::untrusted::identity(&repository),
        outside: super::commit::outside(&repository),
        dotnet: super::proof::uses_nuget(&repository),
        launcher: super::agent::scope_launcher(&unit),
        repository,
        status,
        mode,
        instructions,
        unit,
    })
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
    files_summary(&super::commit::changes(repository))
}

/// The most changed files the finished card names, and the most characters of a name.
const SHOWN_FILES: usize = 12;
const SHOWN_NAME: usize = 120;

/// The finished card's short list: the first few names, each cut short, and how many
/// more there are, so a setup with thousands of long names cannot hold the dialog.
fn files_summary(changes: &[String]) -> String {
    let mut shown: Vec<String> = changes
        .iter()
        .take(SHOWN_FILES)
        .map(|name| {
            if name.chars().count() > SHOWN_NAME {
                format!("{}…", name.chars().take(SHOWN_NAME).collect::<String>())
            } else {
                name.clone()
            }
        })
        .collect();
    if changes.len() > SHOWN_FILES {
        shown.push(format!("and {} more", changes.len() - SHOWN_FILES));
    }
    shown.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finished_card_names_a_few_files_each_cut_short() {
        let few = vec![".horizon/Dockerfile".to_owned(), ".horizon/cloud.yml".to_owned()];
        assert_eq!(files_summary(&few), ".horizon/Dockerfile · .horizon/cloud.yml");
        let many: Vec<String> = (0..5000)
            .map(|number| format!(".horizon/{}{number}", "x".repeat(500)))
            .collect();
        let summary = files_summary(&many);
        assert!(summary.ends_with("and 4988 more"), "{summary}");
        assert!(summary.chars().count() < SHOWN_FILES * (SHOWN_NAME + 4) + 20);
    }
}
