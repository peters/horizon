use super::{RemoteStorageInspectionError as Error, WorkerStorageStatus};
use crate::remote_worker_ssh::query::{Exchange, InputProgress};
use serde::Deserialize;

pub(super) const REQUEST: &[u8] = b"{\"version\":1}\n";
pub(super) const RESPONSE_LIMIT: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u8,
    status: Status,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Qualified,
    Unsupported,
    Unavailable,
    Rejected,
}

pub(super) fn response(exchange: &Exchange) -> Result<WorkerStorageStatus, Error> {
    // This immutable request has a known exact length. A child may finish after
    // those bytes are written but before the exchange loop observes source EOF.
    if !matches!(exchange.input, InputProgress::Complete(n) | InputProgress::Incomplete(n)
        if n == REQUEST.len() as u64)
    {
        return Err(Error::QueryFailed);
    }
    let code = exchange.status.code();
    if !matches!(code, Some(0..=2)) {
        return Err(Error::QueryFailed);
    }
    if exchange.output.len() > RESPONSE_LIMIT {
        return Err(Error::InvalidResponse);
    }
    let response: Response = serde_json::from_slice(&exchange.output).map_err(|_| Error::InvalidResponse)?;
    if response.version != 1 {
        return Err(Error::InvalidResponse);
    }
    match (response.status, code) {
        (Status::Qualified, Some(0)) => Ok(WorkerStorageStatus::Qualified),
        (Status::Unsupported, Some(1)) => Ok(WorkerStorageStatus::Unsupported),
        (Status::Unavailable, Some(1)) => Ok(WorkerStorageStatus::Unavailable),
        _ => Err(Error::InvalidResponse),
    }
}
