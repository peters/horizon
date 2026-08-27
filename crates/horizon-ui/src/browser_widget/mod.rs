//! Body of `PanelKind::Browser` panels: chrome strip, live screencast
//! frames, and egui→CDP input routing.
//!
//! Perf notes: the texture is only updated when the frame `seq` changes
//! (screencasts are change-driven, so idle pages cost nothing), and mouse
//! moves are deduplicated to actual movement.

const VIEWPORT_RETRY_INTERVAL_SECONDS: f64 = 0.25;
const MAX_VIEWPORT_CONVERGENCE_RETRIES: u8 = 8;
// Keep fallback focus recovery beyond the WebDriver HTTP timeout.
const SAFARI_HOST_FOCUS_RECOVERY_SECONDS: f64 = 12.0;

mod chrome;
mod ime;
mod input;
mod render;

use egui::{Event, Pos2, TextureHandle, Ui};
use horizon_core::browser::{BackendKind, BrowserButton, BrowserCommand, BrowserKey, BrowserModifiers};
use horizon_core::{AppShortcuts, Panel};
use std::{sync::Arc, time::Duration};

/// Per-panel UI state that must survive across frames.
#[derive(Default)]
pub struct BrowserUiState {
    /// Backend whose session owns every cache below. A backend switch creates
    /// a new browser at its default viewport, so stale geometry must never
    /// suppress the replacement session's first `SetViewport` command.
    active_backend: Option<BackendKind>,
    texture: Option<TextureHandle>,
    seq: u64,
    /// Last viewport size sent to the active backend (responsive layout and
    /// input geometry follow the panel).
    last_viewport: (u32, u32),
    /// Earliest time a still-mismatched frame may trigger another viewport
    /// command. Protocol success is not visual convergence: browsers can
    /// acknowledge a resize while an older-sized frame remains active.
    viewport_retry_at: f64,
    /// Number of duplicate commands sent for the current target after its
    /// initial resize. This bounds recovery for backends whose window and
    /// content viewport dimensions cannot be made identical.
    viewport_retry_count: u8,
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
    pressed_keys: std::collections::HashMap<BrowserKey, PressedBrowserKey>,
    /// App-owned shortcuts and browser-local reload stay consumed through
    /// their release even if a later frame has different modifier state.
    suppressed_shortcut_keys: std::collections::HashSet<BrowserKey>,
    /// Copy/Cut pseudo-events synthesize a complete CDP key pair; consume a
    /// later native release, but abandon suppression if a new press starts.
    clipboard_release_keys: std::collections::HashSet<BrowserKey>,
    /// A non-empty IME preedit remains active across frames until commit or
    /// dismissal. While active, printable key events must not guess Latin
    /// text ahead of the authoritative IME commit.
    ime_composing: bool,
    /// Escape exits panel fullscreen after the app has already cleared the
    /// fullscreen flag, so remember the preceding frame for input filtering.
    fullscreen_active_last_frame: bool,
    host_focus_requested_at: Option<f64>,
}

impl BrowserUiState {
    /// Reset render and input caches when a different backend takes ownership
    /// of this panel. Returns whether a reset occurred.
    fn synchronize_backend(&mut self, backend: BackendKind) -> bool {
        if self.active_backend == Some(backend) {
            return false;
        }
        *self = Self {
            active_backend: Some(backend),
            ..Self::default()
        };
        true
    }
}

#[derive(Clone, Copy)]
struct BrowserPointerClick {
    button: BrowserButton,
    position: Pos2,
    time: f64,
    count: u32,
}

struct PressedBrowserKey {
    physical_key: Option<BrowserKey>,
    text: Option<String>,
}

pub struct BrowserView<'a> {
    panel: &'a mut Panel,
    ui_state: &'a mut BrowserUiState,
    shortcuts: &'a AppShortcuts,
    fullscreen_active: bool,
    shortcut_bindings: &'a [horizon_core::ShortcutBinding],
    frame_has_pointer_button: bool,
}

impl<'a> BrowserView<'a> {
    #[must_use]
    pub fn new(
        panel: &'a mut Panel,
        ui_state: &'a mut BrowserUiState,
        shortcuts: &'a AppShortcuts,
        fullscreen_active: bool,
        shortcut_bindings: &'a [horizon_core::ShortcutBinding],
        frame_has_pointer_button: bool,
    ) -> BrowserView<'a> {
        Self {
            panel,
            ui_state,
            shortcuts,
            fullscreen_active,
            shortcut_bindings,
            frame_has_pointer_button,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, events: &[Event], is_focused: bool, interactive: bool) -> bool {
        if self.panel.browser().is_none() {
            ui.centered_and_justified(|ui| ui.label("Browser content missing"));
            return false;
        }
        let state = &mut *self.ui_state;
        let panel_id = self.panel.id;
        if let Some(browser) = self.panel.browser() {
            state.synchronize_backend(browser.backend());
        }
        if let Some(browser) = self.panel.browser_mut()
            && let Some(text) = browser.take_clipboard_text()
        {
            ui.ctx().copy_text(text);
        }
        if let Some(browser) = self.panel.browser_mut() {
            if browser.backend() == BackendKind::SafariWebDriver && browser.needs_event_waker() {
                let ctx = ui.ctx().clone();
                browser.set_event_waker(Arc::new(move || ctx.request_repaint()));
            }
            restore_host_focus(ui, state, browser.take_host_focus_request());
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
            // `chrome::show` can switch the backend in this same frame. Drop
            // the old session's viewport/texture/input caches before reading
            // a replacement frame or deciding whether to send its viewport.
            state.synchronize_backend(browser.backend());
            render::show_body(ui, panel_id, browser, state, interactive)
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
        if let Some(body_id) = body.keyboard_focus_id {
            if page_keyboard_active {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(body_id, page_focus_event_filter());
                });
                if let Some(body_rect) = body.image_rect {
                    ime::publish_page_ime_output(ui, body_id, body_rect);
                }
            } else {
                ime::clear_page_ime_state(ui, body_id);
            }
        }
        if let Some(browser) = self.panel.browser_mut() {
            if body.retry_clicked {
                browser.relaunch();
                // The new Chrome starts at its default viewport. Clear every
                // per-session input/render cache so this frame immediately
                // resends the panel's real viewport and cannot carry a held
                // button or key into the replacement session.
                *state = BrowserUiState::default();
            }
            synchronize_viewport(ui, browser, state, &body);
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
                    shortcut_bindings: self.shortcut_bindings,
                    frame_has_pointer_button: self.frame_has_pointer_button,
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

fn restore_host_focus(ui: &Ui, state: &mut BrowserUiState, request: Option<bool>) {
    let now = ui.input(|input| input.time);
    if request == Some(true) {
        state.host_focus_requested_at = None;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        return;
    }
    if request == Some(false) {
        state.host_focus_requested_at = Some(now);
    }
    let Some(requested_at) = state.host_focus_requested_at else {
        return;
    };
    if now - requested_at >= SAFARI_HOST_FOCUS_RECOVERY_SECONDS {
        state.host_focus_requested_at = None;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
    } else {
        ui.ctx().request_repaint_after(Duration::from_millis(10));
    }
}

fn synchronize_viewport(
    ui: &Ui,
    browser: &horizon_core::browser::BrowserPanelState,
    state: &mut BrowserUiState,
    body: &render::BodyOutput,
) {
    // Follow panel resizes/fullscreen with the emulated viewport so responsive
    // layout and backend input geometry match what is on screen. Retry at a
    // bounded rate until a matching frame is actually published; a protocol
    // acknowledgement alone is not proof that input coordinates converged.
    let Some(viewport) = body.viewport_size else {
        return;
    };
    let now = ui.input(|input| input.time);
    let frame_matches = frame_matches_viewport(body.frame_size, Some(viewport), state.last_viewport);
    if frame_matches {
        state.viewport_retry_count = 0;
    }
    if viewport == state.last_viewport
        && !frame_matches
        && state.viewport_retry_count < MAX_VIEWPORT_CONVERGENCE_RETRIES
        && now < state.viewport_retry_at
    {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(state.viewport_retry_at - now));
    }
    if viewport.0 <= 32
        || viewport.1 <= 32
        || !viewport_command_due(
            body.frame_size,
            viewport,
            state.last_viewport,
            now,
            state.viewport_retry_at,
            state.viewport_retry_count,
        )
    {
        return;
    }
    input::cancel_pointer_capture(browser, state, body.image_rect, body.frame_size);
    if browser.try_send(BrowserCommand::SetViewport {
        width: viewport.0,
        height: viewport.1,
    }) {
        if viewport == state.last_viewport {
            state.viewport_retry_count = state.viewport_retry_count.saturating_add(1);
        } else {
            state.viewport_retry_count = 0;
        }
        state.last_viewport = viewport;
        state.viewport_retry_at = now + VIEWPORT_RETRY_INTERVAL_SECONDS;
    }
}

const fn page_focus_event_filter() -> egui::EventFilter {
    egui::EventFilter {
        tab: true,
        horizontal_arrows: true,
        vertical_arrows: true,
        escape: false,
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

fn viewport_command_due(
    frame_size: Option<[f32; 2]>,
    desired_viewport: (u32, u32),
    sent_viewport: (u32, u32),
    now: f64,
    retry_at: f64,
    retry_count: u8,
) -> bool {
    desired_viewport != sent_viewport
        || (retry_count < MAX_VIEWPORT_CONVERGENCE_RETRIES
            && !frame_matches_viewport(frame_size, Some(desired_viewport), sent_viewport)
            && now >= retry_at)
}

#[cfg(test)]
mod tests {
    use egui::pos2;
    use horizon_core::browser::BackendKind;

    use super::{
        BrowserUiState, MAX_VIEWPORT_CONVERGENCE_RETRIES, frame_matches_viewport, page_focus_event_filter,
        page_keyboard_can_route, viewport_command_due,
    };

    #[test]
    fn backend_switch_resets_session_owned_viewport_and_input_caches() {
        let mut state = BrowserUiState::default();
        assert!(state.synchronize_backend(BackendKind::ChromiumCdp));
        state.seq = 42;
        state.last_viewport = (800, 600);
        state.viewport_retry_at = 12.5;
        state.viewport_retry_count = 3;
        state.last_mouse = Some(pos2(12.0, 34.0));
        state.url_buffer = String::from("https://example.test/");
        state.url_submit_enter_pending = true;

        assert!(!state.synchronize_backend(BackendKind::ChromiumCdp));
        assert_eq!(state.seq, 42);
        assert_eq!(state.last_viewport, (800, 600));

        assert!(state.synchronize_backend(BackendKind::FirefoxBidi));
        assert_eq!(state.active_backend, Some(BackendKind::FirefoxBidi));
        assert_eq!(state.seq, 0);
        assert_eq!(state.last_viewport, (0, 0));
        assert!((state.viewport_retry_at - 0.0).abs() < f64::EPSILON);
        assert_eq!(state.viewport_retry_count, 0);
        assert!(state.last_mouse.is_none());
        assert!(state.url_buffer.is_empty());
        assert!(!state.url_submit_enter_pending);
    }

    #[test]
    fn page_focus_keeps_navigation_keys_out_of_egui() {
        let filter = page_focus_event_filter();
        assert!(filter.tab);
        assert!(filter.horizontal_arrows);
        assert!(filter.vertical_arrows);
        assert!(!filter.escape);
    }

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
    fn mismatched_viewport_retries_only_after_the_bounded_interval() {
        assert!(viewport_command_due(
            Some([800.0, 600.0]),
            (900, 600),
            (800, 600),
            1.0,
            2.0,
            0
        ));
        assert!(!viewport_command_due(
            Some([804.0, 600.0]),
            (800, 600),
            (800, 600),
            1.0,
            2.0,
            0
        ));
        assert!(viewport_command_due(
            Some([804.0, 600.0]),
            (800, 600),
            (800, 600),
            2.0,
            2.0,
            0
        ));
        assert!(!viewport_command_due(
            Some([800.0, 600.0]),
            (800, 600),
            (800, 600),
            3.0,
            2.0,
            0
        ));
        assert!(!viewport_command_due(
            Some([804.0, 600.0]),
            (800, 600),
            (800, 600),
            3.0,
            2.0,
            MAX_VIEWPORT_CONVERGENCE_RETRIES
        ));
    }

    #[test]
    fn page_keyboard_requires_window_panel_and_egui_focus() {
        assert!(page_keyboard_can_route(true, false, false));
        assert!(!page_keyboard_can_route(false, false, false));
        assert!(!page_keyboard_can_route(true, true, false));
        assert!(!page_keyboard_can_route(true, false, true));
    }
}
