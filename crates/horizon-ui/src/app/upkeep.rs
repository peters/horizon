//! Carries out what Upkeep boards ask for while they are drawn.
use egui::Context;
use horizon_core::upkeep::Request;
use horizon_core::{PanelId, PanelKind, PanelOptions};

use super::HorizonApp;

/// The title of the browser panel that shows pull requests beside a board.
const PULL_REQUEST_PANEL: &str = "Pull request";

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
            }
        }
    }

    /// Shows `url` in the workspace's pull request browser panel, opening one beside the
    /// board when there is none.
    fn open_pull_request(&mut self, ctx: &Context, board: PanelId, url: &str) {
        let Some(workspace) = self.board.panel(board).map(|panel| panel.workspace_id) else {
            return;
        };
        let existing = self
            .board
            .panels
            .iter()
            .find(|panel| {
                panel.workspace_id == workspace && panel.kind == PanelKind::Browser && panel.title == PULL_REQUEST_PANEL
            })
            .map(|panel| panel.id);
        if let Some(panel) = existing {
            if let Some(browser) = self.board.panel_mut(panel).and_then(|panel| panel.browser_mut()) {
                browser.submit_navigation(url);
            }
            self.board.focus(panel);
            return;
        }
        let position = self.board.panel(board).map(|panel| {
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
        if let Ok(panel) = created {
            self.board.focus(panel);
        }
        // The board and the page it opened are seen side by side.
        self.fit_workspace_visible(ctx, workspace);
        self.mark_runtime_dirty();
    }
}
