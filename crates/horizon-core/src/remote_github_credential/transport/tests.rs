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
