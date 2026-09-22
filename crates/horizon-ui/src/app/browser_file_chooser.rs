use horizon_core::WorkspaceId;

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn browser_file_chooser_open(&self) -> bool {
        self.board.panels.iter().any(|panel| {
            panel
                .browser()
                .is_some_and(|browser| browser.frame_slot.file_chooser().request().is_some())
        })
    }

    pub(super) fn host_dialog_open(&self) -> bool {
        self.cloud_creation_open() || self.browser_file_chooser_open()
    }

    pub(super) fn render_browser_file_chooser(&mut self, ctx: &egui::Context, workspace: Option<WorkspaceId>) {
        let candidate = self.board.panels.iter().find_map(|panel| {
            let in_viewport = workspace.map_or_else(
                || {
                    self.board
                        .workspace(panel.workspace_id)
                        .is_some_and(|workspace| !self.detached_workspaces.contains_key(&workspace.local_id))
                },
                |workspace| panel.workspace_id == workspace,
            );
            let browser = panel.browser()?;
            (in_viewport && browser.frame_slot.file_chooser().request().is_some())
                .then(|| (panel.id, browser.frame_slot.file_chooser().clone()))
        });
        if let Some((panel, handle)) = candidate
            && self
                .panel_render_caches
                .browser_ui_state
                .entry(panel)
                .or_default()
                .show_file_chooser(ctx, panel, &handle)
        {
            self.consume_navigation_key(
                ctx,
                horizon_core::ShortcutBinding::new(
                    horizon_core::ShortcutModifiers::NONE,
                    horizon_core::ShortcutKey::Escape,
                ),
            );
        }
    }
}
