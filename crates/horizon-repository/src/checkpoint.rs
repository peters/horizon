use horizon_core::repository_overlay::checkpoint::{
    CheckpointError, CheckpointFailure, CheckpointGeneration, CheckpointRequest, REQUEST_LIMIT, checkpoint_once,
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Serialize)]
struct Response {
    version: u8,
    status: &'static str,
    generation: Option<CheckpointGeneration>,
    retained: Option<PathBuf>,
    reason: Option<CheckpointError>,
}

pub(super) fn run(input: &mut impl Read, output: &mut impl Write, diagnostics: &mut impl Write) -> ExitCode {
    run_with(input, output, diagnostics, |request| checkpoint_once(request, || false))
}

fn run_with(
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
    execute: impl FnOnce(&CheckpointRequest) -> Result<CheckpointGeneration, CheckpointFailure>,
) -> ExitCode {
    let mut bytes = Vec::new();
    let request = input
        .take(REQUEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()
        .filter(|_| bytes.len() <= REQUEST_LIMIT)
        .and_then(|_| serde_json::from_slice::<CheckpointRequest>(&bytes).ok())
        .ok_or(CheckpointFailure {
            reason: CheckpointError::Invalid,
            retained: None,
        });
    let mut response = Response {
        version: 1,
        status: "verified",
        generation: None,
        retained: None,
        reason: None,
    };
    let code = match request.and_then(|request| execute(&request)) {
        Ok(generation) => {
            response.generation = Some(generation);
            0
        }
        Err(failure) => {
            let rejected = failure.retained.is_none()
                && matches!(failure.reason, CheckpointError::Invalid | CheckpointError::Unsupported);
            response.status = if rejected { "rejected" } else { "unconfirmed" };
            response.retained = failure.retained;
            response.reason = Some(failure.reason);
            if rejected { 2 } else { 1 }
        }
    };
    super::write_response(&response, code, super::protocol::RESPONSE_LIMIT, output, diagnostics)
}

#[cfg(test)]
mod tests;
