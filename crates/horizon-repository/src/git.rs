use horizon_core::repository_git::{self, GitPreparation, GitPreparationError, GitPreparationResponse, REQUEST_LIMIT};
use std::{
    io::{Read, Write},
    process::ExitCode,
};

pub(super) fn inspect(
    checkout: bool,
    input: &mut impl Read,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> ExitCode {
    #[derive(serde::Serialize)]
    struct Response {
        version: u8,
        binding_sha256: Option<horizon_core::cloud_run::ArtifactDigest>,
        runtime: Option<String>,
        root: Option<repository_git::GitCheckoutLocation>,
        reason: Option<GitPreparationError>,
    }
    let mut response = Response {
        version: 1,
        binding_sha256: None,
        runtime: None,
        root: None,
        reason: None,
    };
    response.reason = (|| {
        let mut bytes = Vec::new();
        input
            .take(REQUEST_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| GitPreparationError::Invalid)?;
        let request = GitPreparation::decode(&bytes)?;
        response.binding_sha256 = Some(request.binding()?);
        response.runtime = Some(request.runtime_id.to_string());
        if checkout {
            response.root = Some(request.inspect_checkout()?);
        }
        Ok(())
    })()
    .err();
    let code = match response.reason {
        None => 0,
        Some(GitPreparationError::Invalid | GitPreparationError::Unsupported) => 2,
        Some(_) => 1,
    };
    super::write_response(&response, code, repository_git::RESPONSE_LIMIT, output, diagnostics)
}

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
    fn task_binding_is_framed_and_read_only_before_checkout_inspection() {
        let request = serde_json::json!({"version":1,"workspace_local_id":"fixture",
            "runtime_id":"00000000-0000-4000-8000-000000000001",
            "source":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":"work/one"},
            "work_branch":"work/one"});
        let bytes = serde_json::to_vec(&request).unwrap();
        let mut output = vec![];
        assert_eq!(
            inspect(false, &mut bytes.as_slice(), &mut output, &mut std::io::sink()),
            ExitCode::SUCCESS
        );
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["runtime"], request["runtime_id"]);
        assert!(response["root"].is_null() && response["reason"].is_null());
        assert_eq!(response["binding_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(
            inspect(
                false,
                &mut bytes.as_slice(),
                &mut [0; 0].as_mut_slice(),
                &mut std::io::sink()
            ),
            ExitCode::from(3)
        );
        for checkout in [false, true] {
            for invalid in [
                b"private-invalid".to_vec(),
                vec![b' '; REQUEST_LIMIT + 1],
                [bytes.clone(), b"{}".to_vec()].concat(),
            ] {
                output.clear();
                assert_eq!(
                    inspect(checkout, &mut invalid.as_slice(), &mut output, &mut std::io::sink()),
                    ExitCode::from(2)
                );
                let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(response["reason"], "invalid");
                assert!(
                    response["root"].is_null() && response["runtime"].is_null() && response["binding_sha256"].is_null()
                );
            }
        }
    }
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
