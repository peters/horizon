//! Upkeep in the app: the New Upkeep dialog, and what Upkeep boards ask for while they
//! are drawn.
use egui::Context;
use horizon_core::upkeep::Request;
use horizon_core::{PanelId, PanelKind, PanelOptions};

use super::HorizonApp;

#[cfg(feature = "cloud-workspaces")]
mod dialog;
#[cfg(feature = "cloud-workspaces")]
mod editor;
#[cfg(feature = "cloud-workspaces")]
pub(super) mod form;
#[cfg(feature = "cloud-workspaces")]
mod repositories;
#[cfg(feature = "cloud-workspaces")]
mod state;

#[cfg(feature = "cloud-workspaces")]
pub(super) use state::{Answer, Exchanged, Prepared, Preparing, Read, Saved, State, Sync};

/// Without cloud workspaces there is no Upkeep to keep.
#[cfg(not(feature = "cloud-workspaces"))]
#[derive(Default)]
pub(super) struct State;

/// The title of the browser panel that shows pull requests beside a board.
const PULL_REQUEST_PANEL: &str = "Pull request";

/// What other parts of the app call. They live here so they stay in one place.
#[cfg(feature = "cloud-workspaces")]
impl HorizonApp {
    /// The folder of this computer's cloud settings, once cloud workspaces are ready.
    pub(super) fn upkeep_root(&self) -> Option<std::path::PathBuf> {
        self.ready_cloud_root()
    }

    /// After the saved choice of `cloud` changed: reads the saved Upkeeps again and makes
    /// the next exchange with its worker immediate.
    pub(super) fn upkeep_settings_saved(&mut self, cloud: &str) {
        // A read under way may hold the list from before the change.
        self.upkeep.saved = None;
        self.upkeep.reading = None;
        self.upkeep.read_after = None;
        self.upkeep.syncs.entry(cloud.to_owned()).or_default().next = None;
    }

    /// Gives each board that shows a saved Upkeep's status its cloud, so it offers Run now,
    /// Pause and Repositories.
    pub(super) fn link_upkeep_boards(&mut self, saved: &Saved) {
        for panel in &mut self.board.panels {
            let Some(board) = panel.content.upkeep_mut() else {
                continue;
            };
            board.cloud = horizon_core::upkeep::worker::cloud_of(board.path())
                .filter(|cloud| saved.iter().any(|(held, _)| held == cloud));
        }
    }

    /// Carries out Run now and Pause of the board of `cloud` with the next exchange, which
    /// starts at once.
    fn signal_upkeep(&mut self, cloud: &str, run_now: bool, paused: Option<bool>) {
        let sync = self.upkeep.syncs.entry(cloud.to_owned()).or_default();
        sync.signals.run_now |= run_now;
        if paused.is_some() {
            sync.signals.paused = paused;
        }
        sync.next = None;
    }
}

#[cfg(feature = "cloud-workspaces")]
impl HorizonApp {
    /// Opens New Upkeep and starts the search for repositories.
    pub(super) fn open_upkeep_dialog(&mut self, ctx: &Context) {
        // A start under way shows again, rather than a second dialog.
        if let Some(form) = self.upkeep.form.as_mut() {
            form.hidden = false;
            return;
        }
        let mut form = form::Form::default();
        if let Some(root) = self.upkeep_root() {
            form.find(root, ctx);
        }
        self.upkeep.form = Some(form);
    }

    pub(super) fn render_upkeep_dialog(&mut self, ctx: &Context) {
        let Some(form) = self.upkeep.form.as_mut() else {
            return;
        };
        form.receive();
        form.forget_counts_without_bots();
        // The counts are those of the chosen bots: another choice searches again.
        if form.stale()
            && let Some(root) = self.upkeep_root()
            && let Some(form) = self.upkeep.form.as_mut()
        {
            form.find(root, ctx);
        }
        let Some(form) = self.upkeep.form.as_mut().filter(|form| !form.hidden) else {
            return;
        };
        match dialog::show(ctx, form) {
            dialog::Outcome::Open => {}
            // A start under way goes on: its cloud appears when its repository is ready,
            // and the dialog shows again if it fails.
            dialog::Outcome::Cancel if form.starting => form.hidden = true,
            dialog::Outcome::Cancel => self.upkeep.form = None,
            dialog::Outcome::Start => {
                let (config, workspace) = (form.config(), form.workspace);
                form.error = None;
                form.starting = true;
                if let Err(error) = self.start_upkeep(ctx, workspace, config) {
                    self.upkeep_started(Err(error));
                }
            }
        }
    }

    /// Closes New Upkeep after its cloud was added, or shows why the start failed.
    pub(super) fn upkeep_started(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.upkeep.form = None,
            Err(error) => {
                tracing::warn!(%error, "Upkeep did not start");
                if let Some(form) = self.upkeep.form.as_mut() {
                    form.starting = false;
                    form.hidden = false;
                    form.error = Some(error);
                }
            }
        }
    }
}

impl HorizonApp {
    /// Carries out what Upkeep boards asked for while they were drawn.
    pub(super) fn apply_upkeep_requests(&mut self, ctx: &Context) {
        let requests: Vec<(PanelId, Request)> = self
            .board
            .panels
            .iter_mut()
            .filter_map(|panel| {
                let request = panel.content.upkeep_mut()?.request.take()?;
                Some((panel.id, request))
            })
            .collect();
        for (board, request) in requests {
            match request {
                Request::Open(url) => self.open_pull_request(ctx, board, &url),
                #[cfg(feature = "cloud-workspaces")]
                Request::RunNow | Request::Pause(_) | Request::Repositories => {
                    let Some(cloud) = self
                        .board
                        .panel(board)
                        .and_then(|panel| panel.content.upkeep())
                        .and_then(|upkeep| upkeep.cloud.clone())
                    else {
                        continue;
                    };
                    match request {
                        Request::RunNow => self.signal_upkeep(&cloud, true, None),
                        Request::Pause(paused) => self.signal_upkeep(&cloud, false, Some(paused)),
                        _ => {
                            // While the saved Upkeeps are read again, the click waits a frame.
                            if !self.open_upkeep_repositories(ctx, &cloud)
                                && let Some(upkeep) =
                                    self.board.panel_mut(board).and_then(|panel| panel.content.upkeep_mut())
                            {
                                upkeep.request = Some(Request::Repositories);
                            }
                        }
                    }
                }
                // Without cloud workspaces no board has a cloud, so none offers these.
                #[cfg(not(feature = "cloud-workspaces"))]
                Request::RunNow | Request::Pause(_) | Request::Repositories => {}
            }
        }
    }

    /// Shows `url` in the workspace's pull request browser panel, opening one beside the
    /// board when there is none.
    fn open_pull_request(&mut self, ctx: &Context, board: PanelId, url: &str) {
        let Some(workspace) = self.board.panel(board).map(|panel| panel.workspace_id) else {
            return;
        };
        // A fullscreen board would hide the page it opens.
        if self.fullscreen_panel == Some(board) {
            self.fullscreen_panel = None;
        }
        // Only the panel this board opened is reused: a title can be anyone's.
        let existing = self
            .board
            .panel(board)
            .and_then(|panel| panel.content.upkeep())
            .and_then(|upkeep| upkeep.browser)
            .filter(|page| {
                self.board
                    .panel(*page)
                    .is_some_and(|panel| panel.workspace_id == workspace && panel.kind == PanelKind::Browser)
            });
        if let Some(panel) = existing {
            if let Some(browser) = self.board.panel_mut(panel).and_then(|panel| panel.browser_mut()) {
                browser.submit_navigation(url);
            }
            self.board.focus(panel);
            // The panel may be out of view; show it beside the board.
            self.show_board_and_page(ctx, workspace);
            return;
        }
        // Beside the board, unless the workspace arranges its panels itself: an explicit
        // position would turn that arrangement off.
        let arranged = self
            .board
            .workspace(workspace)
            .is_some_and(|workspace| workspace.layout.is_some());
        let position = (!arranged).then(|| self.board.panel(board)).flatten().map(|panel| {
            let layout = &panel.layout;
            [layout.position[0] + layout.size[0] + 24.0, layout.position[1]]
        });
        let created = self.board.create_panel(
            PanelOptions {
                name: Some(PULL_REQUEST_PANEL.into()),
                name_is_custom: Some(true),
                kind: PanelKind::Browser,
                command: Some(url.to_owned()),
                browser_config: Some(self.template_config.browser.clone()),
                position,
                size: Some([960.0, 780.0]),
                ..PanelOptions::default()
            },
            workspace,
        );
        let panel = match created {
            Ok(panel) => panel,
            Err(error) => {
                tracing::warn!(%error, "could not open a browser panel for an Upkeep pull request");
                return;
            }
        };
        if let Some(upkeep) = self.board.panel_mut(board).and_then(|board| board.content.upkeep_mut()) {
            upkeep.browser = Some(panel);
        }
        self.board.focus(panel);
        self.show_board_and_page(ctx, workspace);
        self.mark_runtime_dirty();
    }

    /// Fits the workspace so the board and its page are seen side by side, in the main
    /// window or in the workspace's own window.
    fn show_board_and_page(&mut self, ctx: &Context, workspace: horizon_core::WorkspaceId) {
        if !self.workspace_is_detached(workspace) {
            self.fit_workspace_visible(ctx, workspace);
            return;
        }
        let local_id = self
            .board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone());
        if let Some(detached) = local_id.and_then(|local_id| self.detached_workspaces.get_mut(&local_id)) {
            detached.pending_fit = true;
        }
        ctx.request_repaint();
    }
}
