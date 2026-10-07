use egui::{Context, Pos2};
use horizon_core::PanelId;
use horizon_core::browser::{BackendKind, BrowserCommand};

use super::{FileDropScope, HorizonApp};

impl HorizonApp {
    pub(super) fn browser_drop_panel(
        &self,
        fullscreen: Option<PanelId>,
        position: Option<Pos2>,
        scope: FileDropScope,
    ) -> Option<PanelId> {
        let position = position?;
        let panel = if let Some(panel) = fullscreen {
            panel
        } else {
            self.panel_screen_order.iter().rev().copied().find(|id| {
                self.panel_screen_rects
                    .get(id)
                    .is_some_and(|rect| rect.contains(position))
            })?
        };
        (self.panel_is_in_scope(panel, scope) && self.board.panel(panel)?.browser().is_some()).then_some(panel)
    }

    pub(super) fn handle_browser_file_drop(
        &mut self,
        ctx: &Context,
        fullscreen: Option<PanelId>,
        position: Option<Pos2>,
        scope: FileDropScope,
        dropped: &[egui::DroppedFileHandle],
    ) -> bool {
        let Some(panel) = self.browser_drop_panel(fullscreen, position, scope) else {
            return false;
        };
        let geometry = self
            .panel_render_caches
            .browser_ui_state
            .get(&panel)
            .and_then(|state| state.drop_geometry);
        let Some(browser) = self.board.panel_mut(panel).and_then(horizon_core::Panel::browser_mut) else {
            return false;
        };
        if browser.is_remote() || !matches!(browser.backend(), BackendKind::ChromiumCdp | BackendKind::FirefoxBidi) {
            browser.navigation_error =
                Some("File drops require local Chromium or Firefox. Use the page's upload button.".into());
            return true;
        }
        let Some((rect, size)) = geometry else {
            browser.navigation_error = Some("Wait for the browser page to load before dropping files.".into());
            return true;
        };
        let Some(position) = position.filter(|position| rect.contains(*position)) else {
            browser.navigation_error = Some("Drop files on the page content, below the browser toolbar.".into());
            return true;
        };
        let offset = position - rect.min;
        browser.send(BrowserCommand::DropFiles {
            x: f64::from(offset.x / rect.width() * size[0]),
            y: f64::from(offset.y / rect.height() * size[1]),
            paths: dropped.iter().map(|file| file.path().to_path_buf()).collect(),
        });
        browser.navigation_error = None;
        self.board.focus(panel);
        ctx.request_repaint();
        true
    }
}
