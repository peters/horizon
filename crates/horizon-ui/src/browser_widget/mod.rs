//! Body of `PanelKind::Browser` panels: chrome strip, live screencast
//! frames, and egui→CDP input routing.
//!
//! Perf notes: the texture is only updated when the frame `seq` changes
//! (screencasts are change-driven, so idle pages cost nothing), and mouse
//! moves are deduplicated to actual movement.

mod chrome;
mod input;
mod render;

use egui::{Pos2, TextureHandle, Ui};
use horizon_core::Panel;
use horizon_core::browser::{BrowserButton, BrowserCommand};

/// Per-panel UI state that must survive across frames.
#[derive(Default)]
pub struct BrowserUiState {
    texture: Option<TextureHandle>,
    seq: u64,
    /// Last viewport size sent to Chrome (responsive layout + input
    /// geometry follow the panel; the driver dedupes no-ops).
    last_viewport: (u32, u32),
    /// Last mouse position forwarded to the page (movement dedup).
    last_mouse: Option<Pos2>,
    /// Button captured by this panel (drags must deliver their release
    /// even when it lands outside the rect).
    captured_button: Option<BrowserButton>,
    /// URL bar buffer (follows the panel URL while unfocused).
    url_buffer: String,
}

pub struct BrowserView<'a> {
    panel: &'a mut Panel,
    ui_state: &'a mut BrowserUiState,
}

impl<'a> BrowserView<'a> {
    #[must_use]
    pub fn new(panel: &'a mut Panel, ui_state: &'a mut BrowserUiState) -> BrowserView<'a> {
        Self { panel, ui_state }
    }

    pub fn show(&mut self, ui: &mut Ui, is_focused: bool, interactive: bool) -> bool {
        if self.panel.browser().is_none() {
            ui.centered_and_justified(|ui| ui.label("Browser content missing"));
            return false;
        }
        let state = &mut *self.ui_state;
        let panel_id = self.panel.id;

        let (url_focused, chrome_clicked) = {
            let Some(browser) = self.panel.browser_mut() else {
                return false;
            };
            chrome::show(ui, panel_id, browser, state, interactive)
        };
        let (body_rect, frame_size, retry_clicked, body_clicked) = {
            let Some(browser) = self.panel.browser() else {
                return false;
            };
            render::show_body(ui, panel_id, browser, state)
        };
        if let Some(browser) = self.panel.browser_mut() {
            if retry_clicked {
                browser.relaunch();
            }
            // Follow panel resizes/fullscreen with the emulated viewport
            // so responsive layout and CDP input geometry match what is on
            // screen (the driver no-ops an unchanged size).
            if let Some(rect) = body_rect {
                // egui layout sizes are finite and non-negative.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let viewport = (rect.width().round() as u32, rect.height().round() as u32);
                if viewport.0 > 32 && viewport.1 > 32 && viewport != state.last_viewport {
                    state.last_viewport = viewport;
                    browser.send(BrowserCommand::SetViewport {
                        width: viewport.0,
                        height: viewport.1,
                    });
                }
            }
            input::handle(
                ui,
                browser,
                state,
                body_rect,
                frame_size,
                input::InputFlags {
                    is_focused,
                    interactive,
                    url_focused,
                },
            );
        }
        // Request panel focus only on an actual click in this panel (same
        // convention as the terminal body); an unconditional request would
        // steal focus from other panels every frame.
        chrome_clicked || body_clicked
    }
}
