//! The record of a real profile host, the answer of a real probe, and the
//! command line of a real remote server, rather than a live machine.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::{
    MAX_ACTOR_LEN, RemoteChannel, RemoteChannelError, RemoteHostRecord, create_directory_command, parse_probe,
    probe_command, quote, read_file_command, remove_directory_command, root_expression, server_command,
    stage_file_command, valid_target,
};

const RECORD: &str =
    r#"{"host_instance":"7c1f8a9e-2b44-4f4c-9d3f-3c0b6f6f2a11","pid":4242,"started_at":1761264000000}"#;

#[test]
fn the_probe_answer_gives_the_root_and_the_record_of_the_remote_host() {
    let text = format!("/srv/state\n{RECORD}\n");
    let host = parse_probe(&text).expect("a running host");
    assert_eq!(
        host,
        RemoteHostRecord {
            root: std::path::PathBuf::from("/srv/state"),
            host_instance: "7c1f8a9e-2b44-4f4c-9d3f-3c0b6f6f2a11".to_string(),
        }
    );
}

#[test]
fn a_record_that_no_running_host_wrote_is_not_a_host() {
    assert!(parse_probe("").is_none(), "an empty answer");
    assert!(parse_probe("/srv/state\n").is_none(), "no record");
    assert!(
        parse_probe("/srv/state\n{\"host_instance\":\"\",\"pid\":1}\n").is_none(),
        "a blank instance"
    );
    assert!(
        parse_probe("\n{\"host_instance\":\"instance\",\"pid\":1}\n").is_none(),
        "no root"
    );
    assert!(
        parse_probe(&format!("/srv/state\n{RECORD}\n")).is_some(),
        "the probe answer takes no further line"
    );
}

#[test]
fn a_target_that_ssh_reads_as_an_option_or_a_split_is_refused() {
    for accepted in ["127.0.0.1", "user@browser-vm", "browser-vm", "host.example.net"] {
        assert!(valid_target(accepted), "{accepted}");
    }
    for refused in [
        "",
        "   ",
        "-oProxyCommand=evil",
        "two words",
        "host\nother",
        &"a".repeat(256),
    ] {
        assert!(!valid_target(refused), "{refused:?}");
    }
}

#[test]
fn a_refused_channel_names_its_reason_before_it_opens_a_connection() {
    let error = RemoteChannel::open("-oProxyCommand=evil", "horizon:person:browser-panel")
        .err()
        .expect("a target that ssh reads as an option");
    assert!(
        matches!(&error, RemoteChannelError::InvalidTarget { target } if target == "-oProxyCommand=evil"),
        "{error}"
    );
    let long_actor = "a".repeat(MAX_ACTOR_LEN + 1);
    let error = RemoteChannel::open("127.0.0.1", &long_actor)
        .err()
        .expect("an actor that no page can carry");
    assert!(matches!(error, RemoteChannelError::ActorMissing), "{error}");
    let error = RemoteChannel::open("127.0.0.1", " ").err().expect("an empty actor");
    assert!(matches!(error, RemoteChannelError::ActorMissing), "{error}");
}

#[test]
fn every_ssh_of_the_channel_is_bounded_and_can_not_ask_a_question() {
    let command = super::ssh_command(
        "ssh",
        "browser-vm",
        "horizon-browser mcp --connect --inline-screenshots",
    );
    let args = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for bounded in [
        "BatchMode=yes",
        "ConnectTimeout=5",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=2",
    ] {
        assert!(
            args.iter().any(|argument| argument == bounded),
            "{bounded} is missing: {args:?}"
        );
    }
    assert!(
        args.first().is_some_and(|first| first == "-Tq"),
        "no controlling terminal and a quiet stderr: {args:?}"
    );
    assert_eq!(
        args.iter().rev().take(2).rev().collect::<Vec<_>>(),
        ["browser-vm", "horizon-browser mcp --connect --inline-screenshots"],
        "the target and the remote command stay the last two words: {args:?}"
    );
}

#[test]
fn a_call_that_never_answers_is_killed_at_its_deadline() {
    let started = Instant::now();
    let error = super::run_bounded(Command::new("sh"), 0, Duration::from_millis(300)).err();
    let elapsed = started.elapsed();
    let _ = error;
    // The command needs an argument to hang, so the call above fails at once as
    // a spawn error. This is the case that must not wait either.
    assert!(
        elapsed < Duration::from_secs(2),
        "a call that cannot start returns at once: {elapsed:?}"
    );

    let mut hanging = Command::new("sh");
    hanging.args(["-c", "sleep 30"]);
    let started = Instant::now();
    let error = super::run_bounded(hanging, 0, Duration::from_millis(300))
        .err()
        .expect("a call that never answers");
    let elapsed = started.elapsed();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
    assert!(
        elapsed < Duration::from_secs(3),
        "the deadline ends the call, not the sleep: {elapsed:?}"
    );
}

#[test]
fn a_probe_of_a_stand_in_ssh_that_never_answers_is_a_typed_no_answer() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let program = directory.path().join("ssh");
    std::fs::write(&program, "#!/bin/sh\nsleep 30\n").expect("stand-in ssh");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let started = Instant::now();
    let error = super::probe_with("browser-vm", None, &program.to_string_lossy(), Duration::from_secs(1))
        .expect_err("a host that never answers");
    let elapsed = started.elapsed();
    assert!(
        matches!(&error, RemoteChannelError::NoAnswer { target, seconds } if target == "browser-vm" && *seconds == 1),
        "{error}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the probe keeps its own deadline: {elapsed:?}"
    );
}

#[test]
fn the_remote_server_names_the_forwarded_identity_and_the_capture_flags() {
    let command = server_command("horizon-browser", "horizon:panel-1", "host-instance", None);
    assert_eq!(
        command,
        "env HORIZON_BROWSER_ACTOR='horizon:panel-1' HORIZON_BROWSER_HOST_INSTANCE='host-instance' \
         'horizon-browser' mcp --connect --inline-screenshots"
    );
    assert!(
        !command.contains("HORIZON_BROWSER_ROOT"),
        "an unset root leaves the remote default in place"
    );
    let named = server_command(
        "horizon-browser",
        "horizon:panel-1",
        "host-instance",
        Some(Path::new("/srv/browser state")),
    );
    assert!(
        named.starts_with(
            "env HORIZON_BROWSER_ACTOR='horizon:panel-1' HORIZON_BROWSER_HOST_INSTANCE='host-instance' \
            HORIZON_BROWSER_ROOT='/srv/browser state'"
        ),
        "a root with a space stays one word: {named}"
    );
}

#[test]
fn the_probe_reads_the_record_of_the_remote_root_and_no_profile_folder() {
    assert_eq!(root_expression(None), "\"$HOME/.horizon\"");
    assert_eq!(
        probe_command(None),
        "root=\"$HOME/.horizon\"; printf '%s\\n' \"$root\"; \
         tr -d '\\n' < \"$root/profile-host.json\" || exit 3; printf '\\n'"
    );
    assert_eq!(
        probe_command(Some(Path::new("/srv/browser state"))),
        "root='/srv/browser state'; printf '%s\\n' \"$root\"; \
         tr -d '\\n' < \"$root/profile-host.json\" || exit 3; printf '\\n'"
    );
    assert!(
        !probe_command(None).contains("browser-profiles"),
        "the bridge takes no profile list from the folders of the remote machine"
    );
}

#[test]
fn a_remote_path_with_a_quote_can_not_leave_its_word() {
    assert_eq!(quote("/tmp/a'b"), "'/tmp/a'\\''b'");
    assert_eq!(read_file_command(Path::new("/tmp/a'b")), "cat '/tmp/a'\\''b'");
    assert_eq!(remove_directory_command("/tmp/x"), "rm -rf -- '/tmp/x'");
    assert_eq!(
        create_directory_command("/tmp/staging", "/tmp/staging/x"),
        "set -eu; umask 077; mkdir -p -- '/tmp/staging' && mkdir -- '/tmp/staging/x'"
    );
}

#[test]
fn a_staged_file_arrives_under_its_own_name_inside_the_call_folder() {
    let command = stage_file_command("/srv/state/browser-staging/call-1", 2, "a b.pdf");
    assert_eq!(
        command,
        "set -eu; mkdir -p -- '/srv/state/browser-staging/call-1/2' && \
         cat > '/srv/state/browser-staging/call-1/2/a b.pdf'.part && \
         mv -- '/srv/state/browser-staging/call-1/2/a b.pdf'.part '/srv/state/browser-staging/call-1/2/a b.pdf'"
    );
    assert_eq!(
        stage_file_command("/srv/state/browser-staging/call-1", 0, "a;rm -rf /"),
        "set -eu; mkdir -p -- '/srv/state/browser-staging/call-1/0' && \
         cat > '/srv/state/browser-staging/call-1/0/a;rm -rf /'.part && \
         mv -- '/srv/state/browser-staging/call-1/0/a;rm -rf /'.part '/srv/state/browser-staging/call-1/0/a;rm -rf /'",
        "a file name stays one quoted word"
    );
}

#[test]
fn a_staging_path_under_a_root_with_a_space_stays_one_word() {
    let directory = "/srv/browser state/browser-staging/call-1";
    assert_eq!(
        root_expression(Some(Path::new("/srv/browser state"))),
        "'/srv/browser state'"
    );
    assert_eq!(
        read_file_command(Path::new("/srv/browser state/captures/record.webm")),
        "cat '/srv/browser state/captures/record.webm'"
    );
    assert_eq!(
        stage_file_command(directory, 0, "report.pdf"),
        "set -eu; mkdir -p -- '/srv/browser state/browser-staging/call-1/0' && \
         cat > '/srv/browser state/browser-staging/call-1/0/report.pdf'.part && \
         mv -- '/srv/browser state/browser-staging/call-1/0/report.pdf'.part \
         '/srv/browser state/browser-staging/call-1/0/report.pdf'"
    );
    assert_eq!(
        remove_directory_command(directory),
        "rm -rf -- '/srv/browser state/browser-staging/call-1'"
    );
}

#[test]
fn a_staging_path_under_a_root_with_a_quote_stays_one_word() {
    let directory = "/srv/peter's browser/browser-staging/call-1";
    assert_eq!(
        create_directory_command("/srv/peter's browser/browser-staging", directory),
        "set -eu; umask 077; mkdir -p -- '/srv/peter'\\''s browser/browser-staging' && \
         mkdir -- '/srv/peter'\\''s browser/browser-staging/call-1'"
    );
    assert_eq!(
        stage_file_command(directory, 1, "report.pdf"),
        "set -eu; mkdir -p -- '/srv/peter'\\''s browser/browser-staging/call-1/1' && \
         cat > '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'.part && \
         mv -- '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'.part \
         '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'"
    );
    assert_eq!(
        remove_directory_command(directory),
        "rm -rf -- '/srv/peter'\\''s browser/browser-staging/call-1'"
    );
}

#[test]
fn a_staging_path_that_begins_with_a_dash_is_not_an_option() {
    let directory = "-staging/call-1";
    assert_eq!(root_expression(Some(Path::new("-root"))), "'-root'");
    assert_eq!(
        create_directory_command("-staging", directory),
        "set -eu; umask 077; mkdir -p -- '-staging' && mkdir -- '-staging/call-1'"
    );
    assert_eq!(
        stage_file_command(directory, 0, "report.pdf"),
        "set -eu; mkdir -p -- '-staging/call-1/0' && \
         cat > '-staging/call-1/0/report.pdf'.part && \
         mv -- '-staging/call-1/0/report.pdf'.part '-staging/call-1/0/report.pdf'"
    );
    assert_eq!(remove_directory_command(directory), "rm -rf -- '-staging/call-1'");
    assert_eq!(remove_directory_command("-rf"), "rm -rf -- '-rf'");
}
