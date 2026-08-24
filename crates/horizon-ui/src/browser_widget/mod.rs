//! Body of `PanelKind::Browser` panels: chrome strip, live screencast
//! frames, and egui→CDP input routing.
//!
//! Perf notes: the texture is only updated when the frame `seq` changes
//! (screencasts are change-driven, so idle pages cost nothing), and mouse
//! moves are deduplicated to actual movement.

mod chrome;
mod input;
mod render;

use egui::{Event, Pos2, TextureHandle, Ui};
use horizon_core::browser::{BrowserButton, BrowserCommand, BrowserKey, BrowserModifiers};
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
    /// Modifier state at the end of the preceding frame. Pointer moves do
    /// not carry their own snapshot, so ordered event replay starts here.
    pointer_modifiers: BrowserModifiers,
    /// Presses captured by this panel (drags must deliver each release even
    /// when it lands outside the rect). Counts are reused on release.
    captured_clicks: [Option<BrowserPointerClick>; 3],
    /// Most recent completed click, used to identify double/triple clicks.
    last_click: Option<BrowserPointerClick>,
    /// URL bar buffer (follows the panel URL while unfocused).
    url_buffer: String,
    /// Enter submitted in the URL bar; its later key-up must not leak to the
    /// previously focused page element after the text edit drops focus.
    url_submit_enter_pending: bool,
    /// Every key-down delivered to the page, with its layout-resolved text
    /// when present. Tracking non-printable keys too lets focus loss synthesize
    /// every matching key-up instead of leaving Chrome's key state stuck.
    pressed_keys: std::collections::HashMap<BrowserKey, Option<String>>,
    /// App-owned shortcuts and browser-local reload stay consumed through
    /// their release even if a later frame has different modifier state.
    suppressed_shortcut_keys: std::collections::HashSet<BrowserKey>,
    /// Copy/Cut pseudo-events synthesize a complete CDP key pair; consume a
    /// later native release, but abandon suppression if a new press starts.
    clipboard_release_keys: std::collections::HashSet<BrowserKey>,
    /// Escape exits panel fullscreen after the app has already cleared the
    /// fullscreen flag, so remember the preceding frame for input filtering.
    fullscreen_active_last_frame: bool,
}

#[derive(Clone, Copy)]
struct BrowserPointerClick {
    button: BrowserButton,
    position: Pos2,
    time: f64,
    count: u32,
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

    pub fn show(&mut self, ui: &mut Ui, events: &[Event], is_focused: bool, interactive: bool) -> bool {
        if self.panel.browser().is_none() {
            ui.centered_and_justified(|ui| ui.label("Browser content missing"));
            return false;
        }
        let state = &mut *self.ui_state;
        let panel_id = self.panel.id;
        if let Some(browser) = self.panel.browser_mut()
            && let Some(text) = browser.take_clipboard_text()
        {
            ui.ctx().copy_text(text);
        }

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
        let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
        let other_widget_has_focus = ui
            .memory(egui::Memory::focused)
            .is_some_and(|focused| body.keyboard_focus_id != Some(focused));
        let page_keyboard_active =
            page_keyboard_can_route(window_focused && is_focused, url_focused, other_widget_has_focus);
        let keyboard_target = if is_focused && url_focused {
            input::KeyboardTarget::Url
        } else if page_keyboard_active {
            input::KeyboardTarget::Page
        } else {
            input::KeyboardTarget::None
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
                input::cancel_pointer_capture(browser, state, body.image_rect, body.frame_size);
                if browser.try_send(BrowserCommand::SetViewport {
                    width: viewport.0,
                    height: viewport.1,
                }) {
                    state.last_viewport = viewport;
                }
            }
            input::handle(
                ui,
                browser,
                state,
                body.image_rect,
                body.frame_size,
                body.pointer_target,
                input::InputFlags {
                    events,
                    interactive,
                    keyboard_target,
                    pointer_viewport: if frame_matches_viewport(
                        body.frame_size,
                        body.viewport_size,
                        state.last_viewport,
                    ) {
                        input::PointerViewportState::Ready
                    } else {
                        input::PointerViewportState::AwaitingFrame
                    },
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

const fn page_keyboard_can_route(
    viewport_panel_focused: bool,
    url_focused: bool,
    other_widget_has_focus: bool,
) -> bool {
    viewport_panel_focused && !url_focused && !other_widget_has_focus
}

fn frame_matches_viewport(
    frame_size: Option<[f32; 2]>,
    desired_viewport: Option<(u32, u32)>,
    sent_viewport: (u32, u32),
) -> bool {
    let (Some(frame_size), Some(desired_viewport)) = (frame_size, desired_viewport) else {
        return false;
    };
    let (Ok(width), Ok(height)) = (u16::try_from(sent_viewport.0), u16::try_from(sent_viewport.1)) else {
        return false;
    };
    desired_viewport == sent_viewport
        && (frame_size[0] - f32::from(width)).abs() <= f32::EPSILON
        && (frame_size[1] - f32::from(height)).abs() <= f32::EPSILON
}

#[cfg(test)]
mod tests {
    use super::{frame_matches_viewport, page_keyboard_can_route};

    #[test]
    fn pointer_input_waits_for_the_sent_viewport_frame() {
        assert!(frame_matches_viewport(
            Some([800.0, 600.0]),
            Some((800, 600)),
            (800, 600)
        ));
        assert!(!frame_matches_viewport(
            Some([1280.0, 800.0]),
            Some((800, 600)),
            (800, 600)
        ));
        assert!(!frame_matches_viewport(
            Some([800.0, 600.0]),
            Some((900, 600)),
            (800, 600)
        ));
        assert!(!frame_matches_viewport(None, Some((800, 600)), (800, 600)));
    }

    #[test]
    fn page_keyboard_requires_window_panel_and_egui_focus() {
        assert!(page_keyboard_can_route(true, false, false));
        assert!(!page_keyboard_can_route(false, false, false));
        assert!(!page_keyboard_can_route(true, true, false));
        assert!(!page_keyboard_can_route(true, false, true));
    }
}
