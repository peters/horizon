use super::{
    RemoteCredentialDeliveryError as Error, RemoteCredentialInstallation, RepositoryPat, lease_deadline,
    validate_delivery,
};
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_worker_ssh::{known_hosts, prepared_github_install, query},
    remote_workspace_recovery::RecoveredRemoteWorkspace,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};

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
    exchange_before(
        command,
        token,
        Duration::from_secs(15),
        lease_deadline(recovered)?,
        time::OffsetDateTime::now_utc,
    )
}

#[cfg(test)]
pub(super) fn exchange(
    command: Command,
    token: &RepositoryPat<'_>,
    timeout: Duration,
) -> Result<RemoteCredentialInstallation, Error> {
    exchange_before(command, token, timeout, None, time::OffsetDateTime::now_utc)
}

fn exchange_before(
    command: Command,
    token: &RepositoryPat<'_>,
    timeout: Duration,
    deadline: Option<time::OffsetDateTime>,
    utc_now: impl Fn() -> time::OffsetDateTime,
) -> Result<RemoteCredentialInstallation, Error> {
    let started = Instant::now();
    let timeout = lease_timeout(timeout, deadline, utc_now())?;
    // Absolute admission includes spawn overhead and is checked by exchange
    // before spawn, after spawn, after input reads and immediately before EACH
    // write. A forward wall-clock jump also revokes further token release.
    let expired = || started.elapsed() >= timeout || deadline.is_some_and(|end| utc_now() >= end);
    // Known input requires successful delivery of every byte and a zero exit.
    // stderr is discarded; neither raw response nor transport diagnostics escape.
    let mut input = token.0.as_bytes();
    let expected = input.len() as u64;
    let result = query::exchange(command, &mut input, timeout, RESPONSE_LIMIT, expired, Some(expected))
        .map_err(|_| Error::DeliveryUnknown)?;
    known_response(&result, expected)
}

fn known_response(result: &query::Exchange, expected: u64) -> Result<RemoteCredentialInstallation, Error> {
    let (query::InputProgress::Complete(written) | query::InputProgress::Incomplete(written)) = result.input;
    if !result.status.success() || written != expected {
        return Err(Error::DeliveryUnknown);
    }
    response(&result.output)
}

fn lease_timeout(
    timeout: Duration,
    deadline: Option<time::OffsetDateTime>,
    now: time::OffsetDateTime,
) -> Result<Duration, Error> {
    match deadline {
        Some(end) if end <= now => Err(Error::ExpiredWorker),
        Some(end) => Duration::try_from(end - now)
            .map(|remaining| remaining.min(timeout))
            .map_err(|_| Error::ExpiredWorker),
        None => Ok(timeout),
    }
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
