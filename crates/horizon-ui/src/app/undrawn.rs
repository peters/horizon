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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use egui::Context;
use horizon_core::Wake;

use super::HorizonApp;

/// The wait for the read of the agent states, which a worker thread makes.
const READ_TAKES: Duration = Duration::from_millis(100);

/// The wait for the hook of an agent, which writes the state of a turn after
/// the last output of that turn. It is longer than the time between two reads.
const HOOK_TAKES: Duration = Duration::from_millis(600);

/// What the application knows about the frames of its window.
#[derive(Default)]
pub(in crate::app) struct Undrawn {
    /// The count of the frames and the time of the input, as they were when
    /// the last frame took the output of the panels.
    frame: Option<(u64, u64)>,
    /// Whether the last pass drew nothing. Then an event of a terminal asks for
    /// the next pass, because no poll of a frame reads it.
    asks: Arc<AtomicBool>,
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
    /// window draws nothing: the poll of a frame reads it sooner.
    pub(in crate::app) fn wake(&self, ctx: &Context) -> horizon_core::OutputWake {
        let (ctx, asks) = (ctx.clone(), Arc::clone(&self.asks));
        Arc::new(move |wake| {
            if wake == Wake::Echo || asks.load(Ordering::Acquire) {
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
