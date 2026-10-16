use super::*;
#[cfg(target_os = "linux")]
#[test]
fn bounded_file_stops_a_live_producer_before_any_excess_bytes_are_written() {
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    for oversized in [false, true] {
        let mut output = tempfile::tempfile().unwrap();
        let script = if oversized {
            "head -c 65536 /dev/zero; sleep 30"
        } else {
            "head -c 4096 /dev/zero"
        };
        let started = Instant::now();
        let result = runner.bounded_file(
            Command::new("sh").args(["-c", script]).stdin(Stdio::null()),
            &mut output,
            4096,
            Duration::from_secs(10),
        );
        assert_eq!(result.is_err(), oversized);
        assert!(output.metadata().unwrap().len() <= 4096);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn a_timed_out_docker_command_is_a_stuck_daemon_only_when_docker_does_not_answer() {
    use std::path::Path;
    let temp = tempfile::tempdir().unwrap();
    let (cancel, lines) = (Cancellation::default(), std::cell::RefCell::new(Vec::new()));
    let emit = |event| {
        if let Event::Output(line) = event {
            lines.borrow_mut().push(line);
        }
    };
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    let create = |program: &Path| {
        let mut command = Command::new(program);
        command.args(["create", "--name", "horizon-contract-1"]);
        runner.run(
            "worker image contract creation",
            &mut command,
            Duration::from_millis(150),
        )
    };
    // Docker answers its health check: the timeout is a plain one.
    let docker = docker_daemon::fake_docker(
        temp.path(),
        "[ \"$1\" = version ] && echo 29.8.1 && exit\nexec sleep 30",
    );
    assert!(matches!(create(&docker), Err(Error::Invalid(TIMED_OUT))));
    assert!(lines.borrow().is_empty());
    // Docker answers nothing: the daemon is stuck.
    let docker = docker_daemon::fake_docker(temp.path(), "exec sleep 30");
    let result = create(&docker);
    assert!(
        matches!(
            result,
            Err(Error::DockerNotResponding("worker image contract creation"))
        ),
        "{result:?}"
    );
    assert_eq!(*lines.borrow(), ["Docker did not answer docker version within 5 s"]);
}

#[test]
fn descendant_inheriting_output_cannot_bypass_deadline() {
    let cancel = Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    let started = Instant::now();
    let result = runner.run(
        "test",
        Command::new("sh").args(["-c", "sleep 30 & exit 0"]),
        Duration::from_millis(150),
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn cancellation_interrupts_in_flight_image_commands_and_their_descendants() {
    for name in ["image build", "image push"] {
        let cancel = Cancellation::default();
        let descendant = std::cell::Cell::new(None::<u32>);
        let emit = |event| {
            if let Event::Output(line) = event
                && let Some(pid) = line.trim().strip_prefix("operation-started ")
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
        let started = Instant::now();
        let result = runner.run(
            name,
            Command::new("sh").args(["-c", "sleep 30 & printf 'operation-started %s\\n' \"$!\"; wait"]),
            Duration::from_secs(10),
        );
        assert!(matches!(
            result,
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        let pid = descendant.get().expect("operation reported its live child");
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
            thread::sleep(Duration::from_millis(10));
        };
        if !stopped {
            let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
        }
        assert!(stopped, "cancellation left child {pid} running");
    }
}
#[test]
fn stderr_does_not_corrupt_stdout_and_secrets_are_redacted() {
    let cancel = Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec!["secret".into()],
    };
    let output = runner
        .run(
            "test",
            Command::new("sh").args(["-c", "printf secret; printf diagnostic >&2"]),
            Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(output, "[REDACTED]");
}

#[test]
fn parsed_stdout_stays_out_of_the_output_and_stderr_shows() {
    let events = std::cell::RefCell::new(Vec::new());
    let cancel = Cancellation::default();
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    let status = r#"echo '{"state":"ok","last_error":null}'"#;
    let output = runner
        .run_parsed("test", Command::new("sh").args(["-c", status]), Duration::from_secs(2))
        .unwrap();
    assert_eq!(output.trim(), r#"{"state":"ok","last_error":null}"#);
    let failing = format!("{status}; echo denied >&2; exit 1");
    let error = runner
        .run_parsed(
            "test",
            Command::new("sh").args(["-c", &failing]),
            Duration::from_secs(2),
        )
        .unwrap_err();
    assert!(matches!(error, Error::Command("test")));
    let lines: Vec<String> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Output(line) => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(lines, ["denied"], "only stderr is output");
}

#[test]
fn private_authentication_input_cannot_reach_progress_output() {
    use std::os::unix::fs::PermissionsExt;
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), "synthetic-private-credential").unwrap();
    std::fs::set_permissions(input.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    let events = std::cell::RefCell::new(Vec::new());
    let cancel = Cancellation::default();
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    runner
        .private_input(
            Command::new("sh").args(["-c", "value=$(cat); printf '%s' \"$value\"; printf '%s' \"$value\" >&2"]),
            input.path(),
        )
        .unwrap();
    assert!(events.borrow().is_empty());
    std::fs::set_permissions(input.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        runner
            .private_input(Command::new("cat").arg("-"), input.path())
            .is_err()
    );
}
