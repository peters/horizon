use super::*;
use crate::{
    cloud_run::interactive_worker::InteractiveWorkerLifecycle,
    remote_repository_pack::tests::Fixture,
    remote_worker_ssh::query::{Exchange, InputProgress},
};
use std::os::unix::process::ExitStatusExt;

mod transport;

fn wire(bytes: &[u8], code: i32) -> Exchange {
    Exchange {
        status: std::process::ExitStatus::from_raw(code << 8),
        input: InputProgress::Complete(protocol::REQUEST.len() as u64),
        output: bytes.to_vec(),
    }
}

fn reply(status: &str, code: i32) -> Exchange {
    wire(format!("{{\"version\":1,\"status\":\"{status}\"}}\n").as_bytes(), code)
}

#[test]
fn zero_panel_storage_observations_preserve_allocation_and_client_identity() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let before = fixture.current();
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
    assert!(before.workspace().state().spec.panels.is_empty());
    for (name, code, expected) in [
        ("qualified", 0, WorkerStorageStatus::Qualified),
        ("unsupported", 1, WorkerStorageStatus::Unsupported),
        ("unavailable", 1, WorkerStorageStatus::Unavailable),
    ] {
        let observed = inspect_with(&fixture.store, &fixture.recovered, |identity, endpoint| {
            assert_eq!(identity.public_key(), fixture.recovered.identity().public_key());
            assert_eq!(
                Some(endpoint),
                fixture.recovered.observation().and_then(|value| value.ssh.as_ref())
            );
            Ok(reply(name, code))
        });
        assert_eq!(observed, Ok(expected));
        assert_eq!(fixture.current(), before);
        assert_eq!(
            std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
            key
        );
    }
}

#[test]
fn missing_nonready_or_foreign_worker_refuses_before_transport() {
    for lifecycle in [
        None,
        Some(InteractiveWorkerLifecycle::Stopped),
        Some(InteractiveWorkerLifecycle::Unknown),
    ] {
        let fixture = Fixture::new(lifecycle);
        assert_eq!(
            inspect_with(&fixture.store, &fixture.recovered, |_, _| panic!("no SSH")),
            Err(RemoteStorageInspectionError::Admission(
                RemotePanelStatusError::WorkerUnavailable
            )),
        );
    }
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let foreign = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    assert_eq!(
        inspect_with(&foreign.store, &fixture.recovered, |_, _| panic!("no foreign query")),
        Err(RemoteStorageInspectionError::Admission(
            RemotePanelStatusError::StateChanged
        )),
    );
}

#[test]
fn snapshot_and_management_changes_before_or_during_query_reject_results() {
    for stop in [false, true] {
        for during in [false, true] {
            let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
            if !during {
                fixture.edit(stop);
            }
            assert_eq!(
                inspect_with(&fixture.store, &fixture.recovered, |_, _| {
                    assert!(during, "stale admission must not query");
                    fixture.edit(stop);
                    Ok(reply("qualified", 0))
                }),
                Err(RemoteStorageInspectionError::Admission(
                    RemotePanelStatusError::StateChanged
                )),
            );
        }
    }
}

#[test]
fn missing_database_before_or_during_query_is_not_recreated() {
    for during in [false, true] {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let path = fixture.store.path();
        let retained = path.with_file_name("retained.sqlite3");
        let before = std::fs::read(path).expect("database");
        if !during {
            std::fs::rename(path, &retained).expect("retain");
        }
        assert_eq!(
            inspect_with(&fixture.store, &fixture.recovered, |_, _| {
                assert!(during, "missing store must not query");
                std::fs::rename(path, &retained).expect("retain");
                Ok(reply("qualified", 0))
            }),
            Err(RemoteStorageInspectionError::Admission(
                RemotePanelStatusError::StorageUnavailable
            )),
        );
        assert!(!path.exists());
        assert_eq!(std::fs::read(retained).expect("preserved bytes"), before);
    }
}

#[test]
fn wire_requires_complete_exact_input_and_matching_status_exit_pairs() {
    for (name, expected_code, status) in [
        ("qualified", 0, WorkerStorageStatus::Qualified),
        ("unsupported", 1, WorkerStorageStatus::Unsupported),
        ("unavailable", 1, WorkerStorageStatus::Unavailable),
    ] {
        for code in 0..=2 {
            let expected = if code == expected_code {
                Ok(status)
            } else {
                Err(RemoteStorageInspectionError::InvalidResponse)
            };
            assert_eq!(protocol::response(&reply(name, code)), expected);
            let mut early = reply(name, code);
            early.input = InputProgress::Incomplete(protocol::REQUEST.len() as u64);
            assert_eq!(protocol::response(&early), expected);
        }
    }
    for code in [0, 1, 2] {
        assert_eq!(
            protocol::response(&reply("rejected", code)),
            Err(RemoteStorageInspectionError::InvalidResponse)
        );
    }
    for input in [
        InputProgress::Incomplete(0),
        InputProgress::Incomplete(protocol::REQUEST.len() as u64 - 1),
        InputProgress::Incomplete(protocol::REQUEST.len() as u64 + 1),
        InputProgress::Complete(0),
        InputProgress::Complete(protocol::REQUEST.len() as u64 - 1),
        InputProgress::Complete(protocol::REQUEST.len() as u64 + 1),
    ] {
        let mut result = reply("qualified", 0);
        result.input = input;
        assert_eq!(
            protocol::response(&result),
            Err(RemoteStorageInspectionError::QueryFailed)
        );
    }
    for code in [3, 7, 255] {
        assert_eq!(
            protocol::response(&reply("qualified", code)),
            Err(RemoteStorageInspectionError::QueryFailed)
        );
    }
    let mut result = reply("qualified", 0);
    result.status = std::process::ExitStatus::from_raw(9);
    assert_eq!(
        protocol::response(&result),
        Err(RemoteStorageInspectionError::QueryFailed)
    );
}

#[test]
fn malformed_or_excessive_output_never_invents_storage_state() {
    for bytes in [
        b"".as_slice(),
        b"{}",
        b"{\"version\":2,\"status\":\"qualified\"}",
        b"{\"version\":1,\"status\":\"unknown\"}",
        b"{\"version\":1,\"status\":\"qualified\",\"path\":\"private\"}",
        b"{\"version\":1,\"version\":1,\"status\":\"qualified\"}",
        b"{\"version\":1,\"status\":\"qualified\",\"status\":\"qualified\"}",
        b"{\"version\":1,\"status\":\"qualified\"}{}",
    ] {
        assert_eq!(
            protocol::response(&wire(bytes, 0)),
            Err(RemoteStorageInspectionError::InvalidResponse)
        );
    }
    let mut oversized = reply("qualified", 0);
    oversized.output.resize(protocol::RESPONSE_LIMIT + 1, b' ');
    assert_eq!(
        protocol::response(&oversized),
        Err(RemoteStorageInspectionError::InvalidResponse)
    );
}

#[test]
fn failed_queries_leave_saved_state_and_identity_untouched() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let before = fixture.current();
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
    for error in [
        RemoteStorageInspectionError::QueryFailed,
        RemoteStorageInspectionError::Deadline,
        RemoteStorageInspectionError::InvalidResponse,
        RemotePanelStatusError::ClientUnavailable.into(),
    ] {
        let diagnostic = error.to_string();
        let result = inspect_with(&fixture.store, &fixture.recovered, |_, _| Err(error)).expect_err("query error");
        assert_eq!(result.to_string(), diagnostic);
        assert!(!diagnostic.contains("synthetic-worker"));
        assert_eq!(fixture.current(), before);
        assert_eq!(
            std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
            key
        );
    }
}
