//! Explicit streaming pack receipt and current observation, never setup authority.

#[cfg(target_os = "linux")]
mod linux;
mod request;
mod response;

use request::Request;
use response::{Response, Status};
use std::{
    io::{Read, Write},
    process::ExitCode,
};

const VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub(super) enum Command {
    Receive,
    Observe,
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
    execute: impl FnOnce(&Request, &mut dyn Read) -> Response,
) -> ExitCode {
    let response = match request::read(command, input) {
        Ok(request) => execute(&request, input),
        Err(()) => Response::new(Status::Rejected, "invalid or unsupported pack request"),
    };
    super::write_response(
        &response,
        response.exit_code(),
        super::protocol::RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

fn execute(request: &Request, input: &mut dyn Read) -> Response {
    #[cfg(target_os = "linux")]
    {
        linux::execute(request, input)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (request, input);
        Response::new(Status::Unsupported, "pack commands require Linux")
    }
}

#[cfg(test)]
mod tests;
