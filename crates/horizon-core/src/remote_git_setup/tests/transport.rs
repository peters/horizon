use super::super::{transport::*, *};
use crate::remote_worker_ssh::{
    prepared_command, prepared_git_setup,
    query::{Exchange, InputProgress},
};
use std::{
    cell::Cell,
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Command, ExitStatus},
    time::Duration,
};

const SUBMITTED: &[u8] = br#"{"version":1,"state":"submitted","observation":null}"#;

#[test]
fn full_input_is_required_even_for_early_complete_or_nonzero_responses() {
    for complete in [false, true] {
        for written in [0, 3, 4, 5] {
            let input = if complete {
                InputProgress::Complete(written)
            } else {
                InputProgress::Incomplete(written)
            };
            let exchange = Exchange {
                status: ExitStatus::from_raw(0),
                input,
                output: SUBMITTED.into(),
            };
            assert_eq!(known_response(&exchange, 4, false).is_ok(), written == 4);
        }
    }
    let exchange = Exchange {
        status: ExitStatus::from_raw(256),
        input: InputProgress::Complete(4),
        output: br#"{"version":1,"state":"handoff_unconfirmed","observation":null}"#.into(),
    };
    assert_eq!(known_response(&exchange, 4, false), Ok(RemoteGitSubmission::Unknown));
}

#[test]
fn persistent_and_remaining_lease_budgets_are_explicit() {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    for observe in [true, false] {
        assert_eq!(
            lease_timeout(observe, None, now),
            Ok(Duration::from_secs(if observe { 40 } else { 60 }))
        );
        assert_eq!(
            lease_timeout(observe, Some(now + time::Duration::seconds(2)), now),
            Ok(Duration::from_secs(2))
        );
        assert_eq!(
            lease_timeout(observe, Some(now), now),
            Err(RemoteGitSetupError::ExpiredWorker)
        );
    }
}

#[test]
fn expired_before_spawn_or_first_write_never_delivers_input() {
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        exchange_before(Command::new("/nonexistent"), b"bound request", false, Some(now), || now),
        Err(RemoteGitSetupError::ExpiredWorker)
    );
    // Clock calls: budget, pre-spawn, post-spawn admission. Move the absolute
    // deadline forward deterministically, rather than racing a sleeping child.
    let calls = Cell::new(0);
    let mut child = Command::new("/bin/cat");
    child.env_clear();
    let result = exchange_before(
        child,
        b"bound request",
        false,
        Some(now + time::Duration::seconds(1)),
        || {
            let count = calls.get();
            calls.set(count + 1);
            now + time::Duration::seconds(if count >= 2 { 2 } else { 0 })
        },
    );
    assert_eq!(result, Err(RemoteGitSetupError::OutcomeUnknown));
    assert_eq!(calls.get(), 3);
}

#[test]
fn forward_clock_jump_after_initial_write_revokes_further_input() {
    let now = time::OffsetDateTime::now_utc();
    let calls = Cell::new(0);
    let mut child = Command::new("/bin/cat");
    child.env_clear();
    // Exercise multiple stream chunks inside this private transport test. Public
    // requests remain limited to one 16 KiB frame by the worker request decoder.
    let input = vec![b'x'; 48 * 1024];
    let result = exchange_before(child, &input, false, Some(now + time::Duration::seconds(1)), || {
        let count = calls.get();
        calls.set(count + 1);
        now + time::Duration::seconds(if count >= 5 { 2 } else { 0 })
    });
    assert_eq!(result, Err(RemoteGitSetupError::OutcomeUnknown));
    assert_eq!(calls.get(), 6);
}

#[test]
fn actual_child_nonzero_frame_is_retained_and_status_never_submits() {
    for (observe, reply, code, expected) in [
        (
            false,
            "{\"version\":1,\"state\":\"handoff_unconfirmed\",\"observation\":null}",
            "1",
            RemoteGitSubmission::Unknown,
        ),
        (
            true,
            "{\"version\":1,\"state\":\"claimed_unknown\",\"reason\":null,\"checkout\":null}",
            "1",
            RemoteGitSubmission::Observed(RemoteGitObservation {
                state: RemoteGitState::ClaimedUnknown,
                reason: None,
            }),
        ),
    ] {
        let mut child = Command::new("/bin/sh");
        child.env_clear().args([
            "-c",
            "cat >/dev/null; printf '%s' \"$1\"; exit \"$2\"",
            "fixture",
            reply,
            code,
        ]);
        assert_eq!(
            exchange_before(
                child,
                b"synthetic request",
                observe,
                None,
                time::OffsetDateTime::now_utc
            ),
            Ok(expected)
        );
    }
}

#[test]
fn fixed_git_commands_preserve_all_key_only_ssh_options() {
    let fixture = super::fixture();
    let endpoint = fixture.recovered.observation().unwrap().ssh.as_ref().unwrap();
    let baseline = prepared_command(Path::new("/private/key"), Path::new("/private/trust"), endpoint).unwrap();
    for (observe, operation) in [
        (false, "/usr/local/bin/horizon-setup-launch --git"),
        (true, "/usr/local/bin/horizon-repository git-status"),
    ] {
        let command = prepared_git_setup(
            Path::new("/private/key"),
            Path::new("/private/trust"),
            endpoint,
            observe,
        )
        .unwrap();
        let mut expected: Vec<_> = baseline.get_args().map(std::ffi::OsStr::to_os_string).collect();
        *expected.last_mut().unwrap() = operation.into();
        assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            baseline.get_envs().collect::<Vec<_>>()
        );
    }
}
