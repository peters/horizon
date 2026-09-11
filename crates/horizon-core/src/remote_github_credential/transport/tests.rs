use super::*;
use crate::remote_worker_ssh::prepared_command;

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    command
}

#[test]
fn strict_reply_rejects_unknown_fields_versions_statuses_duplicates_and_trailing_data() {
    for bytes in [
        "",
        "{}",
        "null",
        "{\"version\":2,\"status\":\"installed\"}",
        "{\"version\":1,\"status\":\"rejected\"}",
        "{\"version\":1,\"status\":\"installed\",\"secret\":\"synthetic\"}",
        "{\"version\":1,\"version\":1,\"status\":\"installed\"}",
        "{\"version\":1,\"status\":\"present\"}{}",
    ] {
        assert_eq!(response(bytes.as_bytes()), Err(Error::DeliveryUnknown));
    }
    assert_eq!(response(&vec![b' '; RESPONSE_LIMIT + 1]), Err(Error::DeliveryUnknown));
}

#[test]
fn real_child_consumes_token_only_on_stdin_and_reports_both_success_states() {
    for (status, expected) in [
        ("installed", RemoteCredentialInstallation::Installed),
        ("present", RemoteCredentialInstallation::Present),
    ] {
        let script = format!(
            "value=$(cat); [ \"$value\" = synthetic_PAT_123 ] || exit 9; printf '%s\\n' '{{\"version\":1,\"status\":\"{status}\"}}'"
        );
        let command = shell(&script);
        let token = RepositoryPat::new("synthetic_PAT_123").expect("synthetic");
        assert_eq!(exchange(command, &token, Duration::from_secs(2)), Ok(expected));
    }
}

#[test]
fn real_child_failures_and_unbounded_output_are_unknown_without_secret_diagnostics() {
    for (script, timeout) in [
        ("cat >/dev/null; exit 1", Duration::from_secs(2)),
        ("cat >/dev/null; exit 255", Duration::from_secs(2)),
        ("cat >/dev/null; exec sleep 5", Duration::from_millis(30)),
        ("cat >/dev/null; head -c 1025 /dev/zero", Duration::from_secs(2)),
        ("cat; printf synthetic_PAT_123 >&2", Duration::from_secs(2)),
        (
            "cat >/dev/null; printf '%s' '{\"version\":1,\"status\":\"installed\"}'; exit 1",
            Duration::from_secs(2),
        ),
    ] {
        let error = exchange(
            shell(script),
            &RepositoryPat::new("synthetic_PAT_123").expect("synthetic"),
            timeout,
        )
        .expect_err("unknown");
        assert_eq!(error, Error::DeliveryUnknown);
        assert!(!format!("{error:?} {error}").contains("synthetic_PAT_123"));
    }
}

#[test]
fn fixed_command_preserves_pinned_ssh_isolation_and_carries_no_token_metadata() {
    let fixture = crate::remote_repository_pack::tests::Fixture::new(Some(
        crate::cloud_run::interactive_worker::InteractiveWorkerLifecycle::Ready,
    ));
    let identity = fixture.recovered.identity();
    let endpoint = fixture
        .recovered
        .observation()
        .and_then(|value| value.ssh.as_ref())
        .expect("ssh");
    let trust = known_hosts(identity, endpoint).expect("trust");
    let baseline = prepared_command(identity.private_key_path(), trust.path(), endpoint).expect("baseline");
    let command = prepared_github_install(identity.private_key_path(), trust.path(), endpoint).expect("install");
    let mut expected: Vec<_> = baseline.get_args().map(std::ffi::OsStr::to_os_string).collect();
    *expected.last_mut().expect("command") = "/usr/local/bin/horizon-github-credential install".into();
    assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        baseline.get_envs().collect::<Vec<_>>()
    );
    assert!(!format!("{command:?}").contains("synthetic_PAT_123"));
}

#[test]
fn lease_caps_the_transport_but_persistent_workers_keep_fifteen_seconds() {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let normal = Duration::from_secs(15);
    assert_eq!(lease_timeout(normal, None, now), Ok(normal));
    assert_eq!(
        lease_timeout(normal, Some(now + time::Duration::seconds(60)), now),
        Ok(normal)
    );
    assert_eq!(
        lease_timeout(normal, Some(now + time::Duration::milliseconds(250)), now),
        Ok(Duration::from_millis(250))
    );
    for expired in [now, now - time::Duration::seconds(1)] {
        assert_eq!(lease_timeout(normal, Some(expired), now), Err(Error::ExpiredWorker));
    }
}

#[test]
fn known_input_requires_every_byte_and_success_even_with_a_valid_reply() {
    use std::os::unix::process::ExitStatusExt;
    for complete in [false, true] {
        for written in [0, 12, 13, 14] {
            for status in [0, 1] {
                let result = query::Exchange {
                    input: if complete {
                        query::InputProgress::Complete(written)
                    } else {
                        query::InputProgress::Incomplete(written)
                    },
                    status: std::process::ExitStatus::from_raw(status << 8),
                    output: br#"{"version":1,"status":"installed"}"#.to_vec(),
                };
                assert_eq!(known_response(&result, 13).is_ok(), written == 13 && status == 0);
            }
        }
    }
}

#[test]
fn expiry_after_spawn_prevents_the_first_stdin_write() {
    use std::cell::Cell;
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let deadline = now + time::Duration::seconds(1);
    let samples = Cell::new(0);
    let result = exchange_before(
        shell("cat >/dev/null; printf '%s' '{\"version\":1,\"status\":\"installed\"}'"),
        &RepositoryPat::new("synthetic_PAT_123").expect("synthetic"),
        Duration::from_secs(15),
        Some(deadline),
        || {
            let sample = samples.get() + 1;
            samples.set(sample);
            // Construction and pre-spawn admission see a valid lease; the
            // first post-spawn admission sees expiry, without scheduling sleeps.
            if sample < 3 { now } else { deadline }
        },
    );
    assert_eq!(samples.get(), 3);
    assert_eq!(result, Err(Error::DeliveryUnknown));
}

#[test]
fn admission_is_rechecked_immediately_before_first_write_and_on_later_writes() {
    use std::cell::Cell;
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let deadline = now + time::Duration::seconds(1);
    // Sample 5 is immediately before the first write: initial clock, pre-spawn,
    // loop admission, after token read, pre-write. A later sample exercises the
    // next admission while a child that never reads keeps the pipe backpressured.
    for expiry_sample in [5, 7] {
        let samples = Cell::new(0);
        let token = "s".repeat(16_384);
        let result = exchange_before(
            shell("exec sleep 5"),
            &RepositoryPat::new(&token).expect("synthetic"),
            Duration::from_secs(15),
            Some(deadline),
            || {
                let sample = samples.get() + 1;
                samples.set(sample);
                if sample < expiry_sample { now } else { deadline }
            },
        );
        assert_eq!(samples.get(), expiry_sample);
        assert_eq!(result, Err(Error::DeliveryUnknown));
    }
}
