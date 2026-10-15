//! Deletion of only the exact server already recorded for an operation.
use super::{
    ACTION_TIMEOUT, Acted, Announcement, Cancellation, CloudError, CreateState, Hetzner, Method, Progress, refused,
};
use std::time::Instant;

impl Hetzner {
    /// Deletes only the server recorded for this operation and proves it is gone.
    /// Attached volumes are detached by the provider and kept. The server's running
    /// actions finish first, so a delete during provisioning waits for the volume attach.
    /// # Errors
    /// Refuses unbound or mismatching servers and reports unfinished deletion.
    pub fn delete_server(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        progress: impl FnMut(Progress),
    ) -> Result<(), CloudError> {
        self.delete_server_announced(operation_id, state, cancel, persist, progress, &mut |_| Ok(()))
    }

    /// As [`Self::delete_server`], calling `announce` after the identity check and
    /// immediately before the delete request; nothing is announced for a server
    /// already gone.
    pub(in crate::hetzner) fn delete_server_announced(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
        announce: &mut dyn FnMut(Announcement) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        let recorded = match state {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            _ => return Err(CloudError::CreationUnresolved),
        };
        let id: u64 = recorded.parse().map_err(|_| CloudError::Invalid("Invalid worker ID"))?;
        progress(Progress::ConfirmingWorker);
        if let Some(server) = self.inspect_server(id, cancel)? {
            server.verify(operation_id)?;
            if self.settle_actions(id, Instant::now() + ACTION_TIMEOUT, cancel)? {
                progress(Progress::Terminating);
                announce(Announcement::Before)?;
                match self.send(Method::Delete, &format!("/servers/{id}"), None, cancel) {
                    Ok(value) => {
                        let acted: Acted = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
                        self.wait_until(&acted.action, cancel, || Ok(self.inspect_server(id, cancel)?.is_none()))?;
                    }
                    Err(failure) if failure.not_found() => {}
                    Err(failure) => return Err(refused(failure.into(), announce)),
                }
            }
            progress(Progress::ConfirmingTermination);
            if self.inspect_server(id, cancel)?.is_some() {
                return Err(CloudError::Invalid("Termination pending; reconcile again"));
            }
        }
        let next = CreateState::Terminated { worker_id: recorded };
        persist(&next)?;
        *state = next;
        Ok(())
    }
}
