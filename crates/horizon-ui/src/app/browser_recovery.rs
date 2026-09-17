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

    pub(super) fn refresh_remote_recovery_scope(&mut self) {
        self.restamp_browser_manifests_for_placement();
        for panel in self.board.panels.iter().filter(|p| p.browser().is_some()) {
            let Some(workspace) = self.board.workspaces.iter().find(|w| w.id == panel.workspace_id) else {
                continue;
            };
            if let Some(manifest) = manifest::read(&panel.local_id) {
                self.browser_create_host.remote_allocations.update_scope(
                    &panel.local_id,
                    &workspace.local_id,
                    manifest.owner.as_ref().map(|owner| owner.name.as_str()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
