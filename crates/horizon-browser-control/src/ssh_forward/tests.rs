//! A stand-in `ssh` program, a loopback port and a profile folder, rather than a
//! machine that runs a browser.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::remote_browser::{REMOTE_BROWSER_INSTALLATIONS, REMOTE_BROWSER_NAMES};
use super::transport::{failure_reason, quote, ssh_command};
use super::{
    Ssh, SshForward, SshForwardError, accepts_connection, forward_command, remote_browser_command, remote_port_command,
    run_remote_with, valid_profile,
};

const DEADLINE: Duration = Duration::from_secs(5);

/// One test at a time. A child that another thread of this process forked holds
/// the open script of this test, and Linux then refuses to execute it
/// (`ETXTBSY`), so no test writes a script while another one spawns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One `sh` program that stands in for `ssh`, so no test opens a connection.
fn stand_in(directory: &Path, body: &str) -> String {
    let path = directory.join("ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the stand-in ssh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("make it executable");
    path.to_string_lossy().into_owned()
}

/// Whether the kernel still knows `pid`. `kill -0` answers for a zombie too, so
/// only a reaped child makes this false.
fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The pid that the stand-in wrote, once it wrote it: the forward is ready
/// before the program behind it reached its first line.
fn read_pid(path: &Path) -> u32 {
    // The stand-in creates the file before it writes the pid, so an existing
    // file can still be empty.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if let Ok(pid) = text.trim().parse() {
            return pid;
        }
        assert!(Instant::now() < deadline, "the stand-in wrote no pid: {text:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn a_target_that_ssh_reads_as_an_option_is_refused_before_a_connection() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(directory.path(), "exit 1");
    let Err(error) = SshForward::start_with("-oProxyCommand=evil", 9222, &program, DEADLINE, accepts_connection) else {
        panic!("a target that ssh reads as an option was accepted");
    };
    assert!(
        matches!(&error, SshForwardError::InvalidTarget { target } if target == "-oProxyCommand=evil"),
        "{error}"
    );
    let refused = SshForward::start_with("browser-vm", 0, &program, DEADLINE, accepts_connection)
        .expect_err("a port that no forward can take");
    assert!(matches!(refused, SshForwardError::InvalidPort { port: 0 }), "{refused}");
}

#[test]
fn an_ssh_that_refuses_the_host_key_keeps_its_own_words() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(
        directory.path(),
        "printf '%s\\n' 'Host key verification failed.' >&2\nexit 255",
    );
    let Err(error) = SshForward::start_with("browser-vm", 9222, &program, DEADLINE, accepts_connection) else {
        panic!("an ssh that ended was accepted as a forward");
    };
    assert!(
        matches!(&error, SshForwardError::ForwardEnded { target, .. } if target == "browser-vm"),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("Host key verification failed."), "{message}");
    assert!(message.contains("127.0.0.1:"), "the refused port is missing: {message}");
}

#[test]
fn a_forward_that_never_takes_a_connection_ends_on_its_deadline() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(directory.path(), "exec sleep 60");
    let started = Instant::now();
    let Err(error) = SshForward::start_with(
        "browser-vm",
        9222,
        &program,
        Duration::from_millis(200),
        accepts_connection,
    ) else {
        panic!("a forward that took no connection was accepted");
    };
    let elapsed = started.elapsed();
    assert!(
        matches!(&error, SshForwardError::NotListening { target, .. } if target == "browser-vm"),
        "{error}"
    );
    assert!(elapsed < DEADLINE, "the deadline of the forward took {elapsed:?}");
    assert!(error.to_string().contains("ssh wrote no reason"), "{error}");
}

#[test]
fn a_ready_forward_is_reaped_with_its_child_when_it_drops() {
    let _serial = serial();
    let directory = scratch();
    let pid_file = directory.path().join("ssh.pid");
    let program = stand_in(
        directory.path(),
        &format!("printf '%s' \"$$\" > {}\nexec sleep 60", quoted(&pid_file)),
    );
    let mut probed = Vec::new();
    let forward = SshForward::start_with("browser-vm", 9222, &program, DEADLINE, |port| {
        probed.push(port);
        true
    })
    .expect("a forward that takes a connection");

    assert_eq!(probed.first().copied(), Some(forward.local_port()));
    assert_ne!(forward.local_port(), 0);
    let pid = read_pid(&pid_file);
    assert!(process_exists(pid), "the stand-in ssh is up");

    drop(forward);
    assert!(!process_exists(pid), "Drop left the child behind");
}

#[test]
fn a_local_port_that_another_process_took_is_tried_once_more() {
    let _serial = serial();
    let directory = scratch();
    let first = directory.path().join("first");
    let program = stand_in(
        directory.path(),
        &format!(
            "if [ -e {marker} ]; then\n  exec sleep 60\nfi\n: > {marker}\n\
             printf '%s\\n' 'bind [127.0.0.1]:41000: Address already in use' >&2\nexit 255",
            marker = quoted(&first)
        ),
    );
    let mut seen: Vec<u16> = Vec::new();
    let forward = SshForward::start_with("browser-vm", 9222, &program, DEADLINE, |port| {
        seen.push(port);
        seen.iter().any(|other| *other != port)
    })
    .expect("the second forward");

    assert!(first.exists(), "ssh ran only once");
    assert_ne!(
        seen.first(),
        seen.last(),
        "the second forward keeps the port of the first: {seen:?}"
    );
    assert_eq!(Some(forward.local_port()), seen.last().copied());
}

#[test]
fn the_forward_names_one_loopback_forward_and_exits_on_a_failed_bind() {
    let _serial = serial();
    let command = forward_command("ssh", "browser-vm", 41000, 9222);
    let args: Vec<String> = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|argument| argument == "127.0.0.1:41000:127.0.0.1:9222"),
        "no loopback forward: {args:?}"
    );
    for bounded in [
        "-N",
        "BatchMode=yes",
        "ConnectTimeout=5",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=2",
        "ExitOnForwardFailure=yes",
        "LogLevel=ERROR",
    ] {
        assert!(
            args.iter().any(|argument| argument == bounded),
            "{bounded} is missing: {args:?}"
        );
    }
    assert!(
        !args.iter().any(|argument| argument == "-q"),
        "the quiet flag drops the fatal line of ssh: {args:?}"
    );
    assert_eq!(args.last().map(String::as_str), Some("browser-vm"));
}

/// B13: the two `ssh` children of this crate leave multiplexing to the person, as
/// a panel does. The link of a server and the endpoint reads take the shared
/// transport, and the forward of a browser takes its own option list; neither may
/// name `ControlMaster`, `ControlPath`, `ControlPersist` or `-S`, because that
/// decides for all of the person's sessions. A person who shares one connection
/// over a link of a third of a second therefore gets that sharing here too.
#[test]
fn the_children_of_the_browser_control_take_no_multiplexing_decision() {
    let _serial = serial();
    let link = ssh_command("ssh", "server-vm", "/opt/horizon --server-link");
    let forward = forward_command("ssh", "browser-vm", 41000, 9222);
    for (kind, command) in [("the link of a server", link), ("the forward of a browser", forward)] {
        let args: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        for argument in &args {
            let lower = argument.to_ascii_lowercase();
            assert!(
                !lower.contains("controlmaster") && !lower.contains("controlpath") && !lower.contains("controlpersist"),
                "{kind} names no multiplexing option of its own: {args:?}"
            );
        }
        assert!(
            !args.iter().any(|argument| argument == "-S"),
            "{kind} passes no -S master path: {args:?}"
        );
    }
}

#[test]
fn the_browser_command_starts_the_profile_of_that_machine_after_a_stale_endpoint() {
    let _serial = serial();
    let headless = remote_browser_command("mail", true);
    let headed = remote_browser_command("mail", false);
    for expected in [
        r#"profile_dir="$HOME/.horizon/browser-profiles/"'mail'"#,
        r#"active="$profile_dir/DevToolsActivePort""#,
        r#"rm -f "$active""#,
        r"umask 077",
        r#"mkdir -p "$profile_dir" || exit 4"#,
        r"exit 3",
        r"exit 5",
        r#"nohup "$binary" --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 --user-data-dir="$profile_dir""#,
        r#"--no-first-run --no-default-browser-check --disable-features=TranslateUI about:blank >>"$profile_dir/chromium.log" 2>&1 </dev/null &"#,
        "cat \"$active\"",
    ] {
        assert!(headless.contains(expected), "{expected} is missing:\n{headless}");
    }
    assert!(headless.contains("--headless=new"), "{headless}");
    assert!(!headless.contains("--no-startup-window"), "{headless}");
    assert!(headed.contains("--no-startup-window"), "{headed}");
    assert!(!headed.contains("--headless=new"), "{headed}");
    assert!(
        !headless.contains(r#"if [ -s "$active" ]; then"#),
        "the start snippet must replace a stale endpoint instead of printing it:\n{headless}"
    );
    for name in REMOTE_BROWSER_NAMES {
        assert!(
            headless.contains(name),
            "{name} is missing from the search:\n{headless}"
        );
    }
    for installation in REMOTE_BROWSER_INSTALLATIONS {
        assert!(
            headless.contains(installation),
            "{installation} is missing from the search:\n{headless}"
        );
    }
}

#[test]
fn the_port_command_reads_the_endpoint_of_that_profile_only() {
    let _serial = serial();
    assert_eq!(
        remote_port_command("mail"),
        concat!(
            r#"if [ -s "$HOME/.horizon/browser-profiles/"'mail'/DevToolsActivePort ]; then "#,
            r#"cat "$HOME/.horizon/browser-profiles/"'mail'/DevToolsActivePort; fi"#
        ),
        "a profile folder without an endpoint prints nothing instead of failing"
    );
}

#[test]
fn both_snippets_parse_under_sh() {
    let _serial = serial();
    let directory = scratch();
    let snippets = [
        remote_browser_command("mail", true),
        remote_browser_command("mail", false),
        remote_port_command("mail"),
        remote_port_command("a'b"),
    ];
    for (index, snippet) in snippets.iter().enumerate() {
        let path: PathBuf = directory.path().join(format!("snippet-{index}.sh"));
        std::fs::write(&path, snippet).expect("write the snippet");
        let status = Command::new("sh").arg("-n").arg(&path).status().expect("run sh -n");
        assert!(status.success(), "snippet {index} does not parse:\n{snippet}");
    }
}

#[test]
fn a_profile_of_that_machine_is_one_folder_name_and_nothing_else() {
    let _serial = serial();
    for accepted in ["mail", "work-1", "a"] {
        assert!(valid_profile(accepted), "{accepted}");
    }
    for refused in ["", "Mail", "../mail", "mail/root", "-mail", "mail_box", "mail\n"] {
        assert!(!valid_profile(refused), "{refused:?}");
    }
    let command = remote_port_command("a'b");
    assert!(
        command.contains(r"'a'\''b'"),
        "a quote in the profile breaks the shell word: {command}"
    );
}

#[test]
fn the_settings_of_a_connect_carry_the_program_and_the_bounds_of_a_test() {
    let _serial = serial();
    let directory = scratch();
    let pid_file = directory.path().join("ssh.pid");
    let program = stand_in(
        directory.path(),
        &format!(
            "case \" $* \" in\n  *\" -N \"*) printf '%s' \"$$\" > {}; exec sleep 60 ;;\n  *) printf '%s\\n' '41000' '/devtools/browser/6a0c1f' ;;\nesac",
            quoted(&pid_file)
        ),
    );
    let ssh = Ssh::default().with_program(&program).with_probe(|_port| true);
    let answer = ssh
        .run_remote("browser-vm", "cat \"$HOME/x\"", DEADLINE)
        .expect("the answer of the remote command");
    assert_eq!(answer, "41000\n/devtools/browser/6a0c1f\n");

    let forward = ssh
        .start_forward("browser-vm", 9222)
        .expect("a forward of the stand-in");
    assert_ne!(forward.local_port(), 0);
    let pid = read_pid(&pid_file);
    assert!(process_exists(pid), "the stand-in ssh is up");
    drop(forward);
    assert!(!process_exists(pid), "Drop left the child behind");
}

#[test]
fn a_command_on_the_remote_machine_gives_its_standard_output() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(directory.path(), "printf '%s\\n' '41000' '/devtools/browser/6a0c1f'");
    let answer = run_remote_with("browser-vm", "cat \"$HOME/x\"", &program, 1024, DEADLINE)
        .expect("the answer of the remote command");
    assert_eq!(answer, "41000\n/devtools/browser/6a0c1f\n");
}

#[test]
fn a_command_on_the_remote_machine_that_fails_names_its_own_words() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(
        directory.path(),
        "printf '%s\\n' 'cat: cannot open DevToolsActivePort' >&2\nexit 1",
    );
    let error = run_remote_with("browser-vm", "cat nope", &program, 1024, DEADLINE).expect_err("a command that failed");
    assert!(
        matches!(&error, SshForwardError::RemoteCommandFailed { target, .. } if target == "browser-vm"),
        "{error}"
    );
    assert!(error.to_string().contains("cannot open DevToolsActivePort"), "{error}");
}

#[test]
fn a_command_on_the_remote_machine_that_never_answers_ends_on_its_deadline() {
    let _serial = serial();
    let directory = scratch();
    let program = stand_in(directory.path(), "exec sleep 60");
    let error = run_remote_with("browser-vm", "cat nope", &program, 1024, Duration::from_millis(200))
        .expect_err("a command that never answered");
    assert!(
        matches!(&error, SshForwardError::RemoteNoAnswer { target, .. } if target == "browser-vm"),
        "{error}"
    );
}

#[test]
fn every_ssh_of_a_connect_is_bounded_and_can_not_ask_a_question() {
    let command = ssh_command("ssh", "browser-vm", "cat \"$HOME/x\"");
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
        args.first().is_some_and(|first| first == "-T"),
        "no controlling terminal: {args:?}"
    );
    assert!(
        args.iter().any(|argument| argument == "LogLevel=ERROR"),
        "a fatal line of ssh reaches the reason: {args:?}"
    );
    assert!(
        !args.iter().any(|argument| argument == "-q" || argument == "-Tq"),
        "the quiet flag drops the fatal line of ssh and leaves an empty reason: {args:?}"
    );
    assert_eq!(
        args.iter().rev().take(2).rev().collect::<Vec<_>>(),
        ["browser-vm", "cat \"$HOME/x\""],
        "the target and the remote command stay the last two words: {args:?}"
    );
}

#[test]
fn a_failed_call_that_wrote_nothing_carries_its_exit_status() {
    assert_eq!(
        failure_reason(Some(255), ""),
        "ssh exited with status 255 and wrote no reason"
    );
    assert_eq!(failure_reason(None, ""), "ssh ended on a signal and wrote no reason");
    assert_eq!(
        failure_reason(Some(255), "Host key verification failed."),
        "Host key verification failed."
    );
}

#[test]
fn a_value_of_that_machine_stays_one_word_of_the_remote_shell() {
    assert_eq!(quote("/srv/browser state"), "'/srv/browser state'");
    assert_eq!(quote("/tmp/a'b"), "'/tmp/a'\\''b'");
    assert_eq!(quote("a;rm -rf /"), "'a;rm -rf /'");
}
