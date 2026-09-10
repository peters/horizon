//! Combined worker-only intake framing; neither parsing nor observation grants setup.
use horizon_core::repository_overlay::intake::{self, IntakeError, IntakeRequest, IntakeResponse, REQUEST_LIMIT};
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
    let response = match read_request(observe, input) {
        Ok(request) if observe => intake::observe(&request, || false),
        Ok(request) => intake::receive(&request, input, || false),
        Err(()) => IntakeResponse::failure(IntakeError::Invalid),
    };
    super::write_response(
        &response,
        response.exit_code(),
        intake::RESPONSE_LIMIT,
        output,
        diagnostics,
    )
}

fn read_request(observe: bool, input: &mut impl Read) -> Result<IntakeRequest, ()> {
    let length = if observe {
        REQUEST_LIMIT + 1
    } else {
        let mut prefix = [0; 4];
        input.read_exact(&mut prefix).map_err(|_| ())?;
        let length = usize::try_from(u32::from_le_bytes(prefix)).map_err(|_| ())?;
        if length == 0 || length > REQUEST_LIMIT {
            return Err(());
        }
        length
    };
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| ())?;
    input.take(length as u64).read_to_end(&mut bytes).map_err(|_| ())?;
    if !observe && bytes.len() != length {
        return Err(());
    }
    IntakeRequest::decode(&bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn bad_headers_are_bounded_redacted_and_output_loss_is_distinct() {
        for header in [
            b"{}".to_vec(),
            b"private-invalid-input".to_vec(),
            vec![b' '; REQUEST_LIMIT + 1],
        ] {
            for observe in [false, true] {
                let mut bytes = if observe {
                    vec![]
                } else {
                    u32::try_from(header.len()).unwrap().to_le_bytes().to_vec()
                };
                bytes.extend(&header);
                let mut output = vec![];
                assert_eq!(
                    run(observe, &mut bytes.as_slice(), &mut output, &mut io::sink()),
                    ExitCode::from(2)
                );
                let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert!(
                    response["roots"].is_null() && response["pack"].is_null() && response["intent_sha256"].is_null()
                );
                assert!(!String::from_utf8(output).unwrap().contains("private-invalid-input"));
            }
        }
        let mut bytes = (u32::try_from(REQUEST_LIMIT).unwrap() + 1).to_le_bytes().to_vec();
        bytes.extend(b"unread payload");
        let mut input = bytes.as_slice();
        assert!(read_request(false, &mut input).is_err());
        assert_eq!(input, b"unread payload");
        assert_eq!(
            run(
                true,
                &mut b"{}".as_slice(),
                &mut [0; 0].as_mut_slice(),
                &mut [0; 0].as_mut_slice()
            ),
            ExitCode::from(3)
        );
    }

    #[test]
    fn valid_short_reads_stop_at_header_and_status_requires_eof() {
        struct Short<'a>(&'a [u8]);
        impl Read for Short<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let count = buffer.len().min(1);
                self.0.read(&mut buffer[..count])
            }
        }
        let header = serde_json::to_vec(&serde_json::json!({
            "version":1,"workspace_local_id":"fixture-workspace",
            "workflow_id":"11111111-1111-4111-8111-111111111111",
            "job_id":"22222222-2222-4222-8222-222222222222",
            "runtime_generation":1,"worker_resource_id":"fixture-worker",
            "client_key_sha256":"a".repeat(64),
            "source":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":null},
            "pack":{"sha256":"b".repeat(64),"encoded_bytes":32},
            "overlay":{"sha256":"c".repeat(64),"encoded_bytes":1}
        }))
        .unwrap();
        let mut bytes = u32::try_from(header.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend(&header);
        bytes.extend(b"pack-and-overlay");
        let mut short = Short(&bytes);
        let received = read_request(false, &mut short).unwrap();
        assert_eq!(short.0, b"pack-and-overlay");
        assert_eq!(read_request(true, &mut Short(&header)).unwrap(), received);
        let mut trailing = header.clone();
        trailing.extend(b"payload");
        assert!(read_request(true, &mut Short(&trailing)).is_err());
        let truncated = &bytes[..4 + header.len() - 1];
        assert!(read_request(false, &mut Short(truncated)).is_err());
        for prefix in [&[0, 0, 0, 0][..], &[1, 0, 0][..]] {
            assert!(read_request(false, &mut Short(prefix)).is_err());
        }
    }
}
