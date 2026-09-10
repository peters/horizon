use super::*;
use std::{cell::Cell, io};

#[test]
fn exact_status_and_exit_codes_have_no_private_values() {
    for (status, name, code) in [
        (WorkerStorageStatus::Qualified, "qualified", 0),
        (WorkerStorageStatus::Unsupported, "unsupported", 1),
        (WorkerStorageStatus::Unavailable, "unavailable", 1),
    ] {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        assert_eq!(
            run_with(
                &mut br#"{"version":1}"#.as_slice(),
                &mut output,
                &mut diagnostics,
                || status
            ),
            ExitCode::from(code)
        );
        assert_eq!(output, format!("{{\"version\":1,\"status\":\"{name}\"}}\n").as_bytes());
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn malformed_oversized_or_failed_input_never_inspects() {
    struct Failed;
    impl Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("private read failure"))
        }
    }
    for bytes in [
        b"".to_vec(),
        b"{}".to_vec(),
        br#"{"version":2}"#.to_vec(),
        br#"{"version":1,"version":1}"#.to_vec(),
        br#"{"version":1,"path":"private"}"#.to_vec(),
        br#"{"version":1}{}"#.to_vec(),
        br#"{"version":1.0}"#.to_vec(),
        vec![b' '; REQUEST_LIMIT + 1],
    ] {
        let mut output = Vec::new();
        assert_eq!(
            run_with(&mut bytes.as_slice(), &mut output, &mut io::sink(), || panic!(
                "rejected input inspected"
            )),
            ExitCode::from(2)
        );
        assert_eq!(output, b"{\"version\":1,\"status\":\"rejected\"}\n");
    }
    assert_eq!(
        run_with(&mut Failed, &mut io::sink(), &mut io::sink(), || panic!(
            "failed input inspected"
        )),
        ExitCode::from(2)
    );
}

#[test]
fn output_failure_does_not_repeat_inspection() {
    let calls = Cell::new(0);
    let mut diagnostics = Vec::new();
    assert_eq!(
        run_with(
            &mut br#"{"version":1}"#.as_slice(),
            &mut [].as_mut_slice(),
            &mut diagnostics,
            || {
                calls.set(calls.get() + 1);
                WorkerStorageStatus::Qualified
            }
        ),
        ExitCode::from(3)
    );
    assert_eq!(calls.get(), 1);
    assert!(!diagnostics.is_empty());
}

#[test]
fn short_reads_accept_eof_and_oversized_reads_are_bounded() {
    struct Short<'a>(&'a [u8]);
    impl Read for Short<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = bytes.len().min(1);
            self.0.read(&mut bytes[..count])
        }
    }
    assert_eq!(
        run_with(
            &mut Short(b" {\"version\":1} \n"),
            &mut io::sink(),
            &mut io::sink(),
            || WorkerStorageStatus::Qualified
        ),
        ExitCode::from(0)
    );
    let bytes = vec![b' '; REQUEST_LIMIT + 100];
    let mut input = bytes.as_slice();
    assert_eq!(
        run_with(&mut input, &mut io::sink(), &mut io::sink(), || panic!(
            "oversized input inspected"
        )),
        ExitCode::from(2)
    );
    assert_eq!(input.len(), 99);
}
