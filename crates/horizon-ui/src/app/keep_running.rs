//! What a close of the main window does, and the ask that comes before it.
//!
//! The setting `features.when_window_closes` names one of three: Horizon asks
//! (the default), keeps running behind a hidden window, or quits. A kept window
//! ends nothing: the terminals, the agent programs, the link to a server and the
//! shared terminals go on, and the board of the session is saved at the hide. A
//! workspace window of its own closes as it did.
//!
//! The window is hidden with [`egui::ViewportCommand::Visible`], which egui-winit
//! passes to `set_visible` of winit 0.30.13. On X11 that unmaps the window and
//! maps it again with `XMapRaised` to show it; on macOS it orders the window out
//! and in; on Wayland `set_visible` does nothing, so there a close ends Horizon
//! and asks nothing.
//!
//! A hidden window draws no frame, so this work is a frame's work: the close
//! request of a window that the desktop shows, and the pass that a hidden window
//! runs without a frame ([`super::undrawn`]). The ask counts what runs once, when
//! the close request comes, and no frame of a window without an ask pays for it.

use std::time::Duration;

use egui::{Context, ViewportCommand};
use horizon_core::notify::Notice;
use horizon_core::{RunningPrograms, WindowCloseSetting};

use super::HorizonApp;

/// The wait between two passes of a hidden window. A lease that is 15 s old is a
/// window that ended, and a second start of this computer finds the live window
/// through it, so a hidden window writes its heartbeat this often.
const LEASE_KEEPALIVE: Duration = Duration::from_secs(2);

/// The one notice of a run: Horizon still runs, how to show it and how to end it.
const NOTICE: &str = "Horizon keeps running. Start Horizon again to show its window, and choose Quit Horizon in its \
                      menu to end it.";

/// What the person answered the ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum CloseAnswer {
    /// Hide the window and keep every program.
    KeepRunning,
    /// End the programs and the window.
    Quit,
    /// Change nothing.
    Cancel,
}

/// The ask that waits for the person's answer.
pub(in crate::app) struct CloseAsk {
    /// What runs, counted when the ask came, so no frame of it counts again.
    pub(in crate::app) running: RunningPrograms,
    /// Whether `Quit Horizon` in the menu asked, rather than a close request.
    pub(in crate::app) from_menu: bool,
    /// Whether the box `Remember this choice` is checked.
    pub(in crate::app) remember: bool,
}

impl CloseAsk {
    fn of(running: RunningPrograms, from_menu: bool) -> Self {
        Self {
            running,
            from_menu,
            remember: false,
        }
    }
}

/// What this run knows about the close of its window, and about a hidden window.
#[derive(Default)]
pub(in crate::app) struct KeepRunning {
    /// Whether the main window is hidden now.
    pub(in crate::app) hidden: bool,
    /// The ask of the window, while the person owes an answer.
    pub(in crate::app) ask: Option<CloseAsk>,
    /// Whether the notice of the first hide went out in this run.
    told: bool,
}

impl HorizonApp {
    /// The choice of the setting, for a platform that can hide the window: a
    /// toolkit without a hidden window ends Horizon instead of asking.
    fn close_choice(&self) -> WindowCloseSetting {
        if self.platform_hides_windows() {
            self.template_config.features.when_window_closes
        } else {
            WindowCloseSetting::Quit
        }
    }

    /// Whether a start of this computer can show the window of this run again.
    pub(in crate::app) fn window_stays_open(&self) -> bool {
        self.close_choice() != WindowCloseSetting::Quit
    }

    /// What runs inside this Horizon now.
    pub(in crate::app) fn running_programs(&self) -> RunningPrograms {
        self.board
            .running_programs(self.template_config.features.keep_agents_through_restart)
    }

    /// A close request of the main window: it hides, it ends, or it asks first.
    pub(in crate::app) fn close_main_window(&mut self, ctx: &Context) {
        // A second close request while the ask shows changes nothing, and a close
        // that repeated its ask would count the programs again.
        if self.keep_running.ask.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            return;
        }
        if self.close_choice() == WindowCloseSetting::KeepRunning {
            self.hide_main_window(ctx);
            return;
        }
        // Keep the viewport alive while this frame asks, or ends the panels.
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        let running = self.running_programs();
        if self.close_choice() == WindowCloseSetting::Ask && !running.is_empty() {
            self.keep_running.ask = Some(CloseAsk::of(running, false));
        } else {
            self.begin_shutdown();
        }
    }

    /// `Quit Horizon` in the menu: it asks when a program runs, and ends Horizon
    /// at once when none runs.
    pub(in crate::app) fn request_quit(&mut self) {
        let running = self.running_programs();
        if running.is_empty() {
            self.begin_shutdown();
        } else {
            self.keep_running.ask = Some(CloseAsk::of(running, true));
        }
    }

    /// Answers the ask: the box writes the choice for the closes that follow, and
    /// the answer hides the window, ends Horizon, or keeps everything.
    pub(in crate::app) fn answer_close_ask(&mut self, ctx: &Context, answer: CloseAnswer) {
        let Some(ask) = self.keep_running.ask.take() else {
            return;
        };
        let remembered = match answer {
            CloseAnswer::KeepRunning => WindowCloseSetting::KeepRunning,
            CloseAnswer::Quit => WindowCloseSetting::Quit,
            // The box never writes a choice that the person did not make.
            CloseAnswer::Cancel => WindowCloseSetting::Ask,
        };
        if ask.remember && answer != CloseAnswer::Cancel {
            let _ = self.persist_config_change("features.when_window_closes", |config| {
                config.features.when_window_closes = remembered;
            });
        }
        match answer {
            CloseAnswer::KeepRunning => self.hide_main_window(ctx),
            CloseAnswer::Quit => self.begin_shutdown(),
            CloseAnswer::Cancel => {}
        }
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

    /// Hides the main window and keeps everything of this Horizon running.
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

    /// The heartbeat of a hidden window, whose lease a second start reads.
    pub(in crate::app) fn keep_lease_of_hidden_window(&mut self, ctx: &Context) {
        self.refresh_active_session_lease();
        ctx.request_repaint_after(LEASE_KEEPALIVE);
    }
}

mod ask;

#[cfg(test)]
mod tests;
