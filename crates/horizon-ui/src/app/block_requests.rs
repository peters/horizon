//! Host-side handling of the agents' card requests.
//!
//! A card is agent-authored text drawn in a person's workspace, so the host
//! checks the caller, its workspace, the owner and the document itself before
//! it changes a panel. The claim of the queue runs on the coordination worker
//! and the apply on the UI thread, as the Device panel requests do: claiming
//! takes a file lock and writing an answer syncs a file.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::HorizonApp;
use super::answer_retries::{RefusedAnswer, RefusedAnswers};
use super::device_requests::HOST_REQUEST_POLL;
use egui::Context;
use horizon_browser_control::block::{BlockDocument, parse_sent_document};
use horizon_core::Panel;
use horizon_core::browser::manifest::{
    self,
    block::{self, Outcome, Request},
};

mod caller;
mod cards;
mod dispatch;
pub(in crate::app) mod work_items;
pub(in crate::app) mod workers;

pub(in crate::app) use self::caller::CardCaller;
pub(super) use self::caller::{caller_unavailable, caller_without_panel, orchestrator_needs_machine};

/// How often a frame comes back while a prompt line waits for the program of its
/// panel: the line goes out on the frame after that program draws.
const PROMPT_REPAINT: std::time::Duration = std::time::Duration::from_millis(100);

/// A request a claim took. Its file is gone, so this host keeps the request
/// until it answers it.
pub(in crate::app) struct ClaimedBlockRequest {
    /// The queue the request came from; its answer is published there.
    pub(in crate::app) root: Option<PathBuf>,
    pub(in crate::app) request: Request,
}

/// One answer to a claimed request, and the queue it goes to.
pub(in crate::app) struct BlockAnswer {
    root: Option<PathBuf>,
    request: Request,
    outcome: Outcome,
}

impl BlockAnswer {
    pub(in crate::app) fn new(root: Option<&Path>, request: Request, outcome: Outcome) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            request,
            outcome,
        }
    }
}

/// What this host owes the agents whose card requests a claim took.
#[derive(Default)]
pub(in crate::app) struct BlockClaimState {
    /// A claim of this host's requests runs on the coordination worker.
    pub(in crate::app) claiming: bool,
    /// Requests a claim took that no frame applied yet, in claim order.
    pub(in crate::app) claimed: Vec<ClaimedBlockRequest>,
    /// Answers queued on the coordination worker whose outcome did not apply.
    writing: usize,
    /// Jobs of the coordination worker whose answer no frame wrote yet. Such a
    /// job reads a file or starts `ssh-keygen`, so the frame that queued it
    /// answers nothing, and the request pump must keep waking for the answer.
    worker_jobs: usize,
    /// Answers the worker could not write, for a later frame or pump wake and
    /// until each request's deadline.
    refused: RefusedAnswers<BlockAnswer>,
}

impl BlockClaimState {
    /// Whether a claim runs, a request it took waits, or an answer is not yet
    /// written: each needs a later frame or pump wake to settle.
    fn outstanding(&self) -> bool {
        self.claiming
            || !self.claimed.is_empty()
            || self.writing > 0
            || self.worker_jobs > 0
            || !self.refused.is_empty()
    }

    /// Counts one worker job whose answer a later wake writes.
    pub(in crate::app) fn worker_job_started(&mut self) {
        self.worker_jobs += 1;
    }

    /// Counts one worker job that answered, or that ended before it answered.
    pub(in crate::app) fn worker_job_finished(&mut self) {
        self.worker_jobs = self.worker_jobs.saturating_sub(1);
    }
}

/// The document a request carries, as the host draws it: the value itself, or
/// the JSON text of it.
fn card_document(document: &serde_json::Value) -> Result<BlockDocument, Outcome> {
    parse_sent_document(document).map_err(|error| Outcome::failed(error.code(), &error.to_string()))
}

impl HorizonApp {
    /// The frame poll. See [`Self::drain_block_panel_requests`].
    pub(super) fn poll_block_panel_requests(&mut self, ctx: &Context) -> bool {
        self.drain_block_panel_requests(ctx, None)
    }

    /// Applies each request a claim took, answers it, retries the answers the
    /// coordination worker could not write, and asks that worker for this
    /// host's waiting requests. `root` selects an explicit queue directory;
    /// `None` uses the process runtime root.
    ///
    /// The unpresented host's request pump calls it on each wake, where it is
    /// the only path that applies a request and no later frame is guaranteed.
    /// Returns whether a request applied.
    pub(super) fn drain_block_panel_requests(&mut self, ctx: &Context, root: Option<&Path>) -> bool {
        let changed = self.drain_claimed_block_requests(ctx);
        self.claim_block_requests(root);
        changed
    }

    /// Applies each request a claim took, in claim order, and answers it. A
    /// `next_click` that waits for a click is applied here too: the host keeps
    /// it and answers it when a click arrives or its deadline passes.
    fn drain_claimed_block_requests(&mut self, ctx: &Context) -> bool {
        // The outcome of a claim arrives on the coordination worker. A frame
        // that also drains the Device requests has drained this queue already.
        self.apply_browser_host_io();
        // Click waits settle before a request applies: a click that waits reaches
        // the wait it was made for, and the counts a request reports are fresh.
        let settled = self.settle_block_clicks();
        // The one line that a request asked Horizon to type into a new panel goes
        // out when that program has drawn; until then a frame keeps coming.
        let prompts = self.write_ready_prompts();
        if self.agent_opens.panels.holds_prompts() {
            ctx.request_repaint_after(PROMPT_REPAINT);
        }
        // A card of Horizon that asks about a setting answers at its deadline or
        // when the person closes it without a press.
        let settings = self.settle_setting_requests(ctx);
        let claimed = std::mem::take(&mut self.block_claim.claimed);
        let changed = !claimed.is_empty();
        for request in claimed {
            let outcome = self.apply_block_request_at(&request.request, request.root.as_deref(), ctx);
            if let Some(outcome) = outcome {
                self.write_block_answer(BlockAnswer::new(request.root.as_deref(), request.request, outcome));
            }
        }
        changed | self.retry_block_answers() | settled | settings | prompts
    }

    /// Applies a request against the process runtime root, as a claim's apply
    /// does. Tests use it to judge one operation without a queue.
    #[cfg(test)]
    pub(super) fn apply_block_request(&mut self, request: &Request, ctx: &Context) -> Outcome {
        self.apply_block_request_at(request, None, ctx)
            .expect("the request answers at once")
    }

    /// Applies one request that the host keeps open, without a queue: a test of a
    /// wait judges the card and writes the answer it reads itself.
    #[cfg(test)]
    pub(super) fn apply_block_request_waiting(&mut self, request: &Request, ctx: &Context) -> Option<Outcome> {
        self.apply_block_request_at(request, None, ctx)
    }

    /// Applies one request against an explicit queue directory, as a test does
    /// for an operation whose answer the worker writes later. `None` means no
    /// frame will answer it: the coordination worker owes the answer.
    #[cfg(test)]
    pub(super) fn apply_block_request_in(&mut self, request: &Request, root: &Path, ctx: &Context) -> Option<Outcome> {
        self.apply_block_request_at(request, Some(root), ctx)
    }

    /// Claims this host's waiting card requests on the coordination worker, at
    /// most every [`HOST_REQUEST_POLL`]. One claim runs at a time, so a slow
    /// disk never queues one per tick.
    pub(in crate::app) fn claim_block_requests(&mut self, root: Option<&Path>) {
        let now = Instant::now();
        if self.block_claim.claiming
            || self
                .panel_render_caches
                .block_request_poll
                .is_some_and(|last| now.duration_since(last) < HOST_REQUEST_POLL)
        {
            return;
        }
        self.panel_render_caches.block_request_poll = Some(now);
        self.block_claim.claiming = true;
        let claiming = root.map(Path::to_path_buf);
        let board = self.browser_create_host.io.board_fence();
        self.browser_create_host.io.then(
            move || board.holds().then(|| claim_requests(claiming.as_deref())),
            |app, claimed| app.apply_block_claim(claimed.flatten()),
        );
    }

    /// Keeps each request the claim took, in claim order, or refuses it at once
    /// when no frame will apply it because the board it was sent to was
    /// replaced.
    fn apply_block_claim(&mut self, claimed: Option<Vec<ClaimedBlockRequest>>) {
        self.block_claim.claiming = false;
        let Some(claimed) = claimed else { return };
        let retiring = self.browser_host_retiring();
        for request in claimed {
            if retiring {
                self.write_block_answer(BlockAnswer::new(
                    request.root.as_deref(),
                    request.request,
                    request_expired(),
                ));
            } else {
                self.block_claim.claimed.push(request);
            }
        }
    }

    /// Writes one answer on the coordination worker. A write that fails, or a
    /// job that panics, keeps the answer: the request file is gone, so this
    /// host writes the same result again on a later frame or pump wake until
    /// the request's deadline.
    pub(in crate::app) fn write_block_answer(&mut self, answer: BlockAnswer) {
        let request_id = answer.request.request_id.clone();
        let deadline_at_millis = answer.request.deadline_at_millis;
        let writing = answer.request.clone();
        let (root, outcome) = (answer.root.clone(), answer.outcome.clone());
        self.block_claim.writing += 1;
        self.browser_create_host.io.then(
            move || complete_block_request(root.as_deref(), &writing, outcome),
            move |app, written| {
                app.block_claim.writing -= 1;
                if matches!(written, Some(Ok(()))) {
                    return;
                }
                tracing::debug!(%request_id, "the coordination worker could not answer a card request yet");
                app.block_claim
                    .refused
                    .keep(RefusedAnswer::new(answer, request_id, deadline_at_millis));
            },
        );
    }

    /// Writes the answers the worker could not write again, at most every
    /// retry interval and until each request's deadline.
    pub(in crate::app) fn retry_block_answers(&mut self) -> bool {
        let due = self.block_claim.refused.due(manifest::now_millis());
        let retried = !due.is_empty();
        for answer in due {
            self.write_block_answer(answer.result);
        }
        retried
    }

    /// Whether this host still owes an answer to a card request a claim took.
    /// The request pump keeps waking while it does, so each answer lands also
    /// when the host runs no frame. A prompt line that waits for the program of
    /// its panel keeps the pump awake for the same reason.
    pub(in crate::app) fn block_claims_outstanding(&self) -> bool {
        self.block_claim.outstanding() || self.agent_opens.panels.holds_prompts()
    }
}

pub(in crate::app) fn card_owner(panel: &Panel) -> Option<&str> {
    panel.block()?.owner_actor.as_deref()
}

/// Answers `secret_names`: the names of the named secrets that a panel of this
/// computer gets when it starts now, in name order. It reads the snapshot that
/// the worker of this start published, so no frame waits for a file and no value
/// is read here at all. The answer holds no value, no length of a value and no
/// hash of one.
fn answer_secret_names() -> Outcome {
    secret_names_outcome(&horizon_core::named_secrets::published_names())
}

/// The answer of `secret_names` for the names that this process published.
fn secret_names_outcome(names: &[horizon_core::named_secrets::SecretName]) -> Outcome {
    Outcome::SecretNames {
        names: names.iter().map(|name| name.as_str().to_owned()).collect(),
    }
}

/// The refusal of a request whose deadline passed before Horizon applied it.
fn request_expired() -> Outcome {
    Outcome::failed("request_expired", "Block panel request expired before dispatch")
}

/// The requests this host claims, in queue order. A queue that cannot be read
/// leaves its requests for a later tick.
fn claim_requests(root: Option<&Path>) -> Vec<ClaimedBlockRequest> {
    let host = manifest::host_instance();
    let claimed = match root {
        Some(root) => block::claim_at(root, host),
        None => block::claim(host),
    };
    match claimed {
        Ok(requests) => requests
            .into_iter()
            .map(|request| ClaimedBlockRequest {
                root: root.map(Path::to_path_buf),
                request,
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "could not poll block panel requests");
            Vec::new()
        }
    }
}

/// Writes the result envelope of one request.
fn complete_block_request(root: Option<&Path>, request: &Request, outcome: Outcome) -> std::io::Result<()> {
    match root {
        Some(root) => block::complete_at(root, request, outcome),
        None => block::complete(request, outcome),
    }
}

#[cfg(test)]
mod tests;
