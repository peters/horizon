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

impl HorizonApp {
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
    /// board since its request was claimed gets a typed refusal.
    fn apply_claimed_requests(&mut self, claimed: ClaimedRequests) {
        self.browser_create_host.claiming = false;
        for request in claimed.creates {
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
        for request in claimed.visibility {
            match actor_panel(&self.board, &request.actor) {
                Some(actor_panel) => self.apply_browser_visibility_request(&request, actor_panel),
                None => self.fail_visibility(&request, "workspace_unavailable", AGENT_GONE),
            }
        }
        for request in claimed.closes {
            match actor_panel(&self.board, &request.actor) {
                Some(actor_panel) => self.apply_browser_close_request(&request, actor_panel),
                None => self.fail_close(&request, "workspace_unavailable", AGENT_GONE),
            }
        }
    }
}

/// The requests a claim took for this host's agents, in queue order.
struct ClaimedRequests {
    creates: Vec<BrowserCreateRequest>,
    visibility: Vec<BrowserVisibilityRequest>,
    closes: Vec<BrowserCloseRequest>,
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

/// The requests of one queue that are `ours` and that this host claimed.
fn claim_queue<R>(
    kind: &str,
    listed: std::io::Result<Vec<R>>,
    ours: impl Fn(&R) -> bool,
    claim: impl Fn(&R) -> (std::io::Result<Option<R>>, &String),
) -> Vec<R> {
    let listed = listed.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not poll browser {kind} requests");
        Vec::new()
    });
    listed
        .iter()
        .filter(|request| ours(request))
        .filter_map(|request| match claim(request) {
            (Ok(claimed), _) => claimed,
            (Err(error), request_id) => {
                tracing::warn!(%request_id, %error, "could not claim browser {kind} request");
                None
            }
        })
        .collect()
}
