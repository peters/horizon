mod streaming;

use super::*;
use std::{
    process::Command,
    time::{Duration, Instant},
};

const INPUT: &[u8] = b"synthetic query";
const TEST_LIMIT: usize = 4096;

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[test]
fn bounded_command_closes_stdin_and_never_exposes_subprocess_errors() {
    assert_eq!(
        run(shell("exec cat"), INPUT, Duration::from_secs(2), TEST_LIMIT),
        Ok(INPUT.to_vec())
    );
    let error = run(
        shell("printf synthetic-private-response >&2; exit 7"),
        b"",
        Duration::from_secs(2),
        TEST_LIMIT,
    )
    .expect_err("failure");
    assert_eq!(error, Error::QueryFailed);
    assert!(!format!("{error:?} {error}").contains("synthetic-private-response"));
    assert_eq!(
        run(
            Command::new("/nonexistent-horizon-status-client"),
            b"",
            Duration::from_secs(2),
            TEST_LIMIT
        ),
        Err(Error::ClientUnavailable)
    );
    assert_eq!(
        run(shell("printf '%06000d' 0"), b"", Duration::from_secs(2), TEST_LIMIT),
        Err(Error::OutputLimit)
    );
}

#[test]
fn stdin_backpressure_and_inherited_output_cannot_extend_the_deadline() {
    for (script, input) in [("exec sleep 5", vec![b'x'; 1024 * 1024]), ("sleep 1 & exit 0", vec![])] {
        let started = Instant::now();
        assert_eq!(
            run(shell(script), &input, Duration::from_millis(40), TEST_LIMIT),
            Err(Error::Deadline)
        );
        assert!(started.elapsed() < Duration::from_millis(800));
    }
}

#[test]
fn timed_out_client_is_reaped_without_signalling_any_other_process() {
    let fixture = tempfile::tempdir().expect("fixture");
    let pid_file = fixture.path().join("owned-child");
    let mut child = shell("printf '%s' \"$$\" > \"$1\"; exec sleep 5");
    child.arg("fixture").arg(&pid_file);
    assert_eq!(
        run(child, b"", Duration::from_millis(100), TEST_LIMIT),
        Err(Error::Deadline)
    );
    let pid: u32 = std::fs::read_to_string(pid_file)
        .expect("owned PID")
        .parse()
        .expect("PID");
    assert!(!std::path::Path::new("/proc").join(pid.to_string()).exists());
}

#[test]
fn each_caller_selects_its_exact_output_ceiling() {
    for size in [0, 1, 4096, 8192] {
        let input = vec![b'x'; size];
        assert_eq!(run(shell("exec cat"), &input, Duration::from_secs(2), size), Ok(input));
        assert_eq!(
            run(shell("exec cat"), &vec![b'x'; size + 1], Duration::from_secs(2), size),
            Err(Error::OutputLimit)
        );
    }
}
