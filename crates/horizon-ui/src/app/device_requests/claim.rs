//! The claim of the agents' Device panel requests, and the answer that Horizon
//! writes for each request a claim took. A claim takes the queue's lock and
//! removes the request files it takes, and an answer writes and syncs a result
//! file, so both run on the coordination worker: a slow disk never stalls a
//! frame or a request-pump wake. A claim removes the request file, so a
//! request it took is kept here until this host answers it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use horizon_core::browser::manifest::{
    self,
    device::{self, Outcome, Request},
};

use super::super::HorizonApp;
use super::super::answer_retries::{RefusedAnswer, RefusedAnswers};
use super::super::browser_request_claims::HOST_RETIRING;

/// The refusal of a request whose deadline passed before Horizon applied it.
pub(super) fn request_expired() -> Outcome {
    Outcome::failed("request_expired", "Device panel request expired before dispatch")
}

/// A request a claim took. Its file is gone, so this host keeps the request
/// until it answers it.
pub(in crate::app) struct ClaimedRequest {
    /// The queue the request came from; its answer is published there.
    pub(in crate::app) root: Option<PathBuf>,
    pub(in crate::app) request: Request,
    /// The manifest a browser capture judges, read by the claim job so no frame
    /// reads it. A request that captures no browser panel carries none.
    pub(in crate::app) browser_manifest: Option<manifest::BrowserManifest>,
}

/// One answer to a claimed request, and the queue it goes to.
pub(in crate::app) struct DeviceAnswer {
    root: Option<PathBuf>,
    request: Request,
    outcome: Outcome,
}

impl ClaimedRequest {
    /// Refuses this request now, instead of applying it.
    fn refused(&self, outcome: Outcome) -> DeviceAnswer {
        DeviceAnswer {
            root: self.root.clone(),
            request: self.request.clone(),
            outcome,
        }
    }
}

impl DeviceAnswer {
    pub(in crate::app) fn new(root: Option<&Path>, request: Request, outcome: Outcome) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            request,
            outcome,
        }
    }
}

/// What this host owes the agents whose Device requests a claim took.
#[derive(Default)]
pub(in crate::app) struct DeviceClaimState {
    /// A claim of this host's requests runs on the coordination worker.
    pub(in crate::app) claiming: bool,
    /// Requests a claim took that no frame applied yet, in claim order.
    pub(in crate::app) claimed: Vec<ClaimedRequest>,
    /// Answers queued on the coordination worker whose outcome did not apply.
    writing: usize,
    /// Answers the worker could not write, for a later frame or pump wake and
    /// until each request's deadline.
    refused: RefusedAnswers<DeviceAnswer>,
}

impl DeviceClaimState {
    /// Whether a claim runs, a request it took waits, or an answer is not yet
    /// written: each needs a later frame or pump wake to settle.
    fn outstanding(&self) -> bool {
        self.claiming || !self.claimed.is_empty() || self.writing > 0 || !self.refused.is_empty()
    }
}

impl HorizonApp {
    /// Claims this host's waiting Device requests on the coordination worker,
    /// at most every [`super::DEVICE_REQUEST_POLL`]. One claim runs at a time,
    /// so a slow disk never queues one per tick; a claim that starts after a
    /// session switch replaced the board takes nothing, and a later frame or
    /// pump wake applies what it took.
    pub(in crate::app) fn claim_device_requests(&mut self, root: Option<&Path>) {
        let now = Instant::now();
        if self.device_claim.claiming
            || self
                .panel_render_caches
                .device_request_poll
                .is_some_and(|last| now.duration_since(last) < super::DEVICE_REQUEST_POLL)
        {
            return;
        }
        self.panel_render_caches.device_request_poll = Some(now);
        self.device_claim.claiming = true;
        let claiming = root.map(Path::to_path_buf);
        let board = self.browser_create_host.io.board_fence();
        self.browser_create_host.io.then(
            move || board.holds().then(|| claim_requests(claiming.as_deref())),
            |app, claimed| app.apply_device_claim(claimed.flatten()),
        );
    }

    /// Keeps each request the claim took, in claim order, or refuses it at once
    /// when no frame will apply it: the board it was sent to was replaced, or
    /// is retiring for an exit or a session switch.
    fn apply_device_claim(&mut self, claimed: Option<Vec<ClaimedRequest>>) {
        self.device_claim.claiming = false;
        let Some(claimed) = claimed else { return };
        let retiring = self.browser_host_retiring();
        for request in claimed {
            if retiring {
                self.write_device_answer(request.refused(Outcome::failed(HOST_RETIRING.0, HOST_RETIRING.1)));
            } else {
                self.device_claim.claimed.push(request);
            }
        }
    }

    /// Writes one answer on the coordination worker. A write that fails, or a
    /// job that panics, keeps the answer: the request file is gone, so this
    /// host writes the same result again on a later frame or pump wake until
    /// the request's deadline. Two writes of one result envelope answer the
    /// same request once, because the agent removes the result it takes.
    pub(in crate::app) fn write_device_answer(&mut self, answer: DeviceAnswer) {
        let request_id = answer.request.request_id.clone();
        let deadline_at_millis = answer.request.deadline_at_millis;
        let writing = answer.request.clone();
        let (root, outcome) = (answer.root.clone(), answer.outcome.clone());
        self.device_claim.writing += 1;
        self.browser_create_host.io.then(
            move || complete_device_request(root.as_deref(), &writing, outcome),
            move |app, written| {
                app.device_claim.writing -= 1;
                if matches!(written, Some(Ok(()))) {
                    return;
                }
                tracing::debug!(%request_id, "the coordination worker could not answer a Device panel request yet");
                app.device_claim
                    .refused
                    .keep(RefusedAnswer::new(answer, request_id, deadline_at_millis));
            },
        );
    }

    /// Writes the answers the worker could not write again, at most every
    /// `RETRY_INTERVAL` and until each request's deadline. An answer whose
    /// deadline passed is dropped: the agent has its own timeout by then.
    pub(in crate::app) fn retry_device_answers(&mut self) -> bool {
        let due = self.device_claim.refused.due(manifest::now_millis());
        let retried = !due.is_empty();
        for answer in due {
            self.write_device_answer(answer.result);
        }
        retried
    }

    /// Answers every request a claim took that no frame applied, because an
    /// exit or a session switch ends the board that would. Each request is
    /// answered once, before the exit flushes the coordination worker: with
    /// `host_shutdown`, or with `request_expired` when its deadline passed.
    pub(in crate::app) fn refuse_claimed_device_requests(&mut self) {
        for request in std::mem::take(&mut self.device_claim.claimed) {
            let refusal = if request.request.deadline_at_millis <= manifest::now_millis() {
                request_expired()
            } else {
                Outcome::failed(HOST_RETIRING.0, HOST_RETIRING.1)
            };
            self.write_device_answer(request.refused(refusal));
        }
    }

    /// Whether this host still owes an answer to a Device request a claim took.
    /// The request watcher keeps waking while it does, so each answer lands
    /// also when the host runs no frame.
    pub(in crate::app) fn device_claims_outstanding(&self) -> bool {
        self.device_claim.outstanding()
    }
}

/// The requests this host claims, in queue order. A queue that cannot be read
/// leaves its requests for a later tick.
fn claim_requests(root: Option<&Path>) -> Vec<ClaimedRequest> {
    let host = manifest::host_instance();
    let claimed = match root {
        Some(root) => device::claim_at(root, host),
        None => device::claim(host),
    };
    match claimed {
        Ok(requests) => requests
            .into_iter()
            .map(|request| {
                let browser_manifest = claimed_browser_manifest(root, &request);
                ClaimedRequest {
                    root: root.map(Path::to_path_buf),
                    request,
                    browser_manifest,
                }
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "could not poll Device panel requests");
            Vec::new()
        }
    }
}

/// The manifest that judges a browser capture, read in the claim job so no
/// frame reads it. A read that fails is no manifest, which the apply refuses as
/// `browser_unavailable`; a manifest of another panel is refused as
/// `panel_unavailable`.
pub(in crate::app) fn claimed_browser_manifest(
    root: Option<&Path>,
    request: &Request,
) -> Option<manifest::BrowserManifest> {
    let device::Operation::BrowserScreenshot { input } = &request.operation else {
        return None;
    };
    match root {
        Some(root) => manifest::read_at(&manifest::manifest_path_for_root(root, &input.panel_id)),
        None => manifest::read(&input.panel_id),
    }
}

/// Writes the result envelope of one request.
fn complete_device_request(root: Option<&Path>, request: &Request, outcome: Outcome) -> std::io::Result<()> {
    match root {
        Some(root) => device::complete_at(root, request, outcome),
        None => device::complete(request, outcome),
    }
}
