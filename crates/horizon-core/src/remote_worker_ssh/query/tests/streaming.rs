use super::*;

#[test]
fn known_input_accepts_exact_written_bytes_without_reading_source_eof() {
    struct NoEof<'a>(&'a [u8]);
    impl Read for NoEof<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(!self.0.is_empty(), "the known-length query must not probe source EOF");
            self.0.read(bytes)
        }
    }
    for input in [b"".as_slice(), b"known exact input".as_slice()] {
        let command = || {
            let mut command = Command::new("/usr/bin/head");
            command.args(["-c", &input.len().to_string()]);
            command
        };
        let result = exchange(
            command(),
            &mut NoEof(input),
            Duration::from_secs(2),
            input.len(),
            || false,
            Some(input.len() as u64),
        )
        .expect("known handoff");
        assert!(matches!(result.input, InputProgress::Incomplete(written) if written == input.len() as u64));
        assert_eq!(known_response(result, input.len() as u64), Ok(input.to_vec()));
        assert_eq!(
            run(command(), input, Duration::from_secs(2), input.len()),
            Ok(input.to_vec())
        );
    }
}

#[test]
fn known_failures_do_not_wait_for_inherited_stdout_but_streaming_does() {
    for (script, input) in [
        ("sleep 0.4 & exit 7", vec![]),
        ("sleep 0.4 & exit 0", vec![b'x'; 1024 * 1024]),
    ] {
        let timeout = Duration::from_millis(100);
        assert_eq!(run(shell(script), &input, timeout, 32), Err(Error::QueryFailed));
        let result = exchange(shell(script), &mut input.as_slice(), timeout, 32, || false, None);
        assert!(matches!(result, Err(Error::Deadline)));
    }
}

#[test]
fn continuous_output_still_observes_cancellation_and_deadline() {
    let checks = std::cell::Cell::new(0);
    let result = exchange(
        shell("exec yes"),
        &mut io::empty(),
        Duration::from_secs(2),
        1024 * 1024,
        || {
            checks.set(checks.get() + 1);
            checks.get() > 100
        },
        None,
    );
    assert!(matches!(result, Err(Error::QueryFailed)));
    assert!(checks.get() > 100);
    let result = exchange(
        shell("exec yes"),
        &mut io::empty(),
        Duration::from_millis(50),
        64 * 1024 * 1024,
        || false,
        None,
    );
    assert!(matches!(result, Err(Error::Deadline)));
}

#[test]
fn source_read_cannot_deliver_bytes_or_eof_after_admission_expires() {
    struct ChangedDuringRead<'a>(&'a std::cell::Cell<bool>, bool);
    impl Read for ChangedDuringRead<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.set(true);
            bytes[0] = b'x';
            Ok(usize::from(!self.1))
        }
    }
    for eof in [false, true] {
        for deadline in [false, true] {
            let changed = std::cell::Cell::new(false);
            let mut input = ChangedDuringRead(&changed, eof);
            let mut child = OwnedChild(
                Command::new("/bin/cat")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .spawn()
                    .expect("child"),
            );
            let mut stdin = child.0.stdin.take();
            let mut complete = false;
            let mut written = 0;
            let result = stream_input(
                &mut stdin,
                &mut input,
                &mut [0; 16],
                &mut (0..0),
                &mut complete,
                &mut written,
                &|| {
                    if changed.get() {
                        Err(if deadline { Error::Deadline } else { Error::QueryFailed })
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result, Err(if deadline { Error::Deadline } else { Error::QueryFailed }));
            assert!(stdin.is_some() && !complete);
            assert_eq!(written, 0);
        }
    }
}

#[test]
fn early_nonzero_response_survives_incomplete_input() {
    let mut input = io::repeat(b'x').take(8 * 1024 * 1024);
    let result = exchange(
        shell("exec 0<&-; printf observed; exit 4"),
        &mut input,
        Duration::from_secs(2),
        32,
        || false,
        None,
    )
    .expect("retained response");
    assert_eq!(result.status.code(), Some(4));
    assert_eq!(result.output, b"observed");
    assert!(matches!(result.input, InputProgress::Incomplete(_)));
    assert!(input.limit() > 0);
}

#[test]
fn broken_pipe_preserves_output_for_draining() {
    let mut child = OwnedChild(
        shell("printf observed; exit 4")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("child"),
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child.0.stdout.take().expect("stdout");
    assert_eq!(child.0.wait().expect("closed reader").code(), Some(4));
    let mut complete = false;
    let mut written = 0;
    assert_eq!(
        stream_input(
            &mut stdin,
            &mut io::repeat(b'x'),
            &mut [0; 16],
            &mut (0..0),
            &mut complete,
            &mut written,
            &|| Ok(())
        ),
        Ok(())
    );
    assert!(stdin.is_none() && !complete);
    assert_eq!(written, 0);
    let mut output = Vec::new();
    assert_eq!(read_available(&mut stdout, &mut output, 32), Ok(false));
    assert_eq!(read_available(&mut stdout, &mut output, 32), Ok(true));
    assert_eq!(output, b"observed");
}

#[test]
fn complete_stream_and_bounded_failures_preserve_query_contract() {
    let input = vec![b'x'; 8 * 1024 * 1024];
    let started = Instant::now();
    let result = exchange(
        shell("exec cat"),
        &mut input.as_slice(),
        Duration::from_secs(20),
        input.len(),
        || false,
        None,
    )
    .expect("stream");
    eprintln!("bounded local stream: {} bytes in {:?}", input.len(), started.elapsed());
    assert!(result.status.success() && matches!(result.input, InputProgress::Complete(_)));
    assert_eq!(result.input.written(), input.len() as u64);
    assert_eq!(result.output, input);
    for (script, limit, deadline, cancel, expected) in [
        ("printf oversized", 2, 2000, false, Error::OutputLimit),
        ("exec sleep 5", 32, 40, false, Error::Deadline),
        ("exit 0", 32, 2000, true, Error::QueryFailed),
    ] {
        let result = exchange(
            shell(script),
            &mut io::empty(),
            Duration::from_millis(deadline),
            limit,
            || cancel,
            None,
        );
        assert!(matches!(result, Err(error) if error == expected));
    }
}
