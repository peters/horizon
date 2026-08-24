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
use horizon_core::browser::{BrowserButton, BrowserCommand, BrowserKey};
use horizon_core::{AppShortcuts, Panel};

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
    /// Enter submitted in the URL bar; its later key-up must not leak to the
    /// previously focused page element after the text edit drops focus.
    url_submit_enter_pending: bool,
    /// Layout-resolved DOM key captured on key-down and reused for key-up.
    pressed_key_text: std::collections::HashMap<BrowserKey, String>,
    /// App-owned shortcuts and browser-local reload stay consumed through
    /// their release even if a later frame has different modifier state.
    suppressed_shortcut_keys: std::collections::HashSet<BrowserKey>,
    /// Escape exits panel fullscreen after the app has already cleared the
    /// fullscreen flag, so remember the preceding frame for input filtering.
    fullscreen_active_last_frame: bool,
}

pub struct BrowserView<'a> {
    panel: &'a mut Panel,
    ui_state: &'a mut BrowserUiState,
    shortcuts: &'a AppShortcuts,
    fullscreen_active: bool,
}

impl<'a> BrowserView<'a> {
    #[must_use]
    pub fn new(
        panel: &'a mut Panel,
        ui_state: &'a mut BrowserUiState,
        shortcuts: &'a AppShortcuts,
        fullscreen_active: bool,
    ) -> BrowserView<'a> {
        Self {
            panel,
            ui_state,
            shortcuts,
            fullscreen_active,
        }
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
        let body = {
            let Some(browser) = self.panel.browser_mut() else {
                return false;
            };
            render::show_body(ui, panel_id, browser, state)
        };
        if let Some(browser) = self.panel.browser_mut() {
            if body.retry_clicked {
                browser.relaunch();
                // The new Chrome starts at its default viewport. Clear every
                // per-session input/render cache so this frame immediately
                // resends the panel's real viewport and cannot carry a held
                // button or key into the replacement session.
                *state = BrowserUiState::default();
            }
            // Follow panel resizes/fullscreen with the emulated viewport
            // so responsive layout and CDP input geometry match what is on
            // screen (the driver no-ops an unchanged size).
            if let Some(viewport) = body.viewport_size
                && viewport.0 > 32
                && viewport.1 > 32
                && viewport != state.last_viewport
            {
                state.last_viewport = viewport;
                browser.send(BrowserCommand::SetViewport {
                    width: viewport.0,
                    height: viewport.1,
                });
            }
            input::handle(
                ui,
                browser,
                state,
                body.image_rect,
                body.frame_size,
                input::InputFlags {
                    is_focused,
                    interactive,
                    url_focused,
                    shortcuts: self.shortcuts,
                    exit_fullscreen_shortcut_owner: if self.fullscreen_active || state.fullscreen_active_last_frame {
                        input::ShortcutOwner::App
                    } else {
                        input::ShortcutOwner::Page
                    },
                },
            );
        }
        state.fullscreen_active_last_frame = self.fullscreen_active;
        // Request panel focus only on an actual click in this panel (same
        // convention as the terminal body); an unconditional request would
        // steal focus from other panels every frame.
        chrome_clicked || body.body_clicked
    }
}
