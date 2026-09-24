use super::*;
use horizon_cloud::Cancellation;
use std::cell::RefCell;

#[test]
fn transfers_remove_inherited_auth_without_changing_the_parent_environment() {
    const CHILD: &str = "HORIZON_TRANSFER_ENV_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cloud_runtime::command::terminal_progress::tests::transfers_remove_inherited_auth_without_changing_the_parent_environment",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("DOCKER_AUTH_CONFIG", "synthetic-ambient-auth")
            .env("HORIZON_TRANSFER_REMOVE", "synthetic-inherited-value")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }

    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let literal = "literal spaces ' \" ; $(exit 9)";
    for kind in [Transfer::Image, Transfer::Pull, Transfer::File(1)] {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "test -t 1 && test -z \"${DOCKER_AUTH_CONFIG+x}\" && test -z \"${HORIZON_TRANSFER_REMOVE+x}\" && test \"$HORIZON_TRANSFER_KEEP\" = \"$1\" && test \"$PWD\" = \"$2\"",
                "transfer",
                literal,
                directory.to_str().unwrap(),
            ])
            .current_dir(&directory)
            .env("HORIZON_TRANSFER_KEEP", literal)
            .env_remove("DOCKER_AUTH_CONFIG")
            .env_remove("HORIZON_TRANSFER_REMOVE");
        runner
            .transfer("upload", &command, kind, Duration::from_secs(5))
            .unwrap();
        assert_eq!(std::env::var("DOCKER_AUTH_CONFIG").unwrap(), "synthetic-ambient-auth");
        assert_eq!(
            std::env::var("HORIZON_TRANSFER_REMOVE").unwrap(),
            "synthetic-inherited-value"
        );
    }
}

#[test]
fn real_terminal_exposes_transfer_counters_and_preserves_exit_failures() {
    let cancel = Cancellation::default();
    let events = RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec!["synthetic-secret".into()],
    };
    let mut command = Command::new("sh");
    command.env_remove("DOCKER_AUTH_CONFIG");
    command.args([
        "-c",
        "test -t 1 || exit 9; printf 'aaaaaaaaaaaa: Pushing [==>] 2MB/10MB\r\nsynthetic-secret\r\n'; sleep 0.4; exit 7",
    ]);
    assert!(matches!(
        runner.transfer("upload", &command, Transfer::Image, Duration::from_secs(5)),
        Err(Error::Command("upload"))
    ));
    assert!(events.borrow().iter().any(|event| matches!(event, Event::Progress(progress) if progress.completed == 2_000_000 && progress.total == Some(10_000_000))));
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Output(line) if line.contains("synthetic-secret")))
    );
}

#[test]
fn cancellation_stops_the_owned_transfer_promptly() {
    let cancel = Cancellation::default();
    let descendant = std::cell::Cell::new(None::<u32>);
    let emit = |event| {
        if let Event::Output(line) = event
            && let Some(pid) = line.trim().strip_prefix("transfer-started ")
        {
            descendant.set(Some(pid.parse().unwrap()));
            cancel.cancel();
        }
    };
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let mut command = Command::new("sh");
    command.env_remove("DOCKER_AUTH_CONFIG");
    command.args(["-c", "sleep 30 & printf 'transfer-started %s\r\n' \"$!\"; wait"]);
    let started = Instant::now();
    assert!(matches!(
        runner.transfer("upload", &command, Transfer::Image, Duration::from_secs(10)),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    assert!(started.elapsed() < Duration::from_secs(4));
    let pid = descendant.get().expect("transfer reported its child");
    let deadline = Instant::now() + Duration::from_secs(2);
    let stopped = loop {
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8(state.stdout).unwrap();
        if state.trim().is_empty() || state.trim_start().starts_with('Z') {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !stopped {
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    }
    assert!(stopped, "cancellation left transfer child {pid} running");
}

#[test]
fn file_transfer_deadline_allows_progress_but_bounds_stalls_and_total_duration() {
    let mut file = super::TransferDeadline::new(true, Duration::from_secs(600));
    let start = file.started;
    assert!(!file.expired(start + Duration::from_secs(590), 1));
    assert!(!file.expired(start + Duration::from_secs(1100), 2));
    assert!(!file.expired(start + Duration::from_secs(1600), 2));
    assert!(file.expired(start + Duration::from_secs(1701), 2));
    assert!(file.expired(
        start + super::TransferDeadline::MAX_FILE_DURATION + Duration::from_secs(1),
        3
    ));
    let mut image = super::TransferDeadline::new(false, Duration::from_secs(600));
    assert!(image.expired(image.started + Duration::from_secs(601), 1));
}

#[test]
fn file_liveness_advances_with_bytes_while_percentage_is_unchanged() {
    let mut counters = counters::Counters::new(Transfer::File(3_100_000_000));
    let mut deadline = TransferDeadline::new(true, Duration::from_secs(600));
    let start = deadline.started;
    for second in [500, 1000, 1500] {
        counters.observe(&format!("archive.tar 0% {second}KB 1.0KB/s 00:10 ETA"));
        let progress = counters.snapshot("source");
        assert_eq!(progress.completed, second * 1024);
        assert!(!deadline.expired(start + Duration::from_secs(second), counters.activity()));
    }
    counters.observe("archive.tar 0% 500KB 0.0KB/s 00:10 ETA");
    assert!(deadline.expired(start + Duration::from_secs(2101), counters.activity()));
}

#[test]
fn large_file_liveness_tracks_percent_when_scp_rounds_bytes_to_whole_gigabytes() {
    let mut counters = counters::Counters::new(Transfer::File(12 * 1024_u64.pow(3)));
    let mut deadline = TransferDeadline::new(true, Duration::from_secs(600));
    let start = deadline.started;
    for (second, percent) in [(500, 88), (1000, 89), (1500, 90)] {
        counters.observe(&format!("archive.tar {percent}% 11GB 1.0MB/s 00:10 ETA"));
        assert_eq!(counters.snapshot("source").completed, 11 * 1024_u64.pow(3));
        assert!(!deadline.expired(start + Duration::from_secs(second), counters.activity()));
    }
    assert!(deadline.expired(start + Duration::from_secs(2101), counters.activity()));
}
