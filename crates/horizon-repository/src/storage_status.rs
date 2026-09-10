//! Bounded read-only qualification of the fixed worker root; no caller-selected paths.
use horizon_core::repository_overlay::intake::storage_status::{WorkerStorageStatus, inspect_worker_storage};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    process::ExitCode,
};

const REQUEST_LIMIT: usize = 1024;
const RESPONSE_LIMIT: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
}

#[derive(Serialize)]
struct Response {
    version: u8,
    status: &'static str,
}

pub(super) fn run(input: &mut impl Read, output: &mut impl Write, diagnostics: &mut impl Write) -> ExitCode {
    run_with(input, output, diagnostics, inspect_worker_storage)
}

fn run_with(
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
    inspect: impl FnOnce() -> WorkerStorageStatus,
) -> ExitCode {
    let mut bytes = Vec::new();
    let request = input
        .take((REQUEST_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()
        .filter(|_| bytes.len() <= REQUEST_LIMIT)
        .and_then(|_| serde_json::from_slice::<Request>(&bytes).ok())
        .filter(|request| request.version == 1);
    let (status, code) = match request {
        None => ("rejected", 2),
        Some(_) => match inspect() {
            WorkerStorageStatus::Qualified => ("qualified", 0),
            WorkerStorageStatus::Unsupported => ("unsupported", 1),
            WorkerStorageStatus::Unavailable => ("unavailable", 1),
        },
    };
    super::write_response(
        &Response { version: 1, status },
        code,
        RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

#[cfg(test)]
mod tests;
