//! Credential-only recovery never submits repository preparation or task work.

use super::{
    CloudWorkflowStore, ConfiguredRemoteGitSetupError as Error, ConfiguredRemoteGitSetupRequest,
    PreparedRemoteGitSetup, RemoteCredentialInstallation, RemoteGitCredentialMode, RemoteProviderConfig,
    RemoteSshIdentityStore, RepositoryPat,
};

/// Prepare credential-only disclosure for a ready or reconciling saved worker.
/// Reads the existing binding only; no provider call, token read or state mutation.
/// # Errors
/// Rejects stale/foreign bindings and saved phases that cannot receive credentials.
pub fn prepare_configured_remote_git_credential(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
) -> Result<PreparedRemoteGitSetup, Error> {
    let prepared =
        super::prepare_configured_remote_git_setup(store, config, request, RemoteGitCredentialMode::InstallFirst)?;
    require_ready(&prepared)?;
    Ok(prepared)
}

fn require_ready(prepared: &PreparedRemoteGitSetup) -> Result<(), Error> {
    use crate::remote_workspace::RemoteRuntimePhase;
    if !prepared
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .is_some_and(|runtime| {
            matches!(
                runtime.phase,
                RemoteRuntimePhase::Ready | RemoteRuntimePhase::Reconciling
            )
        })
    {
        return Err(Error::CredentialNotReady);
    }
    Ok(())
}

/// Install a missing runtime credential on the explicitly confirmed original worker.
/// The caller owns disclosure consent and transient PAT storage. Existing credentials
/// are never replaced; Installed/Present does not prove GitHub permissions or expiry.
/// This does not inspect or replay Git preparation, create workers or start tasks.
/// Run off-thread. The existing pinned installer bounds stdin delivery to 15 seconds
/// or the remaining worker lease. An uncertain reply must not trigger automatic retry.
/// # Errors
/// Refuses mismatched consent, stale selection/configuration, missing retained trust,
/// pending management and unconfirmed delivery. Drift after dispatch is unknown.
pub fn install_configured_remote_git_credential(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    prepared: &PreparedRemoteGitSetup,
    token: &RepositoryPat<'_>,
) -> Result<RemoteCredentialInstallation, Error> {
    if prepared.credentials != RemoteGitCredentialMode::InstallFirst {
        return Err(Error::CredentialConsentMismatch);
    }
    require_ready(prepared)?;
    #[cfg(target_os = "linux")]
    {
        match super::dispatch_operation(
            store,
            identities,
            config,
            request,
            prepared,
            Operation::Credential(token),
        )? {
            Outcome::Credential(result) => Ok(result),
            Outcome::Git(_) => Err(Error::OutcomeUnknown),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request, prepared, token);
        Err(super::RemoteGitSetupError::UnsupportedPlatform.into())
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub(super) enum Operation<'a> {
    Git {
        token: Option<&'a RepositoryPat<'a>>,
        inspect: bool,
    },
    Credential(&'a RepositoryPat<'a>),
}

#[cfg(target_os = "linux")]
pub(super) enum Outcome {
    Git(super::ConfiguredRemoteGitSubmission),
    Credential(RemoteCredentialInstallation),
}

#[cfg(target_os = "linux")]
pub(super) fn perform<P: crate::cloud_run::interactive_worker::InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    check: impl Fn() -> Result<(), Error>,
    prepared: &PreparedRemoteGitSetup,
    operation: Operation<'_>,
) -> Result<Outcome, Error> {
    match operation {
        Operation::Git { token, inspect } => {
            super::perform_git(store, identities, provider, check, prepared, token, inspect).map(Outcome::Git)
        }
        Operation::Credential(token) => install_with(&check, || {
            crate::remote_github_credential::install_with_admission(
                store,
                identities,
                provider,
                &prepared.allocation,
                token,
                || {
                    check().map_err(|_| {
                        crate::remote_github_credential::RemoteCredentialDeliveryError::Recovery(
                            crate::remote_workspace_recovery::RemoteWorkspaceRecoveryError::StateChanged,
                        )
                    })
                },
            )
            .map_err(Into::into)
        })
        .map(Outcome::Credential),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn install_with(
    check: impl Fn() -> Result<(), Error>,
    install: impl FnOnce() -> Result<RemoteCredentialInstallation, Error>,
) -> Result<RemoteCredentialInstallation, Error> {
    check()?;
    let result = install();
    check().map_err(|_| Error::OutcomeUnknown)?;
    result
}
