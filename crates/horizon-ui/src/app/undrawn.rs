//! The pass of a window that draws no frame.
//!
//! eframe calls `App::logic` before each frame, and alone when a repaint was
//! asked for and the window draws nothing: the window is minimized, or on
//! Wayland the compositor sends no frame callback to a surface that it does
//! not show. A repaint request of any thread leads to that pass, so the work
//! that needs no drawing runs there with no thread and no timer of its own.
//! While frames run, each frame does this work, and the pass before a frame
//! returns at once. Nothing in the pass draws, measures text, reads the
//! pointer or waits.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use egui::Context;
use horizon_core::Wake;

use super::HorizonApp;

/// The wait for the read of the agent states, which a worker thread makes.
const READ_TAKES: Duration = Duration::from_millis(100);

/// The wait for the hook of an agent, which writes the state of a turn after
/// the last output of that turn. It is longer than the time between two reads.
const HOOK_TAKES: Duration = Duration::from_millis(600);

/// A call for the pass books this many milliseconds, and no call goes out
/// while the book is that far ahead of the clock. So eight calls go at once,
/// and about sixty a second after them: the frames of a window that draws.
const CALL_MS: i64 = 16;
const AHEAD_MS: i64 = 8 * CALL_MS;

/// The way from the wake of a terminal to the pass with no wait for eframe,
/// which runs the pass of a repaint request 250 ms late on Wayland.
#[derive(Default)]
struct AtOnce {
    /// The call of the bridge of the window, which asks the UI thread.
    call: OnceLock<Arc<dyn Fn() + Send + Sync>>,
    /// The time up to which the calls are booked, in milliseconds.
    booked: AtomicI64,
}

impl AtOnce {
    /// Asks the UI thread for the pass now, unless the window has no call or
    /// its calls are booked too far ahead: eframe then runs the pass.
    fn ask(&self) -> bool {
        let now = horizon_core::current_unix_millis();
        let booked = self.booked.load(Ordering::Relaxed).max(now);
        let free = booked - now < AHEAD_MS;
        self.booked.store(booked + i64::from(free) * CALL_MS, Ordering::Relaxed);
        self.call.get().filter(|_| free).inspect(|call| call()).is_some()
    }
}

/// What the application knows about the frames of its window.
#[derive(Default)]
pub(in crate::app) struct Undrawn {
    /// The count of the frames and the time of the input, as they were when
    /// the last frame took the output of the panels.
    frame: Option<(u64, u64)>,
    /// Whether the last pass drew nothing. Then an event of a terminal asks for
    /// the next pass, because no poll of a frame reads it.
    asks: Arc<AtomicBool>,
    at_once: Arc<AtOnce>,
    /// Whether the last pass asked for the pass that takes the read of the
    /// agent states, so one read brings one more pass and no chain of them.
    follows: bool,
}

fn mark(ctx: &Context) -> (u64, u64) {
    (ctx.cumulative_frame_nr(), ctx.input(|input| input.time).to_bits())
}

impl Undrawn {
    /// The callback of the terminals of this window. The echo of a key press
    /// asks for a frame. An event of a program asks for a pass only while the
    /// window draws nothing: the UI thread at once, or else eframe.
    pub(in crate::app) fn wake(&self, ctx: &Context) -> horizon_core::OutputWake {
        let (ctx, asks, at_once) = (ctx.clone(), Arc::clone(&self.asks), Arc::clone(&self.at_once));
        Arc::new(move |wake| {
            let undrawn = wake == Wake::Event && asks.load(Ordering::Acquire);
            if wake == Wake::Echo || (undrawn && !at_once.ask()) {
                ctx.request_repaint();
            }
        })
    }

    /// A frame takes the output of the panels now.
    pub(in crate::app) fn note_frame(&mut self, ctx: &Context) {
        self.frame = Some(mark(ctx));
        self.asks.store(false, Ordering::Release);
    }

    /// Whether the pass that runs now draws nothing. A frame that starts takes
    /// new input, and so a new time. A pass that draws nothing keeps the input
    /// of the last frame, and that frame is complete, which a later pass of the
    /// same frame is not.
    fn draws_nothing(&self, ctx: &Context) -> bool {
        self.frame.is_some_and(|(frames, time)| {
            let (now_frames, now_time) = mark(ctx);
            now_frames != frames && now_time == time
        })
    }
}

impl HorizonApp {
    /// Takes the call that asks the UI thread for [`Self::pass_now`].
    pub(in crate::app) fn pass_at_once(&self, call: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.undrawn.at_once.call.set(call);
    }

    /// The pass that a terminal asked for at once, unless a frame ran since.
    pub(in crate::app) fn pass_now(&mut self, ctx: &Context) {
        if self.undrawn.asks.load(Ordering::Acquire) {
            self.logic_pass(ctx);
        }
    }

    /// The pass of eframe before a frame and in place of one. It does the work
    /// only in place of one: the answers of the terminals go out, a panel that
    /// a request opened starts its program, the other computers get the new
    /// list of the shared terminals, and the states of the agent panels change.
    pub(in crate::app) fn logic_pass(&mut self, ctx: &Context) {
        let stops = self.shutdown_progress.is_some() || self.pending_session_switch.is_some();
        if stops || !self.undrawn.draws_nothing(ctx) {
            return;
        }
        self.undrawn.asks.store(true, Ordering::Release);
        // A terminal that a request started or started again has no wake yet.
        self.board.attach_output_wake(&self.output_wake);
        self.queue_panel_starts();
        self.apply_browser_host_io();
        // A hidden window gets no frame, so the request of a second start that
        // shows it runs here.
        self.poll_folder_requests(ctx);
        if self.keep_running.hidden {
            self.keep_lease_of_hidden_window(ctx);
        }
        let output = self.board.process_unread_output();
        if output.activity.terminal {
            // One more pass reads the agent states after the output stopped.
            ctx.request_repaint_after(HOOK_TAKES);
        }
        self.note_panel_output(output);
        self.poll_shared_terminals(ctx);
        // The read of the agent states wakes no pass, so the pass that asked
        // for one asks for the pass that takes it.
        self.refresh_attention(ctx);
        let follows = self.attention.reads() && !self.undrawn.follows;
        self.undrawn.follows = follows;
        if follows {
            ctx.request_repaint_after(READ_TAKES);
        }
    }
}

#[cfg(test)]
pub(in crate::app) mod tests;
