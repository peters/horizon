use super::*;

#[test]
fn finite_causes_round_trip_without_accepting_caller_text() {
    for cause in [
        Cause::LockPoisoned {
            resource: Lock::Guardian,
        },
        Cause::State {
            operation: Operation::Session,
            reason: Reason::TaskPanicked,
        },
        Cause::Io {
            operation: Operation::Output,
            kind: ErrorKind::BrokenPipe,
        },
    ] {
        let bytes = serde_json::to_vec(&cause).unwrap();
        assert_eq!(serde_json::from_slice::<Cause>(&bytes).unwrap(), cause);
        assert!(bytes.len() < 256);
    }
    for value in [
        r#"{"State":{"operation":"Session","reason":"TaskPanicked","message":"private-secret"}}"#,
        r#"{"Io":{"operation":"Output","kind":"private-secret"}}"#,
        r#"{"State":{"operation":"private-path","reason":"TaskPanicked"}}"#,
    ] {
        assert!(serde_json::from_str::<Cause>(value).is_err());
    }
    assert_eq!(
        Diagnostic::Host(Cause::State {
            operation: Operation::Session,
            reason: Reason::TaskPanicked
        })
        .message(),
        "app_host_unavailable: Session: TaskPanicked"
    );
    assert!(
        !DiagnosticError::from(std::io::Error::other("private-secret"))
            .to_string()
            .contains("private-secret")
    );
}

#[cfg(unix)]
#[test]
fn unclassified_os_io_causes_use_one_finite_round_trip_code() {
    let kind = std::io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error()).kind();
    let cause = Cause::Io {
        operation: Operation::Output,
        kind,
    };
    let bytes = serde_json::to_vec(&cause).unwrap();
    let retained: Cause = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(retained.to_string(), cause.to_string());
    assert_eq!(
        retained,
        Cause::Io {
            operation: Operation::Output,
            kind: io_kind::normalize(kind)
        }
    );
}

#[test]
fn archive_causes_preserve_the_hosts_exact_primary_wording() {
    for (failure, message) in [
        (
            HostFailure::EvidenceBytes,
            "the retained evidence exceeded its 1 GiB byte limit",
        ),
        (
            HostFailure::EvidenceFiles,
            "the retained evidence exceeded its 1024 file limit",
        ),
        (
            HostFailure::ReportLimit,
            "the terminal report exceeded its 8 MiB limit or was already saved",
        ),
        (
            HostFailure::CaptureInvalid,
            "the capture was empty or exceeded its byte limit",
        ),
        (HostFailure::ArchiveState, "the evidence archive state lock failed"),
        (
            HostFailure::ArchiveSerialization,
            "the evidence archive serialization failed",
        ),
        (
            HostFailure::ArchiveWrite(ErrorKind::PermissionDenied),
            "the evidence archive write failed (PermissionDenied)",
        ),
    ] {
        let cause = Cause::Archive(failure);
        assert_eq!(cause.to_string(), message);
        assert_eq!(
            Diagnostic::Host(cause).message(),
            format!("app_host_unavailable: {message}")
        );
        assert_eq!(
            serde_json::from_slice::<Cause>(&serde_json::to_vec(&cause).unwrap()).unwrap(),
            cause
        );
    }
    assert!(serde_json::from_str::<HostFailure>(r#"{"ArchiveWrite":"private-secret"}"#).is_err());
    #[cfg(unix)]
    {
        let kind = std::io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error()).kind();
        let cause = Cause::Archive(HostFailure::ArchiveWrite(kind));
        let retained: Cause = serde_json::from_slice(&serde_json::to_vec(&cause).unwrap()).unwrap();
        assert_eq!(retained.to_string(), cause.to_string());
    }
}
