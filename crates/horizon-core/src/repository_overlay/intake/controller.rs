//! Explicit export approval and pinned one-shot handoff; never setup/task authority.
mod input;

use super::{
    ArtifactDigest, BundleState, EncodedIdentity, IntakeError, IntakeRequest, IntakeResponse, IntakeState, PackState,
    RESPONSE_LIMIT, codec,
};
use crate::{
    cloud_run::{CloudWorkflowStore, StoredRemoteAllocation},
    remote_worker_inspection::validate_current,
    remote_worker_ssh::{known_hosts, prepared_intake, query},
    remote_worker_status::RemotePanelStatusError,
    remote_workspace_recovery::RecoveredRemoteWorkspace,
    repository_overlay::{
        bundle::RepositoryOverlayBundle,
        seed::{MAX_OBJECTS, export::PreparedGitPack},
    },
};
use std::{cell::Cell, io::Read, path::Path, time::Duration};

/// Inert proposal borrowing caller-owned inputs. Retain `request` before approval.
/// The caller must keep pack bytes and ancestry stable/exclusive through handoff.
pub struct RepositoryIntakeProposal<'a> {
    allocation: &'a StoredRemoteAllocation,
    path: &'a Path,
    request: IntakeRequest,
    overlay: Vec<u8>,
}

/// Consumed caller assertion, neither cloneable nor deserializable. Hashes, capture,
/// recovery and persistence cannot supply the caller's positive export decision.
pub struct ApprovedRepositoryIntake<'a>(RepositoryIntakeProposal<'a>);

impl<'a> RepositoryIntakeProposal<'a> {
    #[must_use]
    pub fn request(&self) -> &IntakeRequest {
        &self.request
    }

    /// Prepare exact metadata, without filesystem/provider/SSH access or approval.
    /// # Errors
    /// Rejects mismatched full source/base, unavailable identity or invalid limits.
    pub fn new(
        allocation: &'a StoredRemoteAllocation,
        pack: &'a PreparedGitPack,
        bundle: &RepositoryOverlayBundle,
    ) -> Result<Self, ControllerError> {
        if bundle.plan().source() != &allocation.workspace().state().spec.repository
            || pack.base_commit().to_string() != bundle.plan().source().commit.as_str()
        {
            return Err(ControllerError::Request);
        }
        let overlay = codec::encode(bundle).map_err(|_| ControllerError::Request)?.into_vec();
        let request = bound_request(
            allocation,
            EncodedIdentity {
                sha256: pack.sha256().clone(),
                encoded_bytes: pack.encoded_bytes(),
            },
            EncodedIdentity {
                sha256: bundle.manifest_sha256().clone(),
                encoded_bytes: overlay.len() as u64,
            },
        )?;
        Ok(Self {
            allocation,
            path: pack.path(),
            request,
            overlay,
        })
    }

    /// Assert explicit approval for the ENTIRE base closure, including removed files
    /// and commit metadata, and both layers of this one bundle. Not a human signature.
    #[must_use]
    pub fn approve_export(self) -> ApprovedRepositoryIntake<'a> {
        ApprovedRepositoryIntake(self)
    }
}

fn bound_request(
    allocation: &StoredRemoteAllocation,
    pack: EncodedIdentity,
    overlay: EncodedIdentity,
) -> Result<IntakeRequest, ControllerError> {
    let state = allocation.workspace().state();
    let runtime = state.runtime.as_ref().ok_or(ControllerError::Request)?;
    let worker = runtime.worker.as_ref().ok_or(ControllerError::Request)?;
    let key = allocation
        .recovery_request()
        .map_err(|_| ControllerError::Request)?
        .ssh_public_key;
    let endpoint = runtime.ssh.as_ref().ok_or(ControllerError::Request)?;
    if !endpoint.is_complete() {
        return Err(ControllerError::Request);
    }
    let request = IntakeRequest {
        version: 1,
        workspace_local_id: state.spec.workspace_local_id.clone(),
        workflow_id: runtime.workflow_id,
        job_id: runtime.job_id,
        runtime_generation: runtime.generation,
        worker_resource_id: worker.identity.resource_id.clone(),
        client_key_sha256: ArtifactDigest::sha256(key.as_bytes()),
        source: state.spec.repository.clone(),
        pack,
        overlay,
    };
    request.encode().map_err(|_| ControllerError::Request)?;
    Ok(request)
}

/// Consume one approval; failure retains the request and any verified remote response.
/// Run off the UI thread. Blocking reads/spawn/reap are outside the ten-minute pipe
/// deadline. Source mutation may already have disclosed bytes; exclusive ownership
/// is required. No original repository files, automatic retry or cleanup are used.
/// # Errors
/// Any local/remote uncertainty requires observation of this SAME request, not resend.
pub fn send_approved_repository_intake(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    approved: ApprovedRepositoryIntake<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<IntakeResponse, Box<ControllerFailure>> {
    let ApprovedRepositoryIntake(proposal) = approved;
    perform(
        store,
        recovered,
        proposal.request.clone(),
        Some(&proposal),
        &cancelled,
        exchange,
    )
}

/// Observe a retained request without approval, local source reads, initialization or replay.
/// # Errors
/// Missing/invalid response or stale ownership is unknown, never absence.
pub fn observe_repository_intake(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    request: &IntakeRequest,
    cancelled: impl Fn() -> bool,
) -> Result<IntakeResponse, Box<ControllerFailure>> {
    perform(store, recovered, request.clone(), None, &cancelled, exchange)
}

fn perform(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    request: IntakeRequest,
    proposal: Option<&RepositoryIntakeProposal<'_>>,
    cancelled: &dyn Fn() -> bool,
    execute: impl FnOnce(
        &RecoveredRemoteWorkspace,
        bool,
        &mut dyn Read,
        &dyn Fn() -> bool,
    ) -> Result<query::Exchange, ControllerError>,
) -> Result<IntakeResponse, Box<ControllerFailure>> {
    let observed_cancellation = Cell::new(false);
    let track_cancellation = || {
        let current = cancelled();
        observed_cancellation.set(observed_cancellation.get() || current);
        current
    };
    let cancelled: &dyn Fn() -> bool = &track_cancellation;
    let mut response = None;
    let result = (|| {
        validate_current(store, recovered, None)?;
        if proposal.is_some_and(|proposal| proposal.allocation != recovered.allocation())
            || bound_request(recovered.allocation(), request.pack.clone(), request.overlay.clone())? != request
        {
            return Err(ControllerError::Request);
        }
        let encoded = request.encode().map_err(|_| ControllerError::Request)?;
        check_cancel(cancelled)?;
        let mut held = proposal
            .map(|proposal| input::Input::open(proposal.path, &request.pack, cancelled))
            .transpose()?;
        validate_current(store, recovered, None)?;
        let exchange = if let (Some(proposal), Some(held)) = (proposal, &mut held) {
            let prefix = u32::try_from(encoded.len())
                .map_err(|_| ControllerError::Request)?
                .to_le_bytes();
            let mut stream = prefix
                .as_slice()
                .chain(encoded.as_slice())
                .chain((&mut held.file).take(request.pack.encoded_bytes))
                .chain(proposal.overlay.as_slice());
            execute(recovered, false, &mut stream, cancelled)?
        } else {
            execute(recovered, true, &mut encoded.as_slice(), cancelled)?
        };
        response = Some(decode(&exchange, &request, proposal.is_none())?);
        if let Some(held) = held {
            held.verify()?;
        }
        validate_current(store, recovered, None)?;
        check_cancel(cancelled)
    })();
    match (result, response) {
        (Ok(()), Some(response)) if !observed_cancellation.get() => Ok(response),
        (result, response) => Err(Box::new(ControllerFailure {
            request,
            response,
            reason: if observed_cancellation.get() {
                ControllerError::Cancelled
            } else {
                result.err().unwrap_or(ControllerError::Response)
            },
        })),
    }
}

fn exchange(
    recovered: &RecoveredRemoteWorkspace,
    observe: bool,
    input: &mut dyn Read,
    cancelled: &dyn Fn() -> bool,
) -> Result<query::Exchange, ControllerError> {
    let endpoint = recovered
        .observation()
        .and_then(|status| status.ssh.as_ref())
        .ok_or(ControllerError::Request)?;
    let identity = recovered.identity();
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_intake(identity.private_key_path(), trust.path(), endpoint, observe)?;
    query::exchange(
        command,
        input,
        Duration::from_secs(600),
        RESPONSE_LIMIT,
        cancelled,
        None,
    )
    .map_err(|error| query_error(&error))
}

fn query_error(error: &query::Error) -> ControllerError {
    match error {
        query::Error::ClientUnavailable => RemotePanelStatusError::ClientUnavailable.into(),
        query::Error::OutputLimit => ControllerError::Response,
        query::Error::Deadline | query::Error::QueryFailed => ControllerError::Transport,
    }
}

fn decode(
    exchange: &query::Exchange,
    request: &IntakeRequest,
    observe: bool,
) -> Result<IntakeResponse, ControllerError> {
    let invalid = ControllerError::Response;
    if exchange.output.len() > RESPONSE_LIMIT {
        return Err(invalid);
    }
    let response: IntakeResponse = serde_json::from_slice(&exchange.output).map_err(|_| ControllerError::Response)?;
    if response.version != 1
        || exchange.status.code() != Some(i32::from(response.exit_code()))
        || (observe && matches!(response.state, IntakeState::Acknowledged | IntakeState::Unconfirmed))
        || (response.state == IntakeState::Acknowledged && !matches!(exchange.input, query::InputProgress::Complete(_)))
    {
        return Err(invalid);
    }
    let complete = matches!(response.state, IntakeState::Acknowledged | IntakeState::Observed);
    if complete != response.reason.is_none()
        || (response.reason == Some(IntakeError::Invalid)) != (response.state == IntakeState::Rejected)
    {
        return Err(invalid);
    }
    let bound = response.intent_sha256.as_ref()
        == Some(&ArtifactDigest::sha256(
            &request.encode().map_err(|_| ControllerError::Request)?,
        ));
    let roots = response.roots.as_ref();
    let root = Path::new("/workspace/.horizon-worker");
    if bound != roots.is_some()
        || (response.intent_sha256.is_some() && !bound)
        || roots.is_some_and(|roots| {
            roots.packs.as_os_str() != root.join("repository-inputs/packs").as_os_str()
                || roots.bundles.as_os_str() != root.join("repository-inputs/bundles").as_os_str()
                || roots.setup.as_os_str() != root.join("repository-setup").as_os_str()
        })
    {
        return Err(invalid);
    }
    let pack_state = response.pack.as_ref().map(|pack| pack.state);
    if let Some(pack) = &response.pack {
        let roots = roots.ok_or(ControllerError::Response)?;
        let source = pack.source.as_ref().is_some_and(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                path.as_os_str() == roots.packs.join(name).as_os_str()
                    && name.len() <= crate::repository_overlay::checkout::publication::MAX_SIBLING_NAME_BYTES
                    && name.strip_prefix("repository-seed-").is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    })
            })
        });
        let destination = pack
            .destination
            .as_ref()
            .is_some_and(|path| path.as_os_str() == roots.packs.join("base").as_os_str());
        let objects = pack
            .objects
            .is_some_and(|count| count > 0 && u64::from(count) <= MAX_OBJECTS as u64);
        let valid = match pack.state {
            PackState::ReceiveUnconfirmed => {
                (source || pack.source.is_none()) && pack.destination.is_none() && pack.objects.is_none()
            }
            PackState::Unpublished => source && pack.destination.is_none() && objects,
            PackState::RenameUnconfirmed => source && destination && objects,
            _ => pack.source.is_none() && destination && objects,
        };
        if !valid {
            return Err(invalid);
        }
    }
    let observed_progress = pack_state.is_none_or(|state| state == PackState::Observed)
        && response.bundle.is_none_or(|state| state == BundleState::Observed)
        && (response.bundle.is_none() || pack_state.is_some());
    let valid = match response.state {
        IntakeState::Acknowledged => {
            bound && pack_state == Some(PackState::Acknowledged) && response.bundle == Some(BundleState::Acknowledged)
        }
        IntakeState::Observed => {
            bound && pack_state == Some(PackState::Observed) && response.bundle == Some(BundleState::Observed)
        }
        IntakeState::ClaimedUnknown => bound && observed_progress,
        IntakeState::Unconfirmed => {
            bound
                && pack_state != Some(PackState::Observed)
                && response.bundle != Some(BundleState::Observed)
                && (response.bundle.is_none() || pack_state == Some(PackState::Acknowledged))
        }
        IntakeState::Rejected => {
            !bound && response.reason == Some(IntakeError::Invalid) && pack_state.is_none() && response.bundle.is_none()
        }
        IntakeState::Error => {
            bound
                && pack_state.is_none()
                && response.bundle.is_none()
                && response.reason != Some(IntakeError::Unsupported)
        }
        IntakeState::Unsupported => response.reason == Some(IntakeError::Unsupported) && observed_progress,
    };
    valid.then_some(response).ok_or(invalid)
}

fn check_cancel(cancelled: &dyn Fn() -> bool) -> Result<(), ControllerError> {
    if cancelled() {
        Err(ControllerError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct ControllerFailure {
    pub request: IntakeRequest,
    pub response: Option<IntakeResponse>,
    pub reason: ControllerError,
}

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error(transparent)]
    Admission(#[from] RemotePanelStatusError),
    #[error("repository export proposal does not match retained ownership")]
    Request,
    #[error("prepared repository input is unsafe, unavailable or changed")]
    Input,
    #[error("repository intake was cancelled; retain request and do not resend")]
    Cancelled,
    #[error("repository intake transport is uncertain; observe without resending")]
    Transport,
    #[error("repository intake response is invalid or inconsistent")]
    Response,
}

#[cfg(test)]
mod tests;
