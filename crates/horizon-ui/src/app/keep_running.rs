//! The close of the main window of a Horizon that keeps running.
//!
//! With the setting `features.keep_running_when_closed` on, a close request of
//! the main window hides that window and ends nothing: the terminals, the agent
//! programs, the link to a server and the shared terminals go on. The state that
//! a close saves is saved at the hide, so a computer that stops while Horizon is
//! hidden loses nothing. A workspace window of its own closes as it did.
//!
//! The window is hidden with [`egui::ViewportCommand::Visible`], which egui-winit
//! passes to `set_visible` of winit 0.30.13. On X11 that unmaps the window and
//! maps it again with `XMapRaised` to show it; on macOS it orders the window out
//! and in; on Wayland `set_visible` does nothing, so there the close ends
//! Horizon as it did.
//!
//! A hidden window draws no frame, so this work is a frame's work: the close
//! request of a window that the desktop shows, and the pass that a hidden window
//! runs without a frame ([`super::undrawn`]).

use std::time::Duration;

use egui::{Context, ViewportCommand};
use horizon_core::notify::Notice;

use super::HorizonApp;

/// The wait between two passes of a hidden window. A lease that is 15 s old is a
/// window that ended, and a second start of this computer finds the live window
/// through it, so a hidden window writes its heartbeat this often.
const LEASE_KEEPALIVE: Duration = Duration::from_secs(2);

/// The one notice of a run: Horizon still runs, how to show it and how to end it.
const NOTICE: &str = "Horizon keeps running. Start Horizon again to show its window, and choose Quit Horizon in its \
                      menu to end it.";

/// What this run knows about a hidden main window.
#[derive(Default)]
pub(in crate::app) struct KeepRunning {
    /// Whether the main window is hidden now.
    pub(in crate::app) hidden: bool,
    /// Whether the notice of the first hide went out in this run.
    told: bool,
}

impl HorizonApp {
    /// Whether a close of the main window hides it: the setting says so, and
    /// this platform can hide and show a window.
    pub(in crate::app) fn hides_the_window_on_close(&self) -> bool {
        self.template_config.features.keep_running_when_closed && self.platform_hides_windows()
    }

    /// Whether the toolkit of this window can hide and show its window.
    #[cfg(target_os = "linux")]
    fn platform_hides_windows(&self) -> bool {
        // Wayland has no way to hide a window, and winit does nothing there.
        !self.observed_keyboard_inputs.is_wayland_backend()
    }

    #[cfg(not(target_os = "linux"))]
    const fn platform_hides_windows(&self) -> bool {
        true
    }

    /// Hides the main window and keeps everything of this Horizon running, for a
    /// close request that the desktop sent.
    pub(in crate::app) fn hide_main_window(&mut self, ctx: &Context) {
        // The state of a close, without the end of the panels: the next start of
        // this computer finds the board as the person left it.
        let _ = self.drain_panel_output();
        let _ = self.auto_save_runtime_state();
        self.queue_editor_saves();
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        // eframe ends the process of a close that no pass cancels.
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        self.keep_running.hidden = true;
        if !self.keep_running.told {
            self.keep_running.told = true;
            self.notifications.show(Notice::new(NOTICE));
        }
        // A hidden window gets a pass only when one is asked for, so the window
        // asks for the pass that writes the heartbeat of its lease.
        self.keep_lease_of_hidden_window(ctx);
    }

    /// Shows the main window again and raises it, for a second start of this
    /// computer: of `horizon`, and of `horizon <folder>`.
    pub(in crate::app) fn show_main_window(&mut self, ctx: &Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        self.keep_running.hidden = false;
    }

    /// The heartbeat of a hidden window: a lease that is 15 s old says that the
    /// window ended, and a second start then opens a second Horizon.
    pub(in crate::app) fn keep_lease_of_hidden_window(&mut self, ctx: &Context) {
        self.refresh_active_session_lease();
        ctx.request_repaint_after(LEASE_KEEPALIVE);
    }
}

#[cfg(test)]
mod tests;
