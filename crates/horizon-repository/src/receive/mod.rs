//! Explicit overlay receipt and read-only observation, not capture/export or setup authority.

mod request;

use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_overlay::bundle::store::{BundleStoreError, RepositoryBundleStore},
};
use request::Operation;
use serde::Serialize;
use std::{
    io::{Read, Write},
    process::ExitCode,
};

const VERSION: u32 = 1;
const RESPONSE_LIMIT: usize = 1024;

#[derive(Clone, Copy)]
pub(super) enum Command {
    Receive,
    Observe,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Acknowledged,
    Observed,
    Missing,
    Rejected,
    Error,
    WriteUnconfirmed,
}

#[derive(Serialize)]
struct Response {
    version: u32,
    status: Status,
    bundle_manifest: Option<ArtifactDigest>,
    reason: Option<&'static str>,
}

impl Response {
    fn new(status: Status, manifest: Option<ArtifactDigest>, reason: Option<&'static str>) -> Self {
        Self {
            version: VERSION,
            status,
            bundle_manifest: manifest,
            reason,
        }
    }

    fn exit_code(&self) -> u8 {
        match self.status {
            Status::Acknowledged | Status::Observed => 0,
            Status::Rejected => 2,
            Status::Missing => 4,
            Status::Error | Status::WriteUnconfirmed => 1,
        }
    }
}

pub(super) fn run(
    command: Command,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    run_with(command, input, output, diagnostics, execute)
}

fn run_with(
    command: Command,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
    execute: impl FnOnce(&Operation) -> Response,
) -> ExitCode {
    let response = match request::read(command, input) {
        Ok(operation) => execute(&operation),
        Err(()) => Response::new(Status::Rejected, None, Some("invalid or unsupported overlay request")),
    };
    super::write_response(&response, response.exit_code(), RESPONSE_LIMIT, output, diagnostics)
}

fn execute(operation: &Operation) -> Response {
    let request = operation.request();
    let Ok(store) = RepositoryBundleStore::open(&request.bundle_store) else {
        return Response::new(Status::Error, None, Some("overlay storage could not be safely opened"));
    };
    match operation {
        Operation::Receive { bundle, .. } => match store.put(bundle) {
            Ok(digest) => Response::new(Status::Acknowledged, Some(digest), None),
            Err(_) => Response::new(
                Status::WriteUnconfirmed,
                None,
                Some("overlay publication was not acknowledged; retain and observe existing data"),
            ),
        },
        Operation::Observe(_) => match store.get(&request.bundle_manifest) {
            Ok(bundle) => Response::new(Status::Observed, Some(bundle.manifest_sha256().clone()), None),
            Err(BundleStoreError::Missing) => Response::new(Status::Missing, None, None),
            Err(_) => Response::new(Status::Error, None, Some("overlay data could not be safely observed")),
        },
    }
}

#[cfg(test)]
mod tests;
