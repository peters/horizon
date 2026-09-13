#![forbid(unsafe_code)]

mod capture;
mod checkpoint;
mod git;
mod intake;
mod pack;
mod protocol;
mod receive;
mod setup;
mod setup_checkout;
mod storage_status;

use std::{
    io::{self, Read, Write},
    process::ExitCode,
};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).take(2).collect();
    let command = arguments.first().and_then(|argument| argument.to_str());
    if arguments.len() != 1
        || !matches!(
            command,
            Some(
                "materialize"
                    | "capture-binding"
                    | "capture-plan"
                    | "capture-once"
                    | "checkpoint-once"
                    | "setup"
                    | "setup-binding"
                    | "setup-checkout"
                    | "setup-status"
                    | "receive-overlay"
                    | "overlay-status"
                    | "receive-pack"
                    | "pack-status"
                    | "intake"
                    | "intake-status"
                    | "storage-status"
                    | "git-prepare"
                    | "git-status"
                    | "git-binding"
                    | "git-checkout"
            )
        )
    {
        let _ = writeln!(
            io::stderr().lock(),
            "Usage: horizon-repository materialize|capture-binding|capture-plan|capture-once|checkpoint-once|setup|setup-status|setup-binding|setup-checkout|receive-overlay|overlay-status|receive-pack|pack-status|intake|intake-status|storage-status|git-prepare|git-status|git-binding|git-checkout < request"
        );
        return ExitCode::from(2);
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut diagnostics = io::stderr().lock();
    match command {
        Some("checkpoint-once") => checkpoint::run(&mut input, &mut output, &mut diagnostics),
        Some("capture-binding") => capture::run(capture::Operation::Binding, &mut input, &mut output, &mut diagnostics),
        Some("capture-plan") => capture::run(capture::Operation::Plan, &mut input, &mut output, &mut diagnostics),
        Some("capture-once") => capture::run(capture::Operation::Once, &mut input, &mut output, &mut diagnostics),
        Some("git-binding") => git::inspect(false, &mut input, &mut output, &mut diagnostics),
        Some("git-checkout") => git::inspect(true, &mut input, &mut output, &mut diagnostics),
        Some("git-prepare") => git::run(false, &mut input, &mut output, &mut diagnostics),
        Some("git-status") => git::run(true, &mut input, &mut output, &mut diagnostics),
        Some("setup") => setup::run(setup::Command::Execute, &mut input, &mut output, &mut diagnostics),
        Some("setup-status") => setup::run(setup::Command::Observe, &mut input, &mut output, &mut diagnostics),
        Some("setup-binding") => setup_checkout::run(
            setup_checkout::Command::Binding,
            &mut input,
            &mut output,
            &mut diagnostics,
        ),
        Some("setup-checkout") => setup_checkout::run(
            setup_checkout::Command::Inspect,
            &mut input,
            &mut output,
            &mut diagnostics,
        ),
        Some("receive-overlay") => receive::run(receive::Command::Receive, &mut input, &mut output, &mut diagnostics),
        Some("overlay-status") => receive::run(receive::Command::Observe, &mut input, &mut output, &mut diagnostics),
        Some("receive-pack") => pack::run(pack::Command::Receive, &mut input, &mut output, &mut diagnostics),
        Some("pack-status") => pack::run(pack::Command::Observe, &mut input, &mut output, &mut diagnostics),
        Some("intake") => intake::run(false, &mut input, &mut output, &mut diagnostics),
        Some("intake-status") => intake::run(true, &mut input, &mut output, &mut diagnostics),
        Some("storage-status") => storage_status::run(&mut input, &mut output, &mut diagnostics),
        _ => run(&mut input, &mut output, &mut diagnostics),
    }
}

fn run(input: &mut impl Read, output: &mut impl Write, diagnostics: &mut impl Write) -> ExitCode {
    let request = protocol::read_request(input);
    let result = request.as_ref().map(protocol::execute);
    let response = match &result {
        Ok(result) => protocol::Response::from_result(result),
        Err(()) => protocol::Response::rejected(),
    };
    write_response(
        &response,
        response.exit_code(),
        protocol::RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

fn write_response(
    response: &impl serde::Serialize,
    exit_code: u8,
    limit: usize,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    let bytes = serde_json::to_vec(response).ok().filter(|bytes| bytes.len() < limit);
    let written = bytes.is_some_and(|bytes| {
        output
            .write_all(&bytes)
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush())
            .is_ok()
    });
    if !written {
        let _ = writeln!(
            diagnostics,
            "Could not write a complete response; retain data and inspect before any retry."
        );
        return ExitCode::from(3);
    }
    ExitCode::from(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_is_complete_json_and_failed_output_is_distinct() {
        let mut bytes = Vec::new();
        assert_eq!(
            run(&mut b"private-invalid-input".as_slice(), &mut bytes, &mut io::sink()),
            ExitCode::from(2)
        );
        assert!(bytes.ends_with(b"\n"));
        let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["version"], 1);
        assert_eq!(response["status"], "rejected");
        assert!(response["source_metadata"].is_null() && response["checkout"].is_null());
        assert!(!String::from_utf8(bytes).unwrap().contains("private-invalid-input"));
        assert_eq!(
            run(
                &mut b"{}".as_slice(),
                &mut [0_u8; 0].as_mut_slice(),
                &mut [0_u8; 0].as_mut_slice()
            ),
            ExitCode::from(3)
        );
    }
}
