//! A link pressed on a card, and the browser panel that it opens.
//!
//! Horizon opens the page itself: the press reaches no agent, records no click
//! and fetches nothing before the person presses. The panel is the browser panel
//! that the canvas picker makes, with the address of the link as its start page,
//! placed beside the card that named it in the card's own workspace.
//!
//! A press that opens no page says so twice, where the person looks: the card
//! keeps a line under that link, and the sidebar says the reason once. A browser
//! engine that cannot start says the same reason, because its panel ends at once
//! and the cleanup that removes it reports the reason first.

use egui::Context;
use horizon_core::{PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::HorizonApp;
use super::actions::support::preset_panel_options;
use super::creation_notice::LinkOrigin;

impl HorizonApp {
    /// Opens `url` in a browser panel of the workspace of the card `panel_id`,
    /// beside that card.
    ///
    /// The document was validated when it arrived, so the address is an absolute
    /// `http` or `https` URL without credentials. A press on a panel that is no
    /// card opens nothing.
    pub(in crate::app) fn open_card_link(&mut self, ctx: &Context, panel_id: PanelId, url: &str) {
        let Some(workspace_id) = self.card_link_workspace(panel_id) else {
            return;
        };
        let options = self.link_browser_options(url);
        match self.create_link_panel(options, workspace_id, panel_id) {
            Ok(opened) => {
                // The engine of a new panel can still refuse to start, and the
                // cleanup then says why this press opened no page.
                self.person_browser_creates.note(
                    opened,
                    Some(LinkOrigin {
                        card: panel_id,
                        url: url.to_owned(),
                    }),
                );
                self.card_link_opened(panel_id, url);
                self.reveal_new_panel(ctx, workspace_id, opened);
                self.mark_runtime_dirty();
            }
            // The notice of the failed creation is the sidebar's, which every
            // panel creation of a person uses; the card says it under the link.
            Err(error) => {
                tracing::error!("failed to create panel: {error}");
                self.notice_panel_creation_failure(PanelKind::Browser, &error);
                self.card_link_failed(panel_id, url, &error.to_string());
            }
        }
    }

    /// Records that the press on the link of `url` opened no page, and why, so
    /// the card says it under that link.
    pub(in crate::app) fn card_link_failed(&mut self, panel_id: PanelId, url: &str, reason: &str) {
        let Some(revision) = self.card_revision(panel_id) else {
            return;
        };
        self.panel_render_caches
            .card_runtime
            .entry(panel_id)
            .or_default()
            .fail_link(revision, url, reason);
    }

    /// Records that the press on the link of `url` opened its page, so its line
    /// of a failed press goes away.
    fn card_link_opened(&mut self, panel_id: PanelId, url: &str) {
        if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&panel_id) {
            card.clear_link_failure(url);
        }
    }

    /// The revision of the document that the card `panel_id` shows.
    pub(in crate::app) fn card_revision(&self, panel_id: PanelId) -> Option<u64> {
        self.board
            .panel(panel_id)
            .and_then(|panel| panel.block())
            .map(|card| card.revision)
    }

    /// The workspace that a browser panel of a card's link goes to: the
    /// workspace of the card, whichever workspace the person looks at now.
    #[must_use]
    pub(in crate::app) fn card_link_workspace(&self, panel_id: PanelId) -> Option<WorkspaceId> {
        self.board
            .panel(panel_id)
            .filter(|panel| panel.kind == PanelKind::Block)
            .map(|panel| panel.workspace_id)
    }

    /// The options of the browser panel that a link opens: the browser panel of
    /// the person's Browser preset, with the address of the link as its start
    /// page.
    ///
    /// A browser panel of a workspace on a machine stays a browser panel of this
    /// Horizon, exactly as one of the canvas picker does: the page is fetched by
    /// the browser engine that the browser host of the settings names.
    fn link_browser_options(&self, url: &str) -> PanelOptions {
        let mut options = self
            .presets
            .iter()
            .find(|preset| preset.kind == PanelKind::Browser)
            .map_or_else(
                || PanelOptions {
                    kind: PanelKind::Browser,
                    browser_config: Some(self.template_config.browser.clone()),
                    follows_browser_host: true,
                    ..PanelOptions::default()
                },
                |preset| preset_panel_options(preset, &self.template_config.browser),
            );
        // A browser panel reads its start page from `command`.
        options.command = Some(url.to_owned());
        options
    }

    /// Creates the browser panel of a pressed link, beside the card that named
    /// it.
    ///
    /// A browser panel of a test would start a browser engine, so the tests of
    /// the press stand an inert browser panel in its place; see
    /// [`tests::HorizonApp::press_link_for_tests`].
    #[cfg(not(test))]
    fn create_link_panel(
        &mut self,
        options: PanelOptions,
        workspace_id: WorkspaceId,
        card: PanelId,
    ) -> horizon_core::Result<PanelId> {
        self.create_panel_with_options_beside(options, workspace_id, Some(card))
    }

    /// The seam of the tests: the press stands an inert browser panel in the
    /// place of the creation above, because a browser panel of a test would start
    /// a browser engine.
    ///
    /// The place of the workspace decides the press exactly as it does for the
    /// creation above, so a card of a workspace whose machine is gone fails the
    /// same way; only the engine is stood in.
    #[cfg(test)]
    fn create_link_panel(
        &mut self,
        options: PanelOptions,
        workspace_id: WorkspaceId,
        card: PanelId,
    ) -> horizon_core::Result<PanelId> {
        let options = self.placed_options(options, workspace_id)?;
        Ok(self.press_link_for_tests(&options, workspace_id, card))
    }
}

#[cfg(test)]
mod tests;
