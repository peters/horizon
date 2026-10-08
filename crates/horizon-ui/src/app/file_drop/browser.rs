use egui::{Context, Pos2};
use horizon_core::PanelId;
use horizon_core::browser::{BackendKind, BrowserCommand};

use super::{FileDropScope, HorizonApp};

impl HorizonApp {
    pub(super) fn browser_drop_allowed(&self, panel: PanelId, position: Pos2) -> bool {
        self.board
            .panel(panel)
            .and_then(horizon_core::Panel::browser)
            .is_some_and(|browser| {
                !browser.is_remote()
                    && matches!(browser.backend(), BackendKind::ChromiumCdp | BackendKind::FirefoxBidi)
                    && self
                        .panel_render_caches
                        .browser_ui_state
                        .get(&panel)
                        .and_then(|state| state.drop_geometry)
                        .is_some_and(|(rect, _)| rect.contains(position))
            })
    }

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
        let accepted = browser.try_send(BrowserCommand::DropFiles {
            x: f64::from(offset.x / rect.width() * size[0]),
            y: f64::from(offset.y / rect.height() * size[1]),
            paths: dropped.iter().map(|file| file.path().to_path_buf()).collect(),
        });
        browser.navigation_error = (!accepted).then(|| {
            "The browser could not accept the file drop. Wait for the page to reconnect, then try again.".into()
        });
        self.board.focus(panel);
        ctx.request_repaint();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support;
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelKind};

    #[test]
    fn browser_drop_feedback_excludes_toolbar_and_reports_disconnected_delivery() {
        let (_temp, mut app) = test_support::test_app();
        let workspace = app.board.create_workspace("upload fixture");
        let panel = PanelId(90);
        app.board.panels.push(Panel::from_content(
            panel,
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        ));
        let image = egui::Rect::from_min_max(Pos2::new(10.0, 100.0), Pos2::new(500.0, 400.0));
        app.panel_render_caches
            .browser_ui_state
            .entry(panel)
            .or_default()
            .drop_geometry = Some((image, [490.0, 300.0]));
        let page = image.center();
        assert!(!app.browser_drop_allowed(panel, Pos2::new(50.0, 50.0)));
        assert!(app.browser_drop_allowed(panel, page));
        assert!(app.handle_browser_file_drop(
            &Context::default(),
            Some(panel),
            Some(page),
            FileDropScope::Root,
            &[test_support::dropped_file("/fixture/Overview.pdf")],
        ));
        assert!(
            app.board
                .panel(panel)
                .unwrap()
                .browser()
                .unwrap()
                .navigation_error
                .as_deref()
                .unwrap()
                .contains("try again")
        );
    }
}
