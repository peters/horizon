use super::*;
use crate::diagnostic::{Cause, GuardianReason, Operation, Reason};
use crate::{Error, Kind, Request, Spec, storage::Directory};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

struct Fixture {
    root: tempfile::TempDir,
    state: tempfile::TempDir,
    spec: Spec,
    log: DiagnosticLog,
}
impl Fixture {
    fn new() -> Self {
        Self::under(&std::env::temp_dir())
    }
    fn under(parent: &Path) -> Self {
        fn private(parent: &Path) -> tempfile::TempDir {
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(parent.canonicalize().unwrap())
                .unwrap()
        }
        let root = private(parent);
        let state = private(parent);
        let spec = Request::new(root.path(), state.path(), vec!["synthetic".into()], Kind::Build, 1, 2)
            .unwrap()
            .spec;
        let directory = Directory::open(state.path()).unwrap();
        directory
            .save(&serde_json::json!({"operation":spec.operation,"guardian_pid":std::process::id(),"complete":false}))
            .unwrap();
        let log = DiagnosticLog::create(&spec, std::process::id(), directory).unwrap();
        Self { root, state, spec, log }
    }
    fn capture(&self) -> DiagnosticLog {
        DiagnosticLog::capture(
            &self.spec,
            std::process::id(),
            Directory::open(self.state.path()).unwrap(),
        )
        .unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.state.path().join("output.log")).unwrap()
    }
    fn marker(&self, value: &serde_json::Value) {
        let mut file = Directory::open(self.state.path())
            .unwrap()
            .new_file("host-diagnostic.json")
            .unwrap();
        serde_json::to_writer(&mut file, value).unwrap();
        file.sync_all().unwrap();
    }
}
fn cause() -> Cause {
    Cause::State {
        operation: Operation::Guardian,
        reason: Reason::ChannelClosed,
    }
}

#[test]
fn trusted_temporary_parent_alias_is_canonicalized_without_accepting_aliased_state() {
    let parent = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let canonical = parent.path().canonicalize().unwrap();
    let alias = canonical.join("trusted-parent-alias");
    std::os::unix::fs::symlink(&canonical, &alias).unwrap();
    let fixture = Fixture::under(&alias);
    let aliased_state = alias.join(fixture.state.path().file_name().unwrap());
    assert!(matches!(
        Request::new(
            fixture.root.path(),
            &aliased_state,
            vec!["synthetic".into()],
            Kind::Build,
            1,
            2
        ),
        Err(Error::StateUnavailable)
    ));
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Ok(Retention::Recorded)
    );
}

#[test]
fn shared_physical_cap_reserves_one_host_record_during_concurrent_output() {
    let fixture = Fixture::new();
    let host = fixture.capture();
    let mut writers = Vec::new();
    for log in [fixture.log.clone(), fixture.log.clone(), host.clone()] {
        writers.push(std::thread::spawn(move || {
            let bytes = [b'x'; 8192];
            for _ in 0..600 {
                log.append(&bytes).unwrap();
            }
        }));
    }
    assert_eq!(host.record(cause(), Duration::from_secs(2)), Ok(Retention::Recorded));
    for writer in writers {
        writer.join().unwrap();
    }
    let output = fixture.bytes();
    assert!(output.len() <= usize::try_from(MAX_BYTES).unwrap());
    assert!(output.len() >= usize::try_from(MAX_BYTES - DIAGNOSTIC_BYTES).unwrap());
    assert_eq!(
        String::from_utf8(output)
            .unwrap()
            .matches("app_host_unavailable: Guardian: ChannelClosed")
            .count(),
        1
    );
    assert_eq!(
        fixture.capture().record(cause(), Duration::from_secs(1)),
        Ok(Retention::AlreadyRecorded)
    );
}

#[test]
fn full_child_output_and_guardian_telemetry_cannot_take_the_host_reserve() {
    let fixture = Fixture::new();
    fixture
        .log
        .append(&vec![b'x'; usize::try_from(MAX_BYTES).unwrap()])
        .unwrap();
    fixture.log.guardian(GuardianReason::ParentDisconnected);
    assert_eq!(
        fixture.bytes().len(),
        usize::try_from(MAX_BYTES - DIAGNOSTIC_BYTES).unwrap()
    );
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Ok(Retention::Recorded)
    );
    assert!(fixture.bytes().len() <= usize::try_from(MAX_BYTES).unwrap());
}

#[test]
fn cloned_and_independently_captured_handles_retain_only_the_first_cause() {
    let fixture = Fixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut writers = Vec::new();
    for index in 0..8 {
        let log = if index % 2 == 0 {
            fixture.log.clone()
        } else {
            fixture.capture()
        };
        let barrier = barrier.clone();
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            log.record(cause(), Duration::from_secs(2)).unwrap()
        }));
    }
    let outcomes: Vec<_> = writers.into_iter().map(|writer| writer.join().unwrap()).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Retention::Recorded)
            .count(),
        1
    );
    assert_eq!(
        String::from_utf8(fixture.bytes())
            .unwrap()
            .matches("app_host_unavailable")
            .count(),
        1
    );
}

#[test]
fn partial_marker_cannot_be_completed_by_child_bytes_or_retry() {
    let fixture = Fixture::new();
    fixture.marker(&serde_json::json!({"operation":fixture.spec.operation,"offset":0,"cause":cause()}));
    assert!(fixture.log.append(b"later-child-bytes").is_err());
    assert!(fixture.log.record(cause(), Duration::from_secs(1)).is_err());
    assert_eq!(fixture.bytes(), Vec::<u8>::new());
    // An outside writer does not make a prepared record trustworthy or authorize truncation.
    std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.state.path().join("output.log"))
        .unwrap()
        .write_all(b"later-child-bytes")
        .unwrap();
    assert!(fixture.capture().record(cause(), Duration::from_secs(1)).is_err());
    assert_eq!(fixture.bytes(), b"later-child-bytes");
}

#[test]
fn child_marker_text_does_not_claim_first_cause_retention() {
    let fixture = Fixture::new();
    fixture
        .log
        .append(b"native_host_diagnostic app_host_unavailable: forged\n")
        .unwrap();
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Ok(Retention::Recorded)
    );
}

#[test]
fn replacement_symlink_hardlink_and_receipt_tamper_fail_closed() {
    for tamper in 0..5 {
        let fixture = Fixture::new();
        let path = fixture.state.path().join("output.log");
        assert_eq!(
            fixture.log.record(cause(), Duration::from_secs(1)),
            Ok(Retention::Recorded)
        );
        match tamper {
            0 => {
                std::fs::rename(&path, fixture.state.path().join("old-output")).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .unwrap();
            }
            1 => {
                std::fs::rename(&path, fixture.state.path().join("old-output")).unwrap();
                std::os::unix::fs::symlink("old-output", &path).unwrap();
            }
            2 => std::fs::hard_link(&path, fixture.state.path().join("foreign-link")).unwrap(),
            3 => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap(),
            _ => Directory::open(fixture.state.path())
                .unwrap()
                .save(&serde_json::json!({"operation":uuid::Uuid::new_v4(),"guardian_pid":std::process::id()}))
                .unwrap(),
        }
        assert!(fixture.log.record(cause(), Duration::from_secs(1)).is_err());
    }
}

#[test]
fn held_root_and_state_visibility_are_required_even_after_first_record() {
    for root in [true, false] {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.log.record(cause(), Duration::from_secs(1)),
            Ok(Retention::Recorded)
        );
        let path = if root {
            fixture.root.path()
        } else {
            fixture.state.path()
        };
        let moved = path.with_extension("moved");
        std::fs::rename(path, &moved).unwrap();
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(fixture.log.record(cause(), Duration::from_secs(1)).is_err());
        std::fs::remove_dir(path).unwrap();
        std::fs::rename(moved, path).unwrap();
    }
}

#[test]
fn lock_contention_has_a_bounded_unconfirmed_result_without_append() {
    let fixture = Fixture::new();
    let file = Directory::open(fixture.state.path())
        .unwrap()
        .existing_file("output.log", true)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        fixture.log.record(cause(), Duration::from_millis(30)),
        Err(DiagnosticError::Timeout)
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(50));
    rustix::fs::flock(&file, rustix::fs::FlockOperation::Unlock).unwrap();
    assert_eq!(fixture.bytes(), Vec::<u8>::new());
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Ok(Retention::Recorded)
    );
}

#[test]
fn a_failed_sync_cannot_become_an_acknowledgement_without_new_durability_checks() {
    let fixture = Fixture::new();
    fixture.log.fail_sync_at(2); // Complete bytes exist, but the log's first fsync failed.
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Err(DiagnosticError::Io(std::io::ErrorKind::Other))
    );
    assert_eq!(fixture.log.sync_count(), 2);
    assert!(
        String::from_utf8(fixture.bytes())
            .unwrap()
            .contains("app_host_unavailable")
    );
    fixture.log.fail_sync_at(3); // A repeat must fsync the existing marker, not trust cached bytes.
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Err(DiagnosticError::Io(std::io::ErrorKind::Other))
    );
    assert_eq!(fixture.log.sync_count(), 3);
    fixture.log.fail_sync_at(0);
    assert_eq!(
        fixture.log.record(cause(), Duration::from_secs(1)),
        Ok(Retention::AlreadyRecorded)
    );
    assert_eq!(fixture.log.sync_count(), 6); // Marker, log and directory all became durable.
    assert_eq!(
        String::from_utf8(fixture.bytes())
            .unwrap()
            .matches("app_host_unavailable")
            .count(),
        1
    );
}
