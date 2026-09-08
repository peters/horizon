#![forbid(unsafe_code)]

mod protocol;

use std::{
    io::{self, Read, Write},
    process::ExitCode,
};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).take(2).collect();
    if arguments.len() != 1 || arguments[0] != "materialize" {
        let _ = writeln!(
            io::stderr().lock(),
            "Usage: horizon-repository materialize < request.json"
        );
        return ExitCode::from(2);
    }
    run(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

fn run(input: &mut impl Read, output: &mut impl Write, diagnostics: &mut impl Write) -> ExitCode {
    let request = protocol::read_request(input);
    let result = request.as_ref().map(protocol::execute);
    let response = match &result {
        Ok(result) => protocol::Response::from_result(result),
        Err(()) => protocol::Response::rejected(),
    };
    let bytes = serde_json::to_vec(&response)
        .ok()
        .filter(|bytes| bytes.len() < protocol::RESPONSE_LIMIT);
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
    ExitCode::from(response.exit_code())
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
