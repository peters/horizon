//! egui → CDP input coordination for the focused browser panel.

mod keyboard;
mod pointer;

use egui::{Event, Ui};
use horizon_core::AppShortcuts;
use horizon_core::browser::BrowserPanelState;

use crate::browser_widget::BrowserUiState;

pub(super) use pointer::cancel_pointer_capture;

/// Focus/interaction flags for one frame.
#[derive(Clone, Copy)]
pub(crate) struct InputFlags<'a> {
    pub(crate) events: &'a [Event],
    pub(crate) interactive: bool,
    pub(crate) keyboard_target: KeyboardTarget,
    pub(crate) pointer_viewport: PointerViewportState,
    pub(crate) shortcuts: &'a AppShortcuts,
    /// App shortcuts this viewport actually dispatches; the browser only
    /// suppresses presses it would otherwise swallow without an effect.
    pub(crate) shortcut_bindings: &'a [horizon_core::ShortcutBinding],
    /// Frame-level hint: any pointer-button event in this viewport's event
    /// slice, used to skip the per-panel pointer scan for panels that
    /// cannot consume it.
    pub(crate) frame_has_pointer_button: bool,
    pub(crate) exit_fullscreen_shortcut_owner: ShortcutOwner,
}

#[derive(Clone, Copy)]
pub(crate) enum KeyboardTarget {
    None,
    Url,
    Page,
}

#[derive(Clone, Copy)]
pub(crate) enum PointerViewportState {
    AwaitingFrame,
    Ready,
}

#[derive(Clone, Copy)]
pub(crate) enum ShortcutOwner {
    App,
    Page,
}

pub fn handle(
    ui: &mut Ui,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    body: Option<egui::Rect>,
    frame_size: Option<[f32; 2]>,
    pointer_target: bool,
    flags: InputFlags<'_>,
) {
    let page_keyboard_active = flags.interactive && matches!(flags.keyboard_target, KeyboardTarget::Page);
    if !page_keyboard_active {
        keyboard::release_pressed_keys(browser, state, keyboard::key_modifiers(ui));
        state.ime_composing = false;
    }
    if !flags.interactive || matches!(flags.keyboard_target, KeyboardTarget::None) {
        keyboard::clear_focus_lost_suppressions(state);
    }
    if !flags.interactive {
        pointer::cancel_pointer_capture(browser, state, body, frame_size);
        state.pointer_modifiers = keyboard::key_modifiers(ui);
        return;
    }
    if matches!(flags.pointer_viewport, PointerViewportState::Ready)
        && let (Some(rect), Some(frame_size)) = (body, frame_size)
    {
        pointer::events(
            ui,
            flags.events,
            browser,
            state,
            pointer::PointerFrame {
                rect,
                frame_size,
                pointer_target,
                frame_has_pointer_button: flags.frame_has_pointer_button,
            },
        );
    } else {
        state.pointer_modifiers = keyboard::key_modifiers(ui);
    }
    if matches!(flags.keyboard_target, KeyboardTarget::Url) {
        keyboard::browser_shortcut_events(
            flags.events,
            browser,
            state,
            flags.shortcuts,
            flags.shortcut_bindings,
            matches!(flags.exit_fullscreen_shortcut_owner, ShortcutOwner::App),
        );
    } else if page_keyboard_active {
        let exit_fullscreen_shortcut_active = matches!(flags.exit_fullscreen_shortcut_owner, ShortcutOwner::App);
        keyboard::events(
            ui,
            flags.events,
            browser,
            state,
            flags.shortcuts,
            flags.shortcut_bindings,
            exit_fullscreen_shortcut_active,
        );
    }
}
