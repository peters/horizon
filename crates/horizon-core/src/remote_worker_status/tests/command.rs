use super::*;
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[test]
fn bounded_command_closes_stdin_and_never_exposes_subprocess_errors() {
    assert_eq!(
        command::run(shell("exec cat"), RUNNING, Duration::from_secs(2)),
        Ok(RUNNING.to_vec())
    );
    let error = command::run(
        shell("printf synthetic-private-response >&2; exit 7"),
        b"",
        Duration::from_secs(2),
    )
    .expect_err("failure");
    assert_eq!(error, RemotePanelStatusError::QueryFailed);
    assert!(!format!("{error:?} {error}").contains("synthetic-private-response"));
    assert_eq!(
        command::run(
            Command::new("/nonexistent-horizon-status-client"),
            b"",
            Duration::from_secs(2)
        ),
        Err(RemotePanelStatusError::ClientUnavailable)
    );
    assert_eq!(
        command::run(shell("printf '%06000d' 0"), b"", Duration::from_secs(2)),
        Err(RemotePanelStatusError::InvalidResponse)
    );
}

#[test]
fn stdin_backpressure_and_inherited_output_cannot_extend_the_deadline() {
    for (script, input) in [("exec sleep 5", vec![b'x'; 1024 * 1024]), ("sleep 1 & exit 0", vec![])] {
        let started = Instant::now();
        assert_eq!(
            command::run(shell(script), &input, Duration::from_millis(40)),
            Err(RemotePanelStatusError::Deadline)
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
        command::run(child, b"", Duration::from_millis(100)),
        Err(RemotePanelStatusError::Deadline)
    );
    let pid: u32 = std::fs::read_to_string(pid_file)
        .expect("owned PID")
        .parse()
        .expect("PID");
    assert!(!std::path::Path::new("/proc").join(pid.to_string()).exists());
}
