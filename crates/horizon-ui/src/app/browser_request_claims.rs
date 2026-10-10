//! Claims of the agents' create, visibility and close requests. The claims
//! take the queues' locks and sync the claimed files to disk, so they run on
//! the coordination worker; a later frame starts what each claimed request
//! asks for.

use horizon_core::browser::manifest::{self, BrowserCloseRequest, BrowserCreateRequest, BrowserVisibilityRequest};
use horizon_core::browser_actor;

use super::HorizonApp;
use super::browser_requests::{actor_panel, fail_create, launched_by_this_host};

/// Why a request whose agent left the board since its claim is refused.
pub(super) const AGENT_GONE: &str = "the requesting agent's panel closed before Horizon could apply the request";

/// The refusal of a request whose claim failed inside Horizon, so it may
/// already be marked as claimed: it is answered rather than left waiting.
const CLAIM_FAILED: (&str, &str) = (
    "claim_failed",
    "Horizon could not claim the request; inspect local logs",
);

/// The refusal of a request that reaches the board while Horizon exits or
/// switches sessions.
pub(super) const HOST_RETIRING: (&str, &str) = (
    "host_shutdown",
    "Horizon is exiting or switching sessions; the request was not applied",
);

impl HorizonApp {
    /// Whether the board is being torn down, for an exit or a session switch:
    /// no request applies to it any more.
    pub(super) fn browser_host_retiring(&self) -> bool {
        self.shutdown_progress.is_some() || self.pending_session_switch.is_some() || self.exit_cleanup_complete
    }

    /// Claims the create, visibility and close requests of the agents on this
    /// board on the coordination worker; a later frame applies them. One
    /// claim runs at a time, so a slow disk never queues one per tick.
    pub(super) fn claim_host_requests(&mut self) {
        if self.browser_create_host.claiming {
            return;
        }
        self.browser_create_host.claiming = true;
        let actors: Vec<String> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.kind.is_agent())
            .map(|panel| browser_actor(&panel.local_id))
            .collect();
        self.browser_create_host
            .io
            .then(move || ClaimedRequests::claim(&actors), Self::apply_claimed_requests);
    }

    /// Starts what each claimed request asks for. An agent that left the
    /// board since its request was claimed gets a typed refusal, and so does
    /// every request while Horizon exits or switches sessions.
    fn apply_claimed_requests(&mut self, claimed: Option<ClaimedRequests>) {
        self.browser_create_host.claiming = false;
        let Some(claimed) = claimed else { return };
        for request in &claimed.creates.broken {
            fail_create(
                &mut self.browser_create_host.io,
                request,
                CLAIM_FAILED.0,
                CLAIM_FAILED.1,
            );
        }
        for request in &claimed.visibility.broken {
            self.fail_visibility(request, CLAIM_FAILED.0, CLAIM_FAILED.1);
        }
        for request in &claimed.closes.broken {
            self.fail_close(request, CLAIM_FAILED.0, CLAIM_FAILED.1);
        }
        let (creates, visibility, closes) = (
            claimed.creates.claimed,
            claimed.visibility.claimed,
            claimed.closes.claimed,
        );
        if self.browser_host_retiring() {
            for request in &creates {
                fail_create(
                    &mut self.browser_create_host.io,
                    request,
                    HOST_RETIRING.0,
                    HOST_RETIRING.1,
                );
            }
            for request in &visibility {
                self.fail_visibility(request, HOST_RETIRING.0, HOST_RETIRING.1);
            }
            for request in &closes {
                self.fail_close(request, HOST_RETIRING.0, HOST_RETIRING.1);
            }
            return;
        }
        for request in creates {
            match actor_panel(&self.board, &request.actor) {
                Some(actor_panel) => self.start_requested_browser(request, actor_panel),
                None => fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    "workspace_unavailable",
                    AGENT_GONE,
                ),
            }
        }
        for request in visibility {
            match actor_panel(&self.board, &request.actor) {
                Some(actor_panel) => self.apply_browser_visibility_request(&request, actor_panel),
                None => self.fail_visibility(&request, "workspace_unavailable", AGENT_GONE),
            }
        }
        for request in closes {
            match actor_panel(&self.board, &request.actor) {
                Some(actor_panel) => self.apply_browser_close_request(&request, actor_panel),
                None => self.fail_close(&request, "workspace_unavailable", AGENT_GONE),
            }
        }
    }
}

/// The requests a claim took for this host's agents, in queue order.
struct ClaimedRequests {
    creates: Claimed<BrowserCreateRequest>,
    visibility: Claimed<BrowserVisibilityRequest>,
    closes: Claimed<BrowserCloseRequest>,
}

/// The requests of one queue that this host claimed, and those whose claim
/// panicked: one may be marked claimed already, so each is refused.
struct Claimed<R> {
    claimed: Vec<R>,
    broken: Vec<R>,
}

impl ClaimedRequests {
    /// Claims, on the coordination worker, every request that this host
    /// launched for one of `actors`. The queues are independent: one that
    /// cannot be read (one malformed request is enough) must not stop the
    /// others from being claimed.
    fn claim(actors: &[String]) -> Self {
        let ours =
            |host: Option<&str>, actor: &str| launched_by_this_host(host) && actors.iter().any(|known| known == actor);
        let (host, pid) = (manifest::host_instance(), std::process::id());
        Self {
            creates: claim_queue(
                "create",
                manifest::list_create_requests(),
                |request| ours(request.host_instance.as_deref(), &request.actor),
                |request| {
                    let claimed = manifest::claim_create_request(&request.request_id, &request.actor, host, pid);
                    (claimed, &request.request_id)
                },
            ),
            visibility: claim_queue(
                "visibility",
                manifest::list_visibility_requests(),
                |request| ours(request.host_instance.as_deref(), &request.actor),
                |request| {
                    let claimed = manifest::claim_visibility_request(&request.request_id, &request.actor, host, pid);
                    (claimed, &request.request_id)
                },
            ),
            closes: claim_queue(
                "close",
                manifest::list_close_requests(),
                |request| ours(request.host_instance.as_deref(), &request.actor),
                |request| {
                    let claimed = manifest::claim_close_request(&request.request_id, &request.actor, host, pid);
                    (claimed, &request.request_id)
                },
            ),
        }
    }
}

/// The requests of one queue that are `ours` and that this host claimed. A
/// claim that panics ends only its own request, which is refused, so every
/// request claimed before or after it is still answered.
fn claim_queue<R: Clone>(
    kind: &str,
    listed: std::io::Result<Vec<R>>,
    ours: impl Fn(&R) -> bool,
    claim: impl Fn(&R) -> (std::io::Result<Option<R>>, &String),
) -> Claimed<R> {
    let listed = listed.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not poll browser {kind} requests");
        Vec::new()
    });
    let mut queue = Claimed {
        claimed: Vec::new(),
        broken: Vec::new(),
    };
    for request in listed.iter().filter(|request| ours(request)) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| claim(request))) {
            Ok((Ok(claimed), _)) => queue.claimed.extend(claimed),
            Ok((Err(error), request_id)) => {
                tracing::warn!(%request_id, %error, "could not claim browser {kind} request");
            }
            Err(_) => {
                tracing::error!("claiming a browser {kind} request panicked");
                queue.broken.push(request.clone());
            }
        }
    }
    queue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_that_panics_is_refused_and_the_others_are_still_claimed() {
        let ids: Vec<String> = ["first", "broken", "last"].map(String::from).to_vec();
        let queue = claim_queue(
            "fixture",
            Ok(ids),
            |_| true,
            |request| {
                assert_ne!(request, "broken", "a fixture claim fails");
                (Ok(Some(request.clone())), request)
            },
        );
        assert_eq!(queue.claimed, ["first", "last"]);
        assert_eq!(queue.broken, ["broken"]);
    }
}
