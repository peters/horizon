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
use std::time::Instant;

impl HorizonApp {
    /// Starts the setup agent in a hidden panel; the dialog shows its progress and
    /// passes the person's messages to it.
    pub(in crate::app::cloud_panel::production) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.cloud_prototype.production.setup_agent else {
            return;
        };
        let repository = match std::fs::canonicalize(&self.cloud_prototype.production.repository) {
            Ok(path) if path.is_dir() => path,
            _ => {
                self.cloud_prototype.error = Some("Choose an existing local repository directory".into());
                return;
            }
        };
        let status = status_path();
        let mode = std::mem::take(&mut self.cloud_prototype.production.setup_mode).resolve(&repository);
        let args = vec![prompt(&self.cloud_prototype.production.setup_agents, &status, &mode)];
        let workspace = self.board.ensure_local_workspace("Cloud setup");
        // Deliberately local: config preparation must not inherit a focused cloud.
        let result = self.board.create_panel(
            PanelOptions {
                name: Some("Cloud repository setup".into()),
                name_is_custom: Some(true),
                kind,
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
                self.cloud_prototype.production.setup_run = Some(Run::new(panel, kind, repository, status, mode));
                self.cloud_prototype.error = None;
                ctx.request_repaint();
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
    }

    /// Samples the agent's screen, delivers a pending Enter, lists the files of a
    /// finished run and notices a setup panel that closed.
    pub(in crate::app::cloud_panel::production) fn sync_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        if run.finished() && run.files.is_none() {
            run.files = Some(changed_files(&run.repository));
        }
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
        let Some((panel_id, kind)) = self
            .cloud_prototype
            .production
            .setup_run
            .as_ref()
            .map(|run| (run.panel, run.kind))
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
                    let agent = if kind == PanelKind::Codex { "Codex" } else { "Claude" };
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
                // Pasted, so a terminal agent takes it as one message even mid-turn.
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend(text.replace(['\r', '\n'], " ").as_bytes());
                bytes.extend(b"\x1b[201~");
                if let Some(panel) = self.board.panel_mut(panel) {
                    panel.write_input(&bytes);
                    run.submit_at = Some(Instant::now() + run::SUBMIT_DELAY);
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
                let _ = std::fs::remove_file(path);
            }
            Action::Use => self.use_cloud_repository_setup(ctx),
        }
    }

    /// Commits the files the agent wrote and reads them as the repository's settings.
    fn use_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(run) = self.cloud_prototype.production.setup_run.as_ref() else {
            return;
        };
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
        let committed =
            git(&["add", "--", ".horizon"]).and_then(|()| git(&["commit", "--quiet", "-m", message, "--", ".horizon"]));
        if let Err(error) = committed {
            self.cloud_prototype.error = Some(format!("Could not commit .horizon: {error}"));
            return;
        }
        if let Some(run) = self.cloud_prototype.production.setup_run.take() {
            if self.board.panel(run.panel).is_some() {
                self.close_panel(run.panel);
            }
            let _ = std::fs::remove_file(run.status_path);
        }
        set_configuration(&mut self.cloud_prototype.production, Configuration::Committed);
        self.read_cloud_profiles(ctx);
    }
}

/// A private file for one run's status, outside the repository.
fn status_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    std::env::temp_dir().join(format!("horizon-cloud-setup-{}-{stamp}.json", std::process::id()))
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
