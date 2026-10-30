//! Host dispatch for exact-allocation recovery after a panel disappears. The
//! claim of the agents' requests, the completion of each answer and the owner
//! read of each remote panel's manifest take coordination locks and write
//! private files, so they run on the coordination worker; a later frame
//! answers what the claim took.
use super::{HorizonApp, browser_requests::actor_panel};
use horizon_core::browser::RemoteRecoveryStatus;
use horizon_core::browser::manifest::{
    self,
    recovery::{RecoveryQueue, RecoveryRequest, RecoveryResult},
};

impl HorizonApp {
    pub(super) fn poll_remote_recovery(&mut self) -> bool {
        self.poll_remote_recovery_queue(&RecoveryQueue::default())
    }

    fn poll_remote_recovery_queue(&mut self, queue: &RecoveryQueue) -> bool {
        self.refresh_remote_recovery_scope();
        let mut changed = self.browser_create_host.remote_allocations.poll();
        self.claim_recovery_requests(queue);
        // A request that waited for its allocation to establish release is
        // answered once the board can answer it.
        changed |= self.answer_recovery_requests(queue);
        changed
    }

    /// Claims this host's recovery requests on the coordination worker; the
    /// frame that applies the claim answers them. One claim runs at a time, so
    /// a slow disk never queues one per tick, and a claim that starts after a
    /// session switch replaced the board takes nothing.
    fn claim_recovery_requests(&mut self, queue: &RecoveryQueue) {
        if self.browser_create_host.recovery_claiming {
            return;
        }
        self.browser_create_host.recovery_claiming = true;
        let (claiming, answering) = (queue.clone(), queue.clone());
        let host = manifest::host_instance().to_string();
        let board = self.browser_create_host.io.board_fence();
        self.browser_create_host.io.then(
            move || board.holds().then(|| claim_recovery(&claiming, &host)),
            move |app, claimed| {
                app.browser_create_host.recovery_claiming = false;
                if let Some(requests) = claimed.flatten() {
                    app.dispatch_recovery_requests(&answering, requests);
                }
            },
        );
    }

    /// Starts the reconciliation of each request the claim took, then answers
    /// every request this board holds.
    fn dispatch_recovery_requests(&mut self, queue: &RecoveryQueue, requests: Vec<RecoveryRequest>) {
        for request in &requests {
            if let Some(reference) = request.reference.as_deref()
                && request.deadline_at_millis >= manifest::now_millis()
                && let Some(workspace) = self.recovery_workspace(request)
            {
                let _ = self
                    .browser_create_host
                    .remote_allocations
                    .reconcile(reference, Some((&request.actor, &workspace)));
            }
        }
        self.browser_create_host.recovery_requests.extend(requests);
        self.answer_recovery_requests(queue);
    }

    /// Answers the claimed requests this board holds from the states in memory,
    /// on the coordination worker. A request whose allocation is still
    /// reconciling waits for a later frame; every other request is answered
    /// once, and an enqueued answer counts as answered rather than as a failed
    /// write, because the worker reports a queue it cannot write.
    fn answer_recovery_requests(&mut self, queue: &RecoveryQueue) -> bool {
        let mut changed = false;
        let requests = std::mem::take(&mut self.browser_create_host.recovery_requests);
        for request in requests {
            let workspace = self.recovery_workspace(&request);
            let expired = request.deadline_at_millis < manifest::now_millis();
            let mut allocations = workspace.as_deref().map_or_else(Vec::new, |workspace| {
                self.browser_create_host
                    .remote_allocations
                    .summaries(Some((&request.actor, workspace)))
            });
            if let Some(reference) = &request.reference {
                allocations.retain(|a| &a.reference == reference);
            }
            let error = if expired {
                Some("request_expired")
            } else if workspace.is_none() || (request.reference.is_some() && allocations.is_empty()) {
                Some("allocation_unavailable")
            } else {
                None
            };
            if error.is_none()
                && allocations
                    .iter()
                    .any(|a| a.status == RemoteRecoveryStatus::Reconciling)
                && request.reference.is_some()
            {
                self.browser_create_host.recovery_requests.push(request);
                continue;
            }
            if error.is_some() {
                allocations.clear();
            }
            self.answer_recovery(queue, request.result(allocations, error.map(str::to_string)));
            changed = true;
        }
        changed
    }

    /// Publishes the board's answer to one request on the coordination worker.
    /// An answer is written once: a later one for the same request finds no
    /// request file and writes nothing.
    fn answer_recovery(&mut self, queue: &RecoveryQueue, result: RecoveryResult) {
        let queue = queue.clone();
        self.browser_create_host.io.write(move || {
            if let Err(error) = queue.complete(&result) {
                tracing::warn!(
                    request_id = %result.request_id,
                    %error,
                    "could not answer a remote recovery request"
                );
            }
        });
    }

    fn recovery_workspace(&self, request: &RecoveryRequest) -> Option<String> {
        if request.host_instance != manifest::host_instance() {
            return None;
        }
        let actor = actor_panel(&self.board, &request.actor)?;
        self.board
            .workspaces
            .iter()
            .find(|w| w.id == actor.workspace_id)
            .map(|w| w.local_id.clone())
    }

    pub(super) fn panel_remote_allocation(&self, local_id: &str) -> Option<&horizon_core::browser::RemoteAllocation> {
        self.board
            .panels
            .iter()
            .find(|panel| panel.local_id == local_id)?
            .browser()?
            .remote_allocation()
    }

    /// Reads, on the coordination worker, the owner each remote panel's
    /// manifest holds, so a retired allocation keeps the scope the manifest
    /// recorded. The reads stay in order after the stamps of this tick.
    pub(super) fn refresh_remote_recovery_scope(&mut self) {
        self.restamp_browser_manifests_for_placement();
        let scopes: Vec<RemotePanelScope> = self
            .board
            .panels
            .iter()
            .filter_map(|panel| {
                let allocation = panel
                    .browser()
                    .and_then(horizon_core::browser::BrowserPanelState::remote_allocation)?;
                let workspace = self.board.workspaces.iter().find(|w| w.id == panel.workspace_id)?;
                Some(RemotePanelScope {
                    local_id: panel.local_id.clone(),
                    allocation: allocation.clone(),
                    workspace: workspace.local_id.clone(),
                })
            })
            .collect();
        // One read at a time: this runs on the tick and on every close, so a
        // slow worker must not take one read of every remote panel per call.
        if scopes.is_empty() || self.browser_create_host.scope_reading {
            return;
        }
        self.browser_create_host.scope_reading = true;
        self.browser_create_host.io.then(
            move || read_remote_panel_scopes(scopes, manifest::read),
            |app, owners| {
                app.browser_create_host.scope_reading = false;
                // A board that was replaced in between keeps its own scopes.
                if app.browser_create_host.io.applying_stale() {
                    return;
                }
                app.apply_remote_recovery_scopes(owners.unwrap_or_default());
            },
        );
    }

    /// Gives each remote allocation the scope that the read of its manifest
    /// recorded, also when the read found no owner: a cleared owner clears the
    /// record, which then authorizes no agent.
    fn apply_remote_recovery_scopes(&mut self, read: Vec<(RemotePanelScope, Option<String>)>) {
        for (scope, owner) in read {
            self.browser_create_host.remote_allocations.update_scope(
                &scope.allocation,
                &scope.workspace,
                owner.as_deref(),
            );
        }
    }
}

/// The owner that the manifest of each remote panel holds, in board order.
///
/// A read that fails is left out, so the record keeps the scope that the last
/// good read recorded and the read is repeated on a later tick. A read that
/// succeeds reports the owner it found, also when it found none, so a manifest
/// whose owner was cleared clears the record.
fn read_remote_panel_scopes(
    scopes: Vec<RemotePanelScope>,
    mut read: impl FnMut(&str) -> Option<manifest::BrowserManifest>,
) -> Vec<(RemotePanelScope, Option<String>)> {
    scopes
        .into_iter()
        .filter_map(|scope| {
            let manifest = read(&scope.local_id)?;
            Some((scope, manifest.owner.map(|owner| owner.name)))
        })
        .collect()
}

/// A remote panel whose manifest owner the host keeps: the answer of the panel
/// allocation expects the workspace the board places it in.
struct RemotePanelScope {
    local_id: String,
    allocation: horizon_core::browser::RemoteAllocation,
    workspace: String,
}

/// The requests the claim took, or none with a warning: a queue that cannot be
/// read leaves its requests for a later tick.
fn claim_recovery(queue: &RecoveryQueue, host: &str) -> Vec<RecoveryRequest> {
    match queue.claim(host) {
        Ok(requests) => requests,
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), "could not poll remote recovery requests");
            Vec::new()
        }
    }
}

/// A remote allocation a stamp covers, and the placement the stamp writes.
pub(super) struct StampedAllocation {
    pub(super) local_id: String,
    pub(super) allocation: horizon_core::browser::RemoteAllocation,
    workspace: String,
}

impl StampedAllocation {
    /// A panel moved and then closed before its stamp landed: its driver
    /// kept the scope of the manifest as it was, which names the workspace
    /// the panel left, while the allocation expects the one it moved to, so
    /// no workspace could recover it. It keeps the scope this stamp would
    /// have written, as when the stamp lands first: the driver's scope moves
    /// to that workspace and keeps the owner the driver read. A scope a failed
    /// stamp invalidated stays invalid.
    fn keep_stamped_scope(&self) {
        self.allocation.move_retained_scope(&self.workspace);
    }
}

/// The allocations whose manifest is gone under `root`, read on the
/// coordination worker after a stamp. The driver removes a manifest only
/// after it kept its scope.
pub(super) fn retired_allocations(root: &std::path::Path, stamped: Vec<StampedAllocation>) -> Vec<StampedAllocation> {
    stamped
        .into_iter()
        .filter(|stamped| !manifest::manifest_path_for_root(root, &stamped.local_id).exists())
        .collect()
}

impl HorizonApp {
    /// The remote allocation of a panel a stamp places in `workspace`. It
    /// expects that workspace from now on, before any file is touched.
    pub(super) fn stamped_allocation(&self, local_id: &str, workspace: &str) -> Option<StampedAllocation> {
        let allocation = self.panel_remote_allocation(local_id)?;
        allocation.expect_workspace(workspace);
        Some(StampedAllocation {
            local_id: local_id.to_string(),
            allocation: allocation.clone(),
            workspace: workspace.to_string(),
        })
    }

    /// Gives each retired allocation whose panel closed the scope its stamp wrote.
    pub(super) fn keep_stamped_scopes(&self, retired: &[StampedAllocation]) {
        for stamped in retired {
            if self.board.panel_id_by_local_id(&stamped.local_id).is_none() {
                stamped.keep_stamped_scope();
            }
        }
    }
}

#[cfg(test)]
mod tests;
