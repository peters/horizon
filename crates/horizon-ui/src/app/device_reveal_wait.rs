//! Reveal answers once the host has drawn the viewer, or when a bounded wait
//! expires with the observation that explains why it was not drawn.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{
    PanelId,
    browser::manifest::{
        self,
        device::{Connection, Operation, Outcome, Request},
    },
};

use super::{HorizonApp, device_requests::complete_device_request};

/// A held reveal's answer and the queue it goes to.
pub(super) struct RevealAnswer {
    root: Option<PathBuf>,
    pub(super) request: Request,
    pub(super) outcome: Outcome,
}

impl RevealAnswer {
    fn new(waiting: AwaitingDeviceReveal, outcome: Outcome) -> Self {
        Self {
            root: waiting.root,
            request: waiting.request,
            outcome,
        }
    }
}

/// Long enough for a restored window and a detached viewport to draw; well
/// inside the MCP bound, which ends five seconds after the dispatch deadline.
const REVEAL_SETTLE: Duration = Duration::from_secs(3);
const REVEAL_RESULT_MARGIN_MILLIS: i64 = 4_000;

pub(super) struct AwaitingDeviceReveal {
    /// Queue the request came from; its answer is published there.
    root: Option<PathBuf>,
    request: Request,
    id: PanelId,
    reveal: u64,
    deadline: Instant,
}

impl HorizonApp {
    /// Holds a successful reveal's result instead of answering with the
    /// frame before the reveal was applied. Returns the outcome to publish now.
    pub(super) fn defer_device_reveal(
        &mut self,
        request: &Request,
        outcome: Outcome,
        root: Option<&Path>,
    ) -> Option<Outcome> {
        let (Operation::Reveal { panel_id }, Outcome::Panels { panels }) = (&request.operation, &outcome) else {
            return Some(outcome);
        };
        // Waiting cannot draw a stopped or failed connection; answer without
        // depending on another frame.
        if panels.iter().any(|panel| !awaits_frames(&panel.connection)) {
            return Some(outcome);
        }
        // A closing or switching host answers now; it will not draw the viewer.
        if self.shutdown_progress.is_some() || self.pending_session_switch.is_some() {
            return Some(outcome);
        }
        let Some(id) = self.board.panel_id_by_local_id(panel_id) else {
            return Some(outcome);
        };
        let Some(reveal) = self
            .panel_render_caches
            .device_ui_state
            .get(&id)
            .map(|state| state.host.reveal_requests())
        else {
            return Some(outcome);
        };
        let remaining = request
            .deadline_at_millis
            .saturating_add(REVEAL_RESULT_MARGIN_MILLIS)
            .saturating_sub(manifest::now_millis());
        let bound = Duration::from_millis(u64::try_from(remaining).unwrap_or(0)).min(REVEAL_SETTLE);
        self.panel_render_caches
            .awaiting_device_reveals
            .push(AwaitingDeviceReveal {
                root: root.map(Path::to_path_buf),
                request: request.clone(),
                id,
                reveal,
                deadline: Instant::now() + bound,
            });
        None
    }

    pub(super) fn holds_device_reveals(&self) -> bool {
        !self.panel_render_caches.awaiting_device_reveals.is_empty()
    }

    /// Runs after `finish_frame`, so observations describe the completed pass.
    pub(super) fn complete_settled_device_reveals(&mut self, ctx: &Context) {
        if !self.holds_device_reveals() {
            return;
        }
        publish(self.take_settled_device_reveals(Instant::now()));
        if self.holds_device_reveals() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    /// The request pump's path while the host runs no frames: expiry and
    /// closed viewers still answer, from the last completed pass.
    pub(super) fn settle_device_reveals_without_frame(&mut self) {
        if self.holds_device_reveals() {
            publish(self.take_settled_device_reveals(Instant::now()));
        }
    }

    /// Answers every held reveal before a session switch renumbers panels or
    /// the host exits, instead of letting its caller time out.
    pub(super) fn abandon_device_reveals(&mut self, message: &str) {
        publish(self.take_abandoned_device_reveals(message));
    }

    pub(super) fn take_abandoned_device_reveals(&mut self, message: &str) -> Vec<RevealAnswer> {
        std::mem::take(&mut self.panel_render_caches.awaiting_device_reveals)
            .into_iter()
            .map(|waiting| RevealAnswer::new(waiting, Outcome::failed("panel_unavailable", message)))
            .collect()
    }

    pub(super) fn take_settled_device_reveals(&mut self, now: Instant) -> Vec<RevealAnswer> {
        let awaiting = std::mem::take(&mut self.panel_render_caches.awaiting_device_reveals);
        let mut settled = Vec::new();
        for waiting in awaiting {
            match self.settled_device_reveal(&waiting, now) {
                Some(outcome) => settled.push(RevealAnswer::new(waiting, outcome)),
                None => self.panel_render_caches.awaiting_device_reveals.push(waiting),
            }
        }
        settled
    }

    fn settled_device_reveal(&mut self, waiting: &AwaitingDeviceReveal, now: Instant) -> Option<Outcome> {
        // Viewer state is dropped with the panel or on shutdown; never answer
        // with a freshly defaulted, stopped stand-in.
        if !self.panel_render_caches.device_ui_state.contains_key(&waiting.id) {
            return Some(Outcome::failed("panel_unavailable", "Device panel closed"));
        }
        let Some(panel) = self.device_observation(waiting.id, &waiting.request.actor) else {
            return Some(Outcome::failed("panel_unavailable", "Device panel closed"));
        };
        let displayed = panel.image.image_displayed
            && self
                .panel_render_caches
                .device_ui_state
                .get(&waiting.id)
                .is_some_and(|state| state.displayed_since_reveal(waiting.reveal));
        // Waiting cannot produce an image for a stopped or failed connection,
        // nor for a reveal that was superseded or dropped before reaching the canvas.
        let unreachable = !awaits_frames(&panel.connection);
        let dropped = !self.device_reveal_reached_or_queued(waiting.id, waiting.reveal);
        (displayed || unreachable || dropped || !panel.visible || now >= waiting.deadline)
            .then(|| Outcome::Panels { panels: vec![panel] })
    }

    fn device_reveal_reached_or_queued(&self, id: PanelId, reveal: u64) -> bool {
        let applied = self
            .panel_render_caches
            .device_ui_state
            .get(&id)
            .is_some_and(|state| state.host.applied_at(reveal).is_some());
        applied
            || self
                .panel_render_caches
                .pending_device_reveal
                .as_ref()
                .is_some_and(|pending| pending.targets(id))
            || self
                .detached_workspaces
                .values()
                .any(|state| state.pending_device_reveal == Some(id))
    }
}

fn awaits_frames(connection: &Connection) -> bool {
    !matches!(connection, Connection::Stopped | Connection::Disconnected)
}

fn publish(answers: Vec<RevealAnswer>) {
    for answer in answers {
        if let Err(error) = complete_device_request(answer.root.as_deref(), &answer.request, answer.outcome) {
            tracing::warn!(%error, "could not publish Device panel result");
        }
    }
}
