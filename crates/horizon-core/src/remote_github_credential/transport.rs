use super::{RemoteCredentialDeliveryError as Error, RemoteCredentialInstallation, RepositoryPat, validate_delivery};
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_worker_ssh::{known_hosts, prepared_github_install, query},
    remote_workspace_recovery::RecoveredRemoteWorkspace,
};
use std::{process::Command, time::Duration};

const RESPONSE_LIMIT: usize = 1024;

pub(super) fn install(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    token: &RepositoryPat<'_>,
) -> Result<RemoteCredentialInstallation, Error> {
    let endpoint = validate_delivery(store, recovered)?;
    let identity = recovered.identity();
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_github_install(identity.private_key_path(), trust.path(), endpoint)?;
    validate_delivery(store, recovered)?;
    exchange(command, token, Duration::from_secs(15))
}

pub(super) fn exchange(
    command: Command,
    token: &RepositoryPat<'_>,
    timeout: Duration,
) -> Result<RemoteCredentialInstallation, Error> {
    // run requires successful delivery of the entire known input and a zero exit.
    // stderr is discarded; neither raw response nor transport diagnostics escape.
    let bytes = query::run(command, token.0.as_bytes(), timeout, RESPONSE_LIMIT).map_err(|_| Error::DeliveryUnknown)?;
    response(&bytes)
}

fn response(bytes: &[u8]) -> Result<RemoteCredentialInstallation, Error> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        version: u8,
        status: RemoteCredentialInstallation,
    }
    if bytes.len() > RESPONSE_LIMIT {
        return Err(Error::DeliveryUnknown);
    }
    let reply: Response = serde_json::from_slice(bytes).map_err(|_| Error::DeliveryUnknown)?;
    if reply.version != 1 {
        return Err(Error::DeliveryUnknown);
    }
    Ok(reply.status)
}

#[cfg(test)]
mod tests;
