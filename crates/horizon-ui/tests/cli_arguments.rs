//! An argument of this build is judged before the start touches the home, the
//! settings, the sessions or the skills of the person.
//!
//! This is the only test of the workspace that starts the binary of the UI
//! crate, and it gives every start its own home in a temporary folder and no
//! display. A start that reaches the window would hang, so the helper stops the
//! exact child after ten seconds and the test fails.
#![forbid(unsafe_code)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);

/// The first line of the usage, which `--help` writes and every refusal repeats.
const USAGE_FIRST_LINE: &str = "usage: horizon [--config <path>] [--ephemeral] [--new-session] [--blank]";

/// One start of the binary and what it left in its private home.
struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    entries_in_home: usize,
}

impl Outcome {
    /// The first line of the standard output, without the line end.
    fn first_stdout_line(&self) -> &str {
        self.stdout.lines().next().unwrap_or_default()
    }

    /// The first line of the standard error, without the line end.
    fn first_stderr_line(&self) -> &str {
        self.stderr.lines().next().unwrap_or_default()
    }
}

/// Starts the binary with `arguments`, a private home and no display.
fn start(arguments: &[&str]) -> Outcome {
    let home = tempfile::tempdir().expect("the private home of the start");
    let output = tempfile::tempdir().expect("the folder of the output");
    let written = |name: &str| {
        std::fs::File::create(output.path().join(name)).unwrap_or_else(|error| panic!("create {name}: {error}"))
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon"))
        .args(arguments)
        .env_clear()
        .env("HOME", home.path())
        .env("HORIZON_AGENT_INTEGRATIONS", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::from(written("stdout")))
        .stderr(Stdio::from(written("stderr")))
        .spawn()
        .expect("start the binary of the UI crate");
    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = child.try_wait().expect("ask the start") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`horizon {}` still ran after {BOUND:?}: it reached the window instead of judging the argument",
                arguments.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let read = |name: &str| {
        std::fs::read_to_string(output.path().join(name)).unwrap_or_else(|error| panic!("read {name}: {error}"))
    };
    Outcome {
        code: status.code().expect("the exit code of the start"),
        stdout: read("stdout"),
        stderr: read("stderr"),
        entries_in_home: std::fs::read_dir(home.path()).expect("read the private home").count(),
    }
}

/// The usage and the version end the start before the home is prepared.
#[test]
fn the_usage_and_the_version_leave_the_home_alone() {
    let usage = USAGE_FIRST_LINE;

    for arguments in [&["--help"][..], &["-h"]] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 0, "`{arguments:?}`: {:?}", outcome.stderr);
        assert_eq!(outcome.first_stdout_line(), usage, "`{arguments:?}`");
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }

    let expected = format!("horizon {}", env!("CARGO_PKG_VERSION"));
    for arguments in [&["--version"][..], &["-V"]] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 0, "`{arguments:?}`: {:?}", outcome.stderr);
        assert_eq!(outcome.first_stdout_line(), expected, "`{arguments:?}`");
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }
}

/// An argument that this build does not know names itself and ends the start,
/// which is what a start by mistake of a test runner or of `--help` must do.
#[test]
fn an_argument_that_this_build_does_not_know_ends_the_start() {
    for (arguments, named) in [
        (&["--list"][..], "--list"),
        (&["--nocapture"], "--nocapture"),
        (&["--definitely-unknown"], "--definitely-unknown"),
        (&["stray"], "stray"),
    ] {
        let outcome = start(arguments);
        assert_eq!(outcome.code, 2, "`{arguments:?}`: {:?}", outcome.stdout);
        assert_eq!(
            outcome.first_stderr_line(),
            format!("error: unknown argument {named}"),
            "`{arguments:?}`"
        );
        assert_eq!(outcome.entries_in_home, 0, "`{arguments:?}` prepared the home");
    }

    // A known mode refuses an argument that is not part of it, so an entry in a
    // client cannot fall through to another mode either.
    let browser_mcp = start(&["--browser-mcp", "--list"]);
    assert_eq!(browser_mcp.code, 2);
    assert_eq!(browser_mcp.entries_in_home, 0);
    let server = start(&["--server", "stray"]);
    assert_eq!(server.code, 2);
    assert_eq!(server.entries_in_home, 0);
}

/// The usage follows the error line of a refusal, which is the line that a
/// person reads in a terminal.
#[test]
fn a_refusal_writes_the_usage_below_its_error() {
    let outcome = start(&["--list"]);
    let lines: Vec<&str> = outcome.stderr.lines().collect();
    assert_eq!(lines.first().copied(), Some("error: unknown argument --list"));
    assert_eq!(lines.get(1).copied(), Some(USAGE_FIRST_LINE));
}
