use horizon_core::repository_git::{self, GitPreparation, GitPreparationError, GitPreparationResponse, REQUEST_LIMIT};
use std::{
    io::{Read, Write},
    process::ExitCode,
};

pub(super) fn run(
    observe: bool,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    let mut bytes = Vec::new();
    let request = input
        .take(REQUEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GitPreparationError::Invalid)
        .and_then(|_| GitPreparation::decode(&bytes));
    let response = match request {
        Ok(request) if observe => repository_git::observe(&request),
        Ok(request) => repository_git::prepare(&request, || false),
        Err(reason) => GitPreparationResponse::failure(reason),
    };
    super::write_response(
        &response,
        response.exit_code(),
        repository_git::RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_input_is_redacted_and_output_loss_is_distinct() {
        for input in [
            b"private-invalid".to_vec(),
            vec![b' '; REQUEST_LIMIT + 1],
            b"{}{}".to_vec(),
        ] {
            for observe in [false, true] {
                let mut output = Vec::new();
                assert_eq!(
                    run(observe, &mut input.as_slice(), &mut output, &mut std::io::sink()),
                    ExitCode::from(2)
                );
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&output).unwrap()["reason"],
                    "invalid"
                );
                assert!(!String::from_utf8(output).unwrap().contains("private-invalid"));
            }
        }
        assert_eq!(
            run(
                false,
                &mut b"{}".as_slice(),
                &mut [0; 0].as_mut_slice(),
                &mut std::io::sink()
            ),
            ExitCode::from(3)
        );
    }
}
