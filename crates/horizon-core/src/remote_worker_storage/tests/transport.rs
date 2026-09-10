use super::*;
use std::{process::Command, time::Duration};

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[test]
fn real_child_receives_fixed_request_and_retains_valid_nonzero_responses() {
    for (status, code, expected) in [
        ("qualified", 0, WorkerStorageStatus::Qualified),
        ("unsupported", 1, WorkerStorageStatus::Unsupported),
        ("unavailable", 1, WorkerStorageStatus::Unavailable),
    ] {
        let script = format!(
            "read -r request; [ \"$request\" = '{{\"version\":1}}' ] || exit 9; \
             [ -z \"$(cat)\" ] || exit 8; printf '%s\\n' '{{\"version\":1,\"status\":\"{status}\"}}'; exit {code}"
        );
        assert_eq!(
            exchange(shell(&script), Duration::from_secs(2)).and_then(|response| protocol::response(&response)),
            Ok(expected)
        );
    }
}

#[test]
fn real_child_can_reply_after_the_fixed_request_without_waiting_for_eof() {
    for (status, code, expected) in [
        ("qualified", 0, WorkerStorageStatus::Qualified),
        ("unsupported", 1, WorkerStorageStatus::Unsupported),
        ("unavailable", 1, WorkerStorageStatus::Unavailable),
    ] {
        let script = format!(
            "read -r request; [ \"$request\" = '{{\"version\":1}}' ] || exit 9; \
             printf '%s\\n' '{{\"version\":1,\"status\":\"{status}\"}}'; exit {code}"
        );
        assert_eq!(
            exchange(shell(&script), Duration::from_secs(2)).and_then(|response| protocol::response(&response)),
            Ok(expected)
        );
    }
}

#[test]
fn real_child_failures_preserve_transport_error_categories() {
    assert!(matches!(
        exchange(
            Command::new("/nonexistent-storage-query-client"),
            Duration::from_secs(1)
        ),
        Err(RemoteStorageInspectionError::Admission(
            RemotePanelStatusError::ClientUnavailable
        ))
    ));
    for (script, deadline, expected) in [
        (
            "cat >/dev/null; exit 255",
            Duration::from_secs(2),
            RemoteStorageInspectionError::QueryFailed,
        ),
        (
            "cat >/dev/null; exec sleep 5",
            Duration::from_millis(30),
            RemoteStorageInspectionError::Deadline,
        ),
        (
            "cat >/dev/null; head -c 1025 /dev/zero",
            Duration::from_secs(2),
            RemoteStorageInspectionError::InvalidResponse,
        ),
    ] {
        assert_eq!(
            exchange(shell(script), deadline).and_then(|response| protocol::response(&response)),
            Err(expected)
        );
    }
}
