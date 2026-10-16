//! The setup agent's session: a hidden panel the dialog starts, samples, writes to
//! and closes, and the commit of the files it wrote.
use super::access::{self, Fixer};
use super::build_on::Submitted;
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
    pub(super) entered: String,
    /// The Docker host its proof builds on; none for this computer.
    build_on: Option<super::build_on::Target>,
    receiver: std::sync::mpsc::Receiver<Result<Prepared, String>>,
}

/// What the thread that prepares a run read and made: the repository, its folder's
/// identity and the files outside `.horizon` before the agent starts, the run's status
/// file, the agent's instructions, the scope it starts in and the Rust git repositories
/// the clone box may name.
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
    git_crates: Vec<String>,
    /// How many endings of runs of the repository had begun before the preparation
    /// waited for them.
    endings: u64,
}

impl HorizonApp {
    /// Starts the setup agent in a hidden panel; the dialog shows its progress and
    /// passes the person's messages to it.
    pub(in crate::app::cloud_panel::production) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        if !cfg!(target_os = "linux") {
            self.cloud_prototype.error = Some(super::PLATFORM_NOTE.into());
            return;
        }
        // One setup for each folder, and runs of other repositories go on as they are. An
        // earlier run of this repository is cleared, unless Use is committing it.
        let entered = self.cloud_prototype.production.repository.trim().to_owned();
        if self.cloud_prototype.production.starting_here() {
            return;
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
        let mode = self.cloud_prototype.production.setup_mode.clone();
        let submitted = Submitted { entered, preset, mode };
        // A host chosen in an earlier opening of the dialog waits for this opening's read.
        if let Some(submitted) = self.cloud_prototype.production.setup_build_on.defer_start(submitted) {
            self.start_submitted_setup(ctx, submitted);
        }
    }

    /// Starts the setup a Start submitted, now or after it waited for the read of the
    /// Docker hosts, on the repository it names whatever the dialog shows by then.
    pub(in crate::app::cloud_panel::production) fn start_submitted_setup(
        &mut self,
        ctx: &egui::Context,
        submitted: Submitted,
    ) {
        let Submitted { entered, preset, mode } = submitted;
        let form = &self.cloud_prototype.production;
        if form.setup_starting.iter().any(|starting| starting.entered == entered) {
            return;
        }
        if let Some(run) = form.setup_runs.iter().find(|run| super::same_folder(run, &entered)) {
            if run.ending() {
                return;
            }
            let panel = run.panel;
            self.cloud_repository_setup_action(ctx, panel, Action::Stop);
        }
        self.cloud_prototype.production.setup_mode = super::Mode::default();
        let build_on = self.cloud_prototype.production.setup_build_on.chosen();
        self.prepare_cloud_setup(ctx, (preset, build_on), mode, entered);
        self.cloud_prototype.error = None;
    }

    /// Prepares a setup of `entered` with `preset` on a thread; it starts once prepared.
    fn prepare_cloud_setup(
        &mut self,
        ctx: &egui::Context,
        (preset, build_on): (horizon_core::PresetConfig, Option<super::build_on::Target>),
        chosen_mode: super::Mode,
        entered: String,
    ) {
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
        self.cloud_prototype.production.setup_starting.push(Starting {
            preset,
            chosen_mode,
            entered,
            build_on,
            receiver,
        });
    }

    /// Starts each agent once the files outside `.horizon` were read for its run. A setup
    /// that prepares again starts at the earliest on the next frame.
    fn poll_cloud_setup_start(&mut self, ctx: &egui::Context) {
        for starting in std::mem::take(&mut self.cloud_prototype.production.setup_starting) {
            let prepared = match starting.receiver.try_recv() {
                Ok(prepared) => prepared,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.cloud_prototype.production.setup_starting.push(starting);
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("Could not read the repository before the setup agent starts".to_owned())
                }
            };
            match prepared {
                Ok(prepared) => self.launch_cloud_repository_setup(ctx, starting, prepared),
                Err(error) => {
                    self.cloud_prototype.production.setup_mode = starting.chosen_mode;
                    self.cloud_prototype.error = Some(error);
                }
            }
        }
    }

    /// Creates the agent's hidden panel and the run that follows it.
    fn launch_cloud_repository_setup(&mut self, ctx: &egui::Context, starting: Starting, prepared: Prepared) {
        let Starting {
            preset,
            chosen_mode,
            entered,
            build_on,
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
            git_crates: crates,
            endings,
        } = prepared;
        // Two entries of one folder, such as a link and its target, still get one run.
        if let Some(other) = self
            .cloud_prototype
            .production
            .setup_runs
            .iter()
            .find(|run| run.repository == repository)
        {
            self.cloud_prototype.error = Some(format!(
                "A setup of {} already runs. Open New cloud for {} to see it or to stop it.",
                repository.display(),
                other.entered
            ));
            self.cloud_prototype.production.setup_mode = chosen_mode;
            run::remove_run_folder(&status);
            return;
        }
        // An earlier run of this folder began to end after the preparation looked, as when
        // it was stopped meanwhile: its agent may have written here while the baseline was
        // read, or still writes. The setup prepares again, so it waits for that ending and
        // reads its baseline and mode after it.
        if run::endings_begun(&repository) != endings {
            run::remove_run_folder(&status);
            self.prepare_cloud_setup(ctx, (preset, build_on), chosen_mode, entered);
            return;
        }
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
        let workspace = self.board.ensure_local_workspace(WORKSPACE);
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
                run.crates.locked = crates;
                run.agent.clone_from(&preset.name);
                run.pending_prompt = pending_prompt;
                run.outside = outside;
                run.dotnet = dotnet;
                run.build_on = build_on;
                self.cloud_prototype.production.setup_runs.push(run);
                self.cloud_prototype.error = None;
                // Every setup shows on the setup board; it opens with the first one.
                self.open_setup_board(ctx, false);
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

    /// Syncs every setup run, starts the setups whose repository was read, and closes
    /// setup panels that no run follows.
    pub(in crate::app::cloud_panel::production) fn sync_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        self.close_orphaned_setup_panels(ctx);
        self.poll_cloud_setup_start(ctx);
        let root = self.cloud_prototype.root.clone();
        let production = &mut self.cloud_prototype.production;
        let wanted = production.setup_runs.iter().any(Run::wants_rented_builder);
        production.setup_builder.tick(wanted, root.as_deref(), ctx);
        for run in self.cloud_prototype.production.setup_run_panels() {
            self.sync_cloud_setup_run(ctx, run);
        }
        self.sync_setup_boards();
    }

    /// Samples the agent's screen, delivers a pending Enter, lists the files of a
    /// finished run and notices a setup panel that closed, for the run of `key`.
    fn sync_cloud_setup_run(&mut self, ctx: &egui::Context, key: horizon_core::PanelId) {
        // Use waits for the agent's processes on a thread; it goes on once they ended.
        if self.poll_cloud_setup_use(ctx, key) {
            return;
        }
        let builder = &self.cloud_prototype.production.setup_builder;
        let (rented_host, builder_wait) = (builder.host(), builder.waiting());
        let Some(run) = self.cloud_prototype.production.setup_run_mut(key) else {
            return;
        };
        run.poll(ctx);
        list_changed_files(run, ctx);
        // The agent asked for a (new) proof: Horizon builds and runs it itself, on the
        // rented builder once it answers.
        let place = run.proof_place((rented_host, builder_wait));
        if let (Some((request, spec)), run::board::ProofPlace::Ready(build_on)) = (run.proof_wanted(), place) {
            run.proved_request = request;
            run.proof_stale_reported = false;
            run.forget_inputs();
            // The finished card lists the files of the proof that passes, not an earlier one.
            run.files = None;
            run.files_job = None;
            run.fixer = None;
            let log = run.status_path.with_extension(format!("proof-{request}.log"));
            let (id, access) = (super::proof::run_id(&run.status_path), run.crate_access());
            // The proof and the preview it replaces stop first: the two never build the same
            // image, and no preview runs on the volume the new proof uses. The preview
            // starts again once the new proof passed.
            let after = run.stop_all();
            run.previewed_request = 0;
            run.proof = Some(super::proof::Proof::start(
                (&run.repository, &id, run.identity),
                build_on,
                (request, spec),
                log,
                (run.host_restore, run.root_dirty.clone(), access),
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
        let preview_report = run.sync_preview(ctx);
        let Some(panel) = self.board.panel_mut(run.panel) else {
            run.agent_panel_closed();
            self.close_cloud_setup_fix_panel(key);
            // A live preview still opens, for example after a refused Use closed the agent.
            self.open_cloud_setup_preview(key);
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
            .or(preview_report)
            .or_else(|| run.take_summary_without_proof().then(|| MISSING_PROOF.to_owned()))
            .or_else(|| {
                run.take_summary_with_invalid_preview()
                    .then(|| INVALID_PREVIEW.to_owned())
            })
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
        self.open_cloud_setup_preview(key);
    }

    /// Opens each live preview of the run of `key` once, as a Device panel beside its
    /// setup agent. A panel that shows a desktop which ended, or was replaced, closes.
    fn open_cloud_setup_preview(&mut self, key: horizon_core::PanelId) {
        let Some(run) = self.cloud_prototype.production.setup_run_mut(key) else {
            return;
        };
        // A panel the person closed stays closed for this desktop.
        if run.preview_panel.is_some_and(|panel| self.board.panel(panel).is_none()) {
            run.preview_panel = None;
        }
        let endpoint = run.preview.as_ref().and_then(super::preview::Preview::endpoint);
        if run.preview_shown == endpoint {
            return;
        }
        if let Some(old) = run.preview_panel.take()
            && self.board.panel(old).is_some()
        {
            self.close_panel(old);
        }
        // The desktop counts as shown once its panel exists, so a failed creation is
        // tried again on the next frame.
        let Some(endpoint) = endpoint else {
            if let Some(run) = self.cloud_prototype.production.setup_run_mut(key) {
                run.preview_shown = None;
            }
            return;
        };
        let Some(run) = self.cloud_prototype.production.setup_run(key) else {
            return;
        };
        // Beside the agent, or where the person works when the agent is gone.
        let Some(workspace) = self
            .board
            .panel(run.panel)
            .map(|panel| panel.workspace_id)
            .or(self.board.active_workspace)
        else {
            return;
        };
        let name = run
            .repository
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let created = self.board.create_panel(
            PanelOptions {
                name: Some(format!("{name} preview")),
                name_is_custom: Some(true),
                kind: PanelKind::Device,
                command: Some(endpoint.clone()),
                // Large enough to see the app beside the connection details.
                size: Some(PREVIEW_PANEL_SIZE),
                ..PanelOptions::default()
            },
            workspace,
        );
        if let Ok(panel) = created
            && let Some(run) = self.cloud_prototype.production.setup_run_mut(key)
        {
            // Never saved: after a restart its desktop is gone.
            if let Some(created) = self.board.panel_mut(panel) {
                created.transient = true;
            }
            run.preview_panel = Some(panel);
            run.preview_shown = Some(endpoint);
        }
    }

    /// The setup agent's own terminal, without panel chrome, in the dialog's right
    /// column, for the repository the dialog shows. Clicking it gives it the keyboard, so
    /// the person talks to the agent here.
    pub(in crate::app::cloud_panel::production) fn show_cloud_setup_terminal(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
    ) {
        let Some((panel_id, agent)) = self
            .cloud_prototype
            .production
            .run_here()
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

    /// Does `action` for the run whose agent started in `key`.
    pub(in crate::app::cloud_panel::production) fn cloud_repository_setup_action(
        &mut self,
        ctx: &egui::Context,
        key: horizon_core::PanelId,
        action: Action,
    ) {
        let Some(run) = self.cloud_prototype.production.setup_run_mut(key) else {
            return;
        };
        let panel = run.panel;
        run.notice = None;
        match action {
            Action::Steer(text) => self.steer_cloud_setup(ctx, key, &text),
            Action::Fix(fixer) => self.start_cloud_setup_fix(ctx, key, fixer),
            Action::ShowFix => {
                if let Some(browser) = run.fix_panel.filter(|browser| self.board.panel(*browser).is_some()) {
                    self.reveal_cloud_setup_panel(ctx, browser);
                } else {
                    // The person closed it: open it again for whoever makes the fix, and show it.
                    let fixer = run.fixer.unwrap_or(Fixer::Person);
                    self.start_cloud_setup_fix(ctx, key, fixer);
                    if fixer == Fixer::Agent
                        && let Some(browser) = self
                            .cloud_prototype
                            .production
                            .setup_run(key)
                            .and_then(|run| run.fix_panel)
                    {
                        self.reveal_cloud_setup_panel(ctx, browser);
                    }
                }
            }
            Action::ProveAgain => self.steer_cloud_setup(ctx, key, access::FIXED),
            Action::Show => {
                self.board.set_panel_visible(panel, true);
                let workspace = self.board.panel(panel).map(|panel| panel.workspace_id);
                self.close_cloud_creation();
                if let Some(workspace) = workspace {
                    self.reveal_new_panel(ctx, workspace, panel);
                }
            }
            // Use cannot be cancelled once it started: Stop waits until it reported.
            Action::Stop if run.ending() => {}
            Action::ShowPreview => {
                if let Some(preview) = run.preview_panel {
                    self.reveal_cloud_setup_panel(ctx, preview);
                }
            }
            Action::Stop => {
                for helper in [run.preview_panel.take(), run.fix_panel.take()].into_iter().flatten() {
                    if self.board.panel(helper).is_some() {
                        self.close_panel(helper);
                    }
                }
                if self.board.panel(panel).is_some() {
                    self.close_panel(panel);
                }
                if let Some(run) = self.cloud_prototype.production.take_setup_run(key) {
                    run.discard();
                }
            }
            Action::Use => self.use_cloud_repository_setup(ctx, key),
        }
    }

    /// Closes the GitHub access panel of the run of `key`, which served that run only.
    fn close_cloud_setup_fix_panel(&mut self, key: horizon_core::PanelId) {
        let Some(browser) = self
            .cloud_prototype
            .production
            .setup_run_mut(key)
            .and_then(|run| run.fix_panel.take())
        else {
            return;
        };
        if self.board.panel(browser).is_some() {
            self.close_panel(browser);
        }
    }

    /// Passes `text` to the agent of the run of `key` as one message.
    fn steer_cloud_setup(&mut self, ctx: &egui::Context, key: horizon_core::PanelId, text: &str) {
        let Some(run) = self.cloud_prototype.production.setup_run_mut(key) else {
            return;
        };
        if let Some(panel) = self.board.panel_mut(run.panel) {
            run::paste(panel, &mut run.submit_at, text);
            ctx.request_repaint_after(run::SUBMIT_DELAY);
        }
    }

    /// Closes the dialog and shows a panel the setup opened beside the agent.
    fn reveal_cloud_setup_panel(&mut self, ctx: &egui::Context, panel: horizon_core::PanelId) {
        let workspace = self.board.panel(panel).map(|panel| panel.workspace_id);
        self.close_cloud_creation();
        if let Some(workspace) = workspace {
            self.reveal_new_panel(ctx, workspace, panel);
        }
    }

    /// Opens the page of the fix the proof of the run of `key` needs in a browser panel
    /// beside its agent. The agent works in it while the dialog shows its progress, or
    /// the person does.
    fn start_cloud_setup_fix(&mut self, ctx: &egui::Context, key: horizon_core::PanelId, fixer: Fixer) {
        let Some(run) = self.cloud_prototype.production.setup_run(key) else {
            return;
        };
        let Some(fix) = run.proof.as_ref().and_then(|proof| proof.fix.clone()) else {
            return;
        };
        let Some(workspace) = self.board.panel(run.panel).map(|panel| panel.workspace_id) else {
            return;
        };
        // Each fix starts on its own page: an earlier fix may have left the panel on
        // another organization's policy page. The open panel goes back to it, keeping its
        // browser profile and so the GitHub sign-in; a panel the person closed is opened
        // again.
        let open = run.fix_panel.filter(|panel| self.board.panel(*panel).is_some());
        let browser = if let Some(panel) = open {
            if let Some(browser) = self.board.panel_mut(panel).and_then(|panel| panel.browser_mut()) {
                browser.submit_navigation(fix.url());
            }
            panel
        } else {
            let created = self.board.create_panel(
                PanelOptions {
                    name: Some("GitHub access".into()),
                    name_is_custom: Some(true),
                    kind: PanelKind::Browser,
                    command: Some(fix.url().to_owned()),
                    browser_config: Some(self.template_config.browser.clone()),
                    ..PanelOptions::default()
                },
                workspace,
            );
            match created {
                Ok(panel) => {
                    // It belongs to this run: never saved, so no restart brings it back alone.
                    if let Some(created) = self.board.panel_mut(panel) {
                        created.transient = true;
                    }
                    panel
                }
                Err(error) => {
                    if let Some(run) = self.cloud_prototype.production.setup_run_mut(key) {
                        run.notice = Some(error.to_string());
                    }
                    return;
                }
            }
        };
        let local = self
            .board
            .panel(browser)
            .map(|panel| panel.local_id.clone())
            .unwrap_or_default();
        if let Some(run) = self.cloud_prototype.production.setup_run_mut(key) {
            run.fix_panel = Some(browser);
            run.fixer = Some(fixer);
        }
        match fixer {
            Fixer::Agent => self.steer_cloud_setup(ctx, key, &fix.task(&local)),
            Fixer::Person => self.reveal_cloud_setup_panel(ctx, browser),
        }
    }
}

/// What the agent hears when it wrote a summary over a preview Horizon cannot start.
const INVALID_PREVIEW: &str = "Horizon cannot start the preview in the status file: it needs \"command\", one shell command of at most 4096 characters, and optionally \"size\" such as \"1280x800\". Fix it, or set preview to null when there is no GUI app, and increase preview_request by one. Use this setup waits for it.";

/// What the agent hears when it wrote a summary but no proof Horizon could run.
const MISSING_PROOF: &str = "Horizon has no proof to run: put a valid proof in the status file (a workflow with at least one job, or commands) and set proof_request to 1. Use this setup waits for Horizon's proof.";

/// What the agent hears when the setup changed after Horizon's proof passed.
const STALE_PROOF: &str = "The .horizon files or the proof changed after Horizon's proof passed, so it no longer proves this setup. Increase proof_request by one so Horizon proves it again.";

/// Why the setup agent did not start on a Linux computer without a systemd user manager.
const NO_SCOPE: &str = "The setup agent needs a systemd user session (systemd-run --user), so Horizon can end everything it starts before it commits. Start one, or use Quick start or write .horizon yourself.";

/// The longest Horizon's exit waits for setup runs to end; the window does not draw meanwhile.
const EXIT_WAIT: Duration = Duration::from_secs(20);

/// The size of the Device panel that shows a live preview.
const PREVIEW_PANEL_SIZE: [f32; 2] = [1040.0, 960.0];

/// The name of a setup agent's hidden panel.
const PANEL_NAME: &str = "Cloud repository setup";

/// The local workspace of the setup agents and the setup board.
pub(super) const WORKSPACE: &str = "Cloud setup";

impl HorizonApp {
    /// Ends every setup run, and every setup about to start, when the board they belong
    /// to goes: a session switch restarts panel ids, so a kept id could name another panel.
    pub(in crate::app) fn end_cloud_repository_setup(&mut self) {
        for run in std::mem::take(&mut self.cloud_prototype.production.setup_runs) {
            run.discard();
        }
        for starting in std::mem::take(&mut self.cloud_prototype.production.setup_starting) {
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

    /// Ends every setup run as Horizon exits, and waits up to [`EXIT_WAIT`] until their
    /// agents and proofs have ended and what they made is removed, with every run already
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
        // The resources kept on a Docker host are retried with a new forward next time.
        super::proof::close_forwards();
        // The rented builder goes last: the runs' resources on it go with it.
        self.cloud_prototype.production.setup_builder.give_back_for_exit();
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
    /// It looks only when the panels or the runs changed since it last looked, so a frame
    /// without such a change costs nothing: a hidden setup panel appears only as panels
    /// are added or a session's board loads, and stops being a run's only as the run
    /// ends.
    fn close_orphaned_setup_panels(&mut self, ctx: &egui::Context) {
        let current = self.cloud_prototype.production.setup_run_panels();
        let looked = egui::Id::new("cloud-setup-orphan-scan");
        let now = (
            self.board.panels.len(),
            self.board.panels.last().map(|panel| panel.id),
            current.clone(),
        );
        if ctx.data(|data| data.get_temp(looked)).as_ref() == Some(&now) {
            return;
        }
        ctx.data_mut(|data| data.insert_temp(looked, now));
        let orphans: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.transient && panel.title == PANEL_NAME && !current.contains(&panel.id))
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
    // What earlier cleanups could not remove is tried again in the background. One
    // setup for each folder: an earlier run of this repository still ending, as after
    // Stop, ends first. Runs of other repositories go on. The count is read first, so
    // an ending that begins later is noticed before the agent starts.
    let endings = run::endings_begun(&repository);
    super::proof::retry_unremoved();
    run::wait_for_ended_runs_of(&repository);
    // Read once the earlier run ended: what it wrote decides the mode, as the baseline.
    let mode = chosen_mode.resolve(&repository);
    let instructions = prompt(agents, (&repository, &status), &mode);
    let unit = format!(
        "horizon-setup-agent-{}-{}.scope",
        super::proof::run_id(&status),
        std::process::id()
    );
    Ok(Prepared {
        endings,
        identity: super::untrusted::identity(&repository),
        outside: super::commit::outside(&repository),
        dotnet: super::proof::uses_nuget(&repository),
        launcher: super::agent::scope_launcher(&unit),
        // Read from the commit, as the proof reads the lock files.
        git_crates: super::proof::crates::locked_repositories(&repository, "HEAD"),
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

/// The finished card's list of changed files, made off the dialog's thread.
fn list_changed_files(run: &mut Run, ctx: &egui::Context) {
    if !run.finished() || run.files.is_some() {
        return;
    }
    if let Some(job) = &run.files_job {
        if let Ok(files) = job.try_recv() {
            run.files = Some(files);
            run.files_job = None;
        }
        return;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let repository = run.repository.clone();
    let wake = ctx.clone();
    std::thread::spawn(move || {
        let _ = sender.send(changed_files(&repository));
        wake.request_repaint();
    });
    run.files_job = Some(receiver);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The setup agent is offered on Linux only.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_setup_prepared_before_its_folder_began_to_end_prepares_again() {
        let (temp, ctx, mut app) =
            crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
                runtime_state: Box::default(),
            });
        let folder = temp.path().join("repository");
        std::fs::create_dir_all(&folder).unwrap();
        let entered = folder.display().to_string();
        // Prepared before an earlier run of the folder began to end, such as a run that a
        // link to the folder started and that is stopped while this one prepares.
        let prepared = prepare(&entered, super::super::Mode::New, &[]).unwrap();
        let first_status = prepared.status.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok(prepared)).unwrap();
        app.cloud_prototype.production.setup_starting.push(Starting {
            preset: horizon_core::PresetConfig {
                name: "Test panel".into(),
                kind: PanelKind::Usage,
                alias: None,
                command: None,
                args: Vec::new(),
                resume: horizon_core::PanelResume::default(),
                ssh_connection: None,
            },
            chosen_mode: super::super::Mode::New,
            entered,
            build_on: None,
            receiver,
        });
        // That run is stopped, and its ending is already over by the next frame: the
        // baseline may still hold what its agent wrote.
        let canonical = std::fs::canonicalize(&folder).unwrap();
        run::keep_ending_of(&canonical, std::thread::spawn(|| {}));
        run::wait_for_ended_runs_of(&canonical);
        app.sync_cloud_repository_setup(&ctx);
        assert!(
            app.cloud_prototype.production.setup_runs.is_empty(),
            "no agent starts on a baseline read while another one ended"
        );
        assert_eq!(
            app.cloud_prototype.production.setup_starting.len(),
            1,
            "it prepares again"
        );
        let started = Instant::now();
        while !app.cloud_prototype.production.setup_starting.is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "it started after preparing again"
            );
            std::thread::sleep(Duration::from_millis(20));
            app.sync_cloud_repository_setup(&ctx);
        }
        assert_eq!(app.cloud_prototype.production.setup_runs.len(), 1);
        let gone = Instant::now();
        while first_status.parent().is_some_and(Path::exists) {
            assert!(
                gone.elapsed() < Duration::from_secs(10),
                "the first preparation's folder goes"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        app.end_cloud_repository_setup();
    }

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
