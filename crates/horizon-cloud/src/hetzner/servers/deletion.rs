//! Deletion of only the exact server already recorded for an operation.
use super::{
    ACTION_TIMEOUT, Acted, Announcement, Cancellation, CloudError, CreateState, Hetzner, Method, Progress, refused,
};
use std::time::Instant;

/// Same-live-client observation of an original bound server's verified identity
/// followed by absence. It is not serializable, reconstructible or a credential.
/// Only `cloud::delete_observed` exposes it after all remaining cleanup succeeds.
///
/// Callers cannot construct an observation from saved identifiers:
/// ```compile_fail
/// use horizon_cloud::hetzner::servers::OriginalServerAbsence;
/// let _ = OriginalServerAbsence { operation_id: "op-1".into(), server_id: 42 };
/// ```
#[derive(Debug)]
pub struct OriginalServerAbsence {
    operation_id: String,
    server_id: u64,
}

impl OriginalServerAbsence {
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn server_id(&self) -> u64 {
        self.server_id
    }
}

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
        persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        progress: impl FnMut(Progress),
        announce: &mut dyn FnMut(Announcement) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        self.delete_server_observed(operation_id, state, cancel, persist, progress, announce)
            .map(drop)
    }

    /// Internal observation: the same immutable client performs every identity,
    /// delete and absence request. A fresh 404 or Terminated retry is not proof.
    pub(in crate::hetzner) fn delete_server_observed(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
        announce: &mut dyn FnMut(Announcement) -> Result<(), CloudError>,
    ) -> Result<Option<OriginalServerAbsence>, CloudError> {
        let originally_bound = matches!(state, CreateState::Bound { .. });
        let recorded = match state {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            _ => return Err(CloudError::CreationUnresolved),
        };
        let id: u64 = recorded.parse().map_err(|_| CloudError::Invalid("Invalid worker ID"))?;
        let mut identified = false;
        progress(Progress::ConfirmingWorker);
        if let Some(server) = self.inspect_server(id, cancel)? {
            server.verify(operation_id)?;
            identified = originally_bound && id != 0 && id.to_string() == recorded;
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
        Ok(identified.then(|| OriginalServerAbsence {
            operation_id: operation_id.to_owned(),
            server_id: id,
        }))
    }
}
