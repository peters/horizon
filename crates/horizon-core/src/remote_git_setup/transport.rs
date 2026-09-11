use super::{
    RemoteGitSetupError as Error, RemoteGitSubmission as Submission, lease_deadline, protocol, validate_current,
};
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_worker_ssh::{known_hosts, prepared_git_setup, query},
    remote_workspace_recovery::RecoveredRemoteWorkspace,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};

pub(super) fn execute(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    input: &[u8],
    observe: bool,
) -> Result<Submission, Error> {
    let endpoint = validate_current(store, recovered)?;
    let identity = recovered.identity();
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_git_setup(identity.private_key_path(), trust.path(), endpoint, observe)?;
    validate_current(store, recovered)?;
    exchange_before(
        command,
        input,
        observe,
        lease_deadline(recovered)?,
        time::OffsetDateTime::now_utc,
    )
}

pub(super) fn exchange_before(
    command: Command,
    mut input: &[u8],
    observe: bool,
    deadline: Option<time::OffsetDateTime>,
    utc_now: impl Fn() -> time::OffsetDateTime,
) -> Result<Submission, Error> {
    let started = Instant::now();
    let timeout = lease_timeout(observe, deadline, utc_now())?;
    let expired = || started.elapsed() >= timeout || deadline.is_some_and(|end| utc_now() >= end);
    let limit = if observe {
        protocol::STATUS_LIMIT
    } else {
        protocol::LAUNCH_LIMIT
    };
    let expected = input.len() as u64;
    // Keep nonzero framed responses, but independently require every immutable
    // request byte written. The callback includes spawn time and guards each write.
    let result =
        query::exchange(command, &mut input, timeout, limit, expired, None).map_err(|_| Error::OutcomeUnknown)?;
    known_response(&result, expected, observe)
}

pub(super) fn known_response(result: &query::Exchange, expected: u64, observe: bool) -> Result<Submission, Error> {
    let (query::InputProgress::Complete(written) | query::InputProgress::Incomplete(written)) = result.input;
    if written != expected {
        return Err(Error::OutcomeUnknown);
    }
    protocol::response(&result.output, result.status.code(), observe)
}

pub(super) fn lease_timeout(
    observe: bool,
    deadline: Option<time::OffsetDateTime>,
    now: time::OffsetDateTime,
) -> Result<Duration, Error> {
    let timeout = Duration::from_secs(if observe { 40 } else { 60 });
    match deadline {
        Some(end) if end <= now => Err(Error::ExpiredWorker),
        Some(end) => Duration::try_from(end - now)
            .map(|remaining| remaining.min(timeout))
            .map_err(|_| Error::ExpiredWorker),
        None => Ok(timeout),
    }
}
