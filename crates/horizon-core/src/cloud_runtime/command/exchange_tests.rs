use super::*;
use std::cell::RefCell;

#[test]
fn failed_private_exchange_reports_no_unavailable_logs_or_remote_output() {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private output was emitted"),
        secrets: Vec::new(),
    };
    let error = runner
        .private_exchange(
            Command::new("sh").args(["-c", "printf 'private stdout'; printf 'private stderr' >&2; exit 1"]),
            b"private request",
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert!(matches!(error, Error::PrivateTransport));
    assert_eq!(
        error.to_string(),
        "Private worker request failed; its output is intentionally not logged"
    );
}

#[test]
fn private_exchange_keeps_exact_bytes_and_never_emits_either_stream() {
    let cancel = Cancellation::default();
    let events = RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let payload = b"private-fixture\n\xff\x00";
    let output = runner
        .private_exchange(
            Command::new("sh").args(["-c", "cat; printf 'private-stderr' >&2"]),
            payload,
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output, payload);
    assert!(events.borrow().is_empty());
}

#[test]
fn private_exchange_bounds_requests_replies_and_process_lifetime() {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private output was emitted"),
        secrets: Vec::new(),
    };
    assert!(
        runner
            .private_exchange(
                &mut Command::new("missing-program"),
                &vec![0; 65537],
                Duration::from_secs(1)
            )
            .is_err()
    );
    assert!(
        runner
            .private_exchange(
                Command::new("sh").args(["-c", "head -c 65537 /dev/zero"]),
                b"{}",
                Duration::from_secs(5),
            )
            .is_err()
    );
    let started = Instant::now();
    assert!(
        runner
            .private_exchange(
                Command::new("sh").args(["-c", "sleep 30 & exit 0"]),
                b"{}",
                Duration::from_millis(100),
            )
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn private_exchange_cancellation_stops_transport_without_emitting_output() {
    let cancel = Cancellation::default();
    let cloned = cancel.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cloned.cancel();
    });
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private output was emitted"),
        secrets: Vec::new(),
    };
    let started = Instant::now();
    assert!(
        runner
            .private_exchange(
                Command::new("sh").args(["-c", "cat >/dev/null; sleep 30"]),
                b"{}",
                Duration::from_secs(20),
            )
            .is_err()
    );
    thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
}
