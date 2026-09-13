use horizon_core::repository_overlay::checkpoint::{
    CheckpointError, CheckpointGeneration, CheckpointRequest, REQUEST_LIMIT, checkpoint_once,
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
    let mut bytes = Vec::new();
    let request = input
        .take(REQUEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()
        .filter(|_| bytes.len() <= REQUEST_LIMIT)
        .and_then(|_| serde_json::from_slice::<CheckpointRequest>(&bytes).ok());
    let (response, code) = match request {
        Some(request) => match checkpoint_once(&request, || false) {
            Ok(generation) => (
                Response {
                    version: 1,
                    status: "verified",
                    generation: Some(generation),
                    retained: None,
                    reason: None,
                },
                0,
            ),
            Err(failure) => {
                let rejected = failure.retained.is_none()
                    && matches!(failure.reason, CheckpointError::Invalid | CheckpointError::Unsupported);
                (
                    Response {
                        version: 1,
                        status: if rejected { "rejected" } else { "unconfirmed" },
                        generation: None,
                        retained: failure.retained,
                        reason: Some(failure.reason),
                    },
                    if rejected { 2 } else { 1 },
                )
            }
        },
        None => (
            Response {
                version: 1,
                status: "rejected",
                generation: None,
                retained: None,
                reason: Some(CheckpointError::Invalid),
            },
            2,
        ),
    };
    super::write_response(&response, code, super::protocol::RESPONSE_LIMIT, output, diagnostics)
}
