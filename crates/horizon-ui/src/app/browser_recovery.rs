//! Host dispatch for exact-allocation recovery after a panel disappears.
use super::{HorizonApp, browser_requests::actor_panel};
use horizon_core::browser::RemoteRecoveryStatus;
use horizon_core::browser::manifest::{
    self,
    recovery::{RecoveryQueue, RecoveryRequest},
};

impl HorizonApp {
    pub(super) fn poll_remote_recovery(&mut self) -> bool {
        self.poll_remote_recovery_queue(&RecoveryQueue::default())
    }

    fn poll_remote_recovery_queue(&mut self, queue: &RecoveryQueue) -> bool {
        self.refresh_remote_recovery_scope();
        let mut changed = self.browser_create_host.remote_allocations.poll();
        let requests = match queue.claim(manifest::host_instance()) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(kind = ?error.kind(), "could not poll remote recovery requests");
                Vec::new()
            }
        };
        for request in requests {
            if let Some(reference) = request.reference.as_deref()
                && request.deadline_at_millis >= manifest::now_millis()
                && let Some(workspace) = self.recovery_workspace(&request)
            {
                let _ = self
                    .browser_create_host
                    .remote_allocations
                    .reconcile(reference, Some((&request.actor, &workspace)));
            }
            self.browser_create_host.recovery_requests.push(request);
        }
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
            if queue
                .complete(&request.result(allocations, error.map(str::to_string)))
                .is_err()
            {
                self.browser_create_host.recovery_requests.push(request);
            } else {
                changed = true;
            }
        }
        changed
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

    pub(super) fn refresh_remote_recovery_scope(&mut self) {
        self.restamp_browser_manifests_for_placement();
        for panel in &self.board.panels {
            let Some(allocation) = panel
                .browser()
                .and_then(horizon_core::browser::BrowserPanelState::remote_allocation)
            else {
                continue;
            };
            let Some(workspace) = self.board.workspaces.iter().find(|w| w.id == panel.workspace_id) else {
                continue;
            };
            if let Some(manifest) = manifest::read(&panel.local_id) {
                self.browser_create_host.remote_allocations.update_scope(
                    allocation,
                    &workspace.local_id,
                    manifest.owner.as_ref().map(|owner| owner.name.as_str()),
                );
            }
        }
    }
}

/// A remote allocation a stamp covers, and the placement the stamp writes.
pub(super) struct StampedAllocation {
    pub(super) local_id: String,
    pub(super) allocation: horizon_core::browser::RemoteAllocation,
    workspace: String,
    owner: Option<String>,
}

impl StampedAllocation {
    /// A panel moved and then closed before its stamp landed: its driver
    /// kept the scope of the manifest as it was, which names the workspace
    /// the panel left, while the allocation expects the one it moved to, so
    /// no workspace could recover it. It keeps the scope this stamp would
    /// have written, as when the stamp lands first. A scope a failed stamp
    /// invalidated stays invalid, and without a known owner the driver's
    /// scope stays, which refuses recovery.
    fn keep_stamped_scope(&self) {
        let Some(owner) = &self.owner else { return };
        self.allocation.retain_scope(horizon_browser::RemoteAllocationScope {
            admission_fallback: false,
            host: manifest::host_instance().to_string(),
            workspace: Some(self.workspace.clone()),
            owner: Some(owner.clone()),
        });
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
            owner: self
                .browser_create_host
                .remote_allocations
                .owner(allocation)
                .map(str::to_string),
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
