//! Pinned, non-creating inspection of an explicitly nominated retained repository pack.

#[cfg(target_os = "linux")]
mod protocol;

use crate::{
    cloud_run::{ArtifactDigest, CloudWorkflowStore, GitCommitSha},
    remote_worker_status::RemotePanelStatusError,
    remote_workspace_recovery::RecoveredRemoteWorkspace,
};

/// Expected existing remote input, not source-export, upload or setup approval.
#[derive(Clone, Copy)]
pub struct RemotePackExpectation<'a> {
    pub path: &'a str,
    pub base_commit: &'a GitCommitSha,
    pub sha256: &'a ArtifactDigest,
    pub encoded_bytes: u64,
}

/// A point-in-time identity observation, not immutable publication, synchronization
/// or a ready checkout. Its path and ancestry must remain stable until consumption.
pub struct RemotePackObservation {
    path: String,
    objects_directory: String,
    base_commit: GitCommitSha,
    sha256: ArtifactDigest,
    encoded_bytes: u64,
    objects: u32,
}

impl RemotePackObservation {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
    #[must_use]
    pub fn objects_directory(&self) -> &str {
        &self.objects_directory
    }
    #[must_use]
    pub fn base_commit(&self) -> &GitCommitSha {
        &self.base_commit
    }
    #[must_use]
    pub fn sha256(&self) -> &ArtifactDigest {
        &self.sha256
    }
    #[must_use]
    pub fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }
    #[must_use]
    pub fn objects(&self) -> u32 {
        self.objects
    }
}

impl std::fmt::Debug for RemotePackObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RemotePackObservation").finish_non_exhaustive()
    }
}

/// Inspect one existing candidate after owned recovery, without uploading, creating,
/// adopting, repairing, deleting or starting anything. Both complete allocation
/// snapshots, retained worker/key/host and lifetime are checked before and after I/O.
/// The exact base must agree with the saved workspace. Caller-owned provider/cost
/// admission and source/namespace trust remain separate. Run off the render thread.
/// Local query teardown does not stop the remote worker or its independent tasks.
/// # Errors
/// Rejects unsupported platforms, stale ownership, pending management, unavailable
/// retained identity, invalid expectations, failed/timed-out SSH or invalid output.
/// Failure is an unknown observation, never absence or a grant to retry setup/upload.
pub fn inspect_remote_repository_pack(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    expected: RemotePackExpectation<'_>,
) -> Result<RemotePackObservation, RemotePackInspectionError> {
    #[cfg(target_os = "linux")]
    {
        inspect_with(store, recovered, expected, query)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, recovered, expected);
        Err(RemotePackInspectionError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn inspect_with(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    expected: RemotePackExpectation<'_>,
    execute: impl FnOnce(
        &crate::remote_ssh_identity::RemoteSshIdentity,
        &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
        &[u8],
    ) -> Result<Vec<u8>, RemotePackInspectionError>,
) -> Result<RemotePackObservation, RemotePackInspectionError> {
    use crate::remote_worker_inspection::validate_current;
    let endpoint = validate_current(store, recovered, None)?;
    if expected.base_commit != &recovered.allocation().workspace().state().spec.repository.commit {
        return Err(RemotePackInspectionError::InvalidRequest);
    }
    let request = protocol::request(expected)?;
    let response = execute(recovered.identity(), endpoint, &request)?;
    let observation = protocol::response(&response, expected)?;
    validate_current(store, recovered, None)?;
    Ok(observation)
}

#[cfg(target_os = "linux")]
fn query(
    identity: &crate::remote_ssh_identity::RemoteSshIdentity,
    endpoint: &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    input: &[u8],
) -> Result<Vec<u8>, RemotePackInspectionError> {
    use crate::remote_worker_ssh::{known_hosts, prepared_pack_status, query};
    // Verification has two separately bounded native Git phases plus bounded
    // encoded input hashing; a deadline does not imply the candidate is absent.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_pack_status(identity.private_key_path(), trust.path(), endpoint)?;
    query::run(command, input, DEADLINE, protocol::RESPONSE_LIMIT).map_err(|error| match error {
        query::Error::ClientUnavailable => RemotePanelStatusError::ClientUnavailable.into(),
        query::Error::QueryFailed => RemotePackInspectionError::QueryFailed,
        query::Error::Deadline => RemotePackInspectionError::Deadline,
        query::Error::OutputLimit => RemotePackInspectionError::InvalidResponse,
    })
}

/// Diagnostics never include candidate paths, identities or remote subprocess data.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemotePackInspectionError {
    #[error(transparent)]
    Admission(#[from] RemotePanelStatusError),
    #[error("remote pack expectation is invalid or does not match the owned repository base")]
    InvalidRequest,
    #[error("remote pack inspection failed; retain input and do not replay setup or upload")]
    QueryFailed,
    #[error("remote pack inspection exceeded its deadline; the candidate state is unknown")]
    Deadline,
    #[error("remote pack observation is invalid or exceeds its size limit")]
    InvalidResponse,
    #[error("protected remote pack inspection is not yet supported on this platform")]
    UnsupportedPlatform,
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
