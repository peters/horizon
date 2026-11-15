//! The state report of a pi panel, run as the real extension file under Node.
//!
//! Horizon gives a pi panel `-e <report>` and the five `HORIZON_WORK_*`
//! variables of that one process. The extension must send each lifecycle event of
//! pi as one detached call of `<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, with
//! one small document on standard input, and it must do nothing else: no text of
//! the person or of the model, no wait, no failure.
//!
//! The test runs the module that the build embeds, through the harness that
//! builds the `pi` object of an extension, and records every call with a small
//! shell program in place of the entry. Node is the one program that a test of
//! this file needs; without it the test says that it skipped.
#![cfg(unix)]
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

const HARNESS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/pi_extension_harness.mjs"
);
/// The report exactly as the build embeds it, so this test reads the file that
/// an installed Horizon writes into its home.
const REPORT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/plugins/pi/horizon-report.mjs"
));
/// The text that the harness fires as the message of the person and as the title
/// of the dialog. No call may carry it.
const SECRET: &str = "not sent";
fn node() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("node"))
        .find(|program| program.is_file())
}

/// The fixture of one run: the report on disk, the directory of the calls, and a
/// recorder in place of the entry of Horizon.
struct Fixture {
    root: tempfile::TempDir,
    /// The program in place of the entry of Horizon.
    recorder: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("fixture root");
        let report = root.path().join("pi-report.mjs");
        std::fs::write(&report, REPORT).expect("the report file");
        std::fs::create_dir(root.path().join("calls")).expect("call directory");
        std::fs::create_dir(root.path().join("sessions")).expect("session directory");
        // One file for each call, so two reports of one event cannot overwrite
        // each other. The program writes its file at once and then holds, which
        // is how the test sees that a report waits for nothing.
        let recorder = horizon_core::stand_in_program_in(
            root.path(),
            "recorder",
            concat!(
                "#!/bin/sh\n",
                "file=\"$HORIZON_TEST_RECORD/call-$$\"\n",
                "{\n  printf 'ARGV %s\\n' \"$*\"\n  printf 'STDIN '\n  cat\n} > \"$file\"\n",
                "sleep 2\n"
            ),
        )
        .expect("recorder program");
        Self { root, recorder }
    }

    fn report(&self) -> PathBuf {
        self.root.path().join("pi-report.mjs")
    }

    fn recorder(&self) -> PathBuf {
        self.recorder.clone()
    }

    fn calls_directory(&self) -> PathBuf {
        self.root.path().join("calls")
    }

    /// The harness of the report, with the identity of one launch.
    fn harness(&self, executable: &Path) -> Harness {
        let mut command = Command::new(node().expect("node is on the PATH of this test"));
        command
            .arg(HARNESS)
            .arg("--extension")
            .arg(self.report())
            .arg("--session-dir")
            .arg(self.root.path().join("sessions"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.path())
            .env("HORIZON_TEST_RECORD", self.calls_directory())
            .env("HORIZON_WORK_ROOT", self.root.path())
            .env("HORIZON_WORK_PANEL", "pi-panel")
            .env("HORIZON_WORK_OWNER", "launch-owner")
            .env("HORIZON_WORK_KIND", "pi")
            .env("HORIZON_WORK_EXECUTABLE", executable)
            .current_dir(self.root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("the harness starts");
        let answers = BufReader::new(child.stdout.take().expect("stdout of the harness"));
        let input = child.stdin.take().expect("stdin of the harness");
        Harness { child, answers, input }
    }

    /// The calls that the entry received, in the order that the report sent them.
    fn calls(&self) -> Vec<Call> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(self.calls_directory())
            .expect("call directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        paths.sort_by_key(|path| {
            (
                std::fs::metadata(path).and_then(|meta| meta.modified()).ok(),
                path.clone(),
            )
        });
        paths
            .iter()
            .map(|path| Call::read(path))
            .collect::<Vec<Call>>()
            .into_iter()
            .collect()
    }

    /// Waits for `count` calls, or fails with what arrived instead.
    ///
    /// The recorder creates the file of a call before it writes the document
    /// into it, so a file that is there with no document yet is a call that the
    /// recorder still writes. The wait is for the documents, and its bound is the
    /// hang guard of the test.
    fn wait_for_calls(&self, count: usize) -> Vec<Call> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let calls = self.calls();
            let written = calls.iter().all(|call| call.document.is_ok());
            if (calls.len() >= count && written) || Instant::now() >= deadline {
                return calls;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// One call of the entry: its arguments and its document.
#[derive(Clone)]
struct Call {
    argv: String,
    /// The document as read from standard input, or the reason it does not read.
    document: Result<serde_json::Value, String>,
    raw: String,
}

impl Call {
    fn read(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut argv = String::new();
        let mut body = String::new();
        let mut reading_argv = true;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("ARGV ") {
                argv.push_str(rest);
            } else if let Some(rest) = line.strip_prefix("STDIN ") {
                body.push_str(rest);
                reading_argv = false;
            } else if !reading_argv {
                body.push_str(line);
            }
        }
        let document = serde_json::from_str(&body).map_err(|error| error.to_string());
        Self {
            argv,
            document,
            raw: text,
        }
    }

    fn event(&self) -> String {
        self.document
            .as_ref()
            .ok()
            .and_then(|document| document["hook_event_name"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// The harness of the report, with the words of the person.
struct Harness {
    child: Child,
    answers: BufReader<std::process::ChildStdout>,
    input: ChildStdin,
}

impl Harness {
    fn line(&mut self) -> String {
        let mut line = String::new();
        self.answers.read_line(&mut line).expect("answer of the harness");
        line.trim_end().to_string()
    }

    /// Fires one word, and answers how long the harness took to answer it.
    fn fire(&mut self, word: &str) -> (String, Duration) {
        let started = Instant::now();
        writeln!(self.input, "{word}").expect("the word reaches the harness");
        self.input.flush().expect("flush the word");
        let answer = self.line();
        (answer, started.elapsed())
    }

    fn end(mut self) {
        self.fire("quit");
        drop(self.input);
        let finished = self.child.wait().expect("the harness ends");
        assert!(finished.success(), "the harness ends after quit: {finished:?}");
    }
}

/// The words of one turn, fired into the report, with the answer of each.
fn fire_turn(harness: &mut Harness) -> Vec<String> {
    let mut answers = Vec::new();
    for word in ["start", "ask", "answer", "stop", "start", "esc"] {
        let (answer, took) = harness.fire(word);
        assert!(
            answer.starts_with("harness:") && answer.ends_with("fired"),
            "{word}: {answer}"
        );
        assert!(
            took < Duration::from_secs(1),
            "pi never waits for a report, and the recorder holds for two seconds: {word} took {took:?}"
        );
        answers.push(answer);
    }
    answers
}

/// One call of the entry, with a document that reads.
fn call_document(call: &Call) -> &serde_json::Value {
    call.document
        .as_ref()
        .unwrap_or_else(|error| panic!("the document of {} does not read: {error}", call.argv))
}

/// The turn identity of one call, or the empty text for an event without one.
fn call_turn(call: &Call) -> String {
    call_document(call)["prompt_id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The dialog identity of one call that names one.
fn call_span(call: &Call) -> String {
    call_document(call)["tool_use_id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The first call of `event`, or the reason that the report sent none.
fn call_of<'a>(calls: &'a [Call], event: &str) -> &'a Call {
    calls
        .iter()
        .find(|call| call.event() == event)
        .unwrap_or_else(|| panic!("the report sent no {event}"))
}

#[test]
fn the_report_starts_the_entry_once_for_each_event_of_a_turn() {
    let Some(_) = node() else {
        eprintln!("skipped: this computer has no node on PATH, and the report of a pi panel runs under Node");
        return;
    };
    let fixture = Fixture::new();
    let mut harness = fixture.harness(&fixture.recorder());
    let banner = harness.line();
    assert!(
        banner.contains("8 events of the report are subscribed"),
        "the report subscribes the events of a pi turn: {banner}"
    );
    let answers = fire_turn(&mut harness);
    let calls = fixture.wait_for_calls(6);
    // The report of a pi panel starts each entry as a detached process, so the
    // files of two calls of one run can arrive in either order. The test reads
    // the events and their turn identities, not the order of the files.
    let mut events: Vec<String> = calls.iter().map(Call::event).collect();
    events.sort();
    assert_eq!(
        events,
        vec![
            "Interrupt",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "UserPromptSubmit",
            "UserPromptSubmit",
        ],
        "the report sends one event for each step of the turn: {answers:?}"
    );
    for call in &calls {
        assert_eq!(
            call.argv, "--agent-work-hook",
            "the entry receives its own argument and nothing else: {}",
            call.argv
        );
    }
    // The turn id of a closing event is the id of the turn that opened it, so a
    // late or a repeated event cannot close another turn of this launch.
    let prompts: Vec<String> = calls.iter().map(call_turn).collect();
    let opened: Vec<String> = calls
        .iter()
        .filter(|call| call.event() == "UserPromptSubmit")
        .map(call_turn)
        .collect();
    assert_eq!(opened.len(), 2, "each busy span opens one turn");
    assert_ne!(opened[0], opened[1], "every busy span opens a turn of its own");
    assert!(
        prompts.contains(&opened[0]) && prompts.contains(&opened[1]),
        "{prompts:?}"
    );
    let stop = call_of(&calls, "Stop");
    let interrupt = call_of(&calls, "Interrupt");
    assert!(opened.contains(&call_turn(stop)), "the stop closes a turn that opened");
    assert!(
        opened.contains(&call_turn(interrupt)),
        "the interrupt closes a turn that opened"
    );
    assert_ne!(
        call_turn(stop),
        call_turn(interrupt),
        "a stop and an interrupt close different turns"
    );
    let ask = call_of(&calls, "PermissionRequest");
    let answer = call_of(&calls, "PostToolUse");
    assert_eq!(
        call_turn(ask),
        call_turn(stop),
        "the dialog belongs to the turn of the stop"
    );
    assert_eq!(
        call_span(ask),
        call_span(answer),
        "the answer closes the dialog of its call"
    );
    assert!(
        call_span(ask).starts_with("ui-prompt-"),
        "the dialog names itself: {}",
        call_span(ask)
    );
    harness.end();
    let calls = fixture.wait_for_calls(7);
    let last = calls.last().expect("the call of the session end");
    assert_eq!(last.event(), "SessionEnd", "the end of the session is recorded");
    assert_eq!(
        call_document(last)["reason"].as_str(),
        Some("quit"),
        "the session ended because pi quit"
    );
    assert!(
        call_document(last)["prompt_id"].is_null(),
        "a session end names no turn"
    );
}

#[test]
fn the_documents_of_the_report_carry_no_text_and_name_their_launch() {
    let Some(_) = node() else {
        eprintln!("skipped: this computer has no node on PATH, and the report of a pi panel runs under Node");
        return;
    };
    let fixture = Fixture::new();
    let mut harness = fixture.harness(&fixture.recorder());
    let banner = harness.line();
    assert!(banner.contains("subscribed"), "{banner}");
    fire_turn(&mut harness);
    let calls = fixture.wait_for_calls(6);
    for call in &calls {
        let document = call_document(call);
        for field in ["session_id", "cwd", "transcript_path"] {
            assert!(
                document[field].as_str().is_some_and(|value| !value.is_empty()),
                "{field} is in the document: {document}"
            );
        }
        let cwd = document["cwd"].as_str().unwrap_or_default();
        let transcript = document["transcript_path"].as_str().unwrap_or_default();
        assert!(cwd.starts_with('/'), "the directory of the panel is absolute: {cwd}");
        assert!(
            transcript.starts_with('/'),
            "the file of the conversation is absolute: {transcript}"
        );
        assert!(
            !call.raw.contains(SECRET),
            "no text of the person and no title reaches the entry: {}",
            call.raw
        );
        assert!(
            document.get("prompt").is_none() && document.get("message").is_none(),
            "the document holds no prompt and no message: {document}"
        );
    }
    harness.end();
}

#[test]
fn a_launch_without_the_variables_reports_nothing() {
    let Some(_) = node() else {
        eprintln!("skipped: this computer has no node on PATH, and the report of a pi panel runs under Node");
        return;
    };
    let fixture = Fixture::new();
    // The execution of a terminal panel in which a person starts pi by hand has
    // no identity of a launch, and the report does nothing at all.
    let mut command = Command::new(node().expect("node"));
    command
        .arg(HARNESS)
        .arg("--extension")
        .arg(fixture.report())
        .arg("--session-dir")
        .arg(fixture.root.path().join("sessions"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", fixture.root.path())
        .env("HORIZON_TEST_RECORD", fixture.calls_directory())
        .env("HORIZON_WORK_EXECUTABLE", fixture.recorder())
        .current_dir(fixture.root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn().expect("the harness starts");
    let mut answers = BufReader::new(child.stdout.take().expect("stdout"));
    let mut input = child.stdin.take().expect("stdin");
    let mut banner = String::new();
    answers.read_line(&mut banner).expect("banner");
    assert!(
        banner.contains("0 events of the report are subscribed"),
        "the report subscribes nothing without its variables: {banner}"
    );
    for word in ["start", "stop"] {
        writeln!(input, "{word}").expect("the word reaches the harness");
        input.flush().expect("flush the word");
        let mut answer = String::new();
        answers.read_line(&mut answer).expect("answer");
        assert!(
            answer.contains("fired"),
            "the harness fires the word with no report: {answer}"
        );
    }
    drop(input);
    let finished = child.wait().expect("the harness ends");
    assert!(finished.success(), "the harness ends without a report: {finished:?}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.calls().is_empty(),
        "a launch without the variables calls no entry"
    );
}

#[test]
fn a_report_whose_entry_is_gone_still_lets_pi_work() {
    let Some(_) = node() else {
        eprintln!("skipped: this computer has no node on PATH, and the report of a pi panel runs under Node");
        return;
    };
    let fixture = Fixture::new();
    // The report names an entry that does not exist, which is the shape of a
    // Horizon that was replaced or removed while the panel runs.
    let mut harness = fixture.harness(&fixture.root.path().join("no-such-horizon"));
    let banner = harness.line();
    assert!(banner.contains("subscribed"), "{banner}");
    for word in ["start", "ask", "stop", "quit"] {
        let (answer, took) = harness.fire(word);
        assert!(
            answer.ends_with("fired"),
            "the report swallows the failed start of its entry: {word}: {answer}"
        );
        assert!(took < Duration::from_secs(1), "{word} took {took:?}");
    }
    drop(harness.input);
    let finished = harness.child.wait().expect("the harness ends");
    assert!(
        finished.success(),
        "pi is never stopped by the report of a panel: {finished:?}"
    );
    assert!(fixture.calls().is_empty(), "a missing entry records nothing");
}
