use horizon_core::{
    cloud_run::{ArtifactDigest, CloudJobId},
    repository_overlay::intake::setup::{
        SETUP_SELECTION_LIMIT, SetupCheckoutError, SetupCheckoutLocation, SetupCheckoutSelection,
    },
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    process::ExitCode,
};

#[derive(Clone, Copy)]
pub(super) enum Command {
    Binding,
    Inspect,
}

#[derive(Serialize)]
struct Response {
    version: u8,
    binding_sha256: Option<ArtifactDigest>,
    runtime: Option<CloudJobId>,
    root: Option<SetupCheckoutLocation>,
    reason: Option<SetupCheckoutError>,
}

pub(super) fn run(
    command: Command,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    let mut response = Response {
        version: 1,
        binding_sha256: None,
        runtime: None,
        root: None,
        reason: None,
    };
    let result = (|| {
        let mut bytes = Vec::new();
        input
            .take(SETUP_SELECTION_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SetupCheckoutError::Invalid)?;
        let selection = SetupCheckoutSelection::decode(&bytes)?;
        response.binding_sha256 = Some(selection.binding()?);
        response.runtime = Some(selection.runtime());
        if matches!(command, Command::Inspect) {
            response.root = Some(selection.inspect(|| false)?);
        }
        Ok(())
    })();
    response.reason = result.err();
    let code = match response.reason {
        None => 0,
        Some(SetupCheckoutError::Invalid) => 2,
        Some(_) => 1,
    };
    super::write_response(&response, code, 16 * 1024, output, diagnostics)
}

#[cfg(test)]
mod tests;
