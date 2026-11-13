//! The state machine of one take, with a recorder that is a script of a test.
//!
//! Every recorder here copies a prepared file to the path of the take and then
//! waits for its signal, so a test proves the bounds of a take without a
//! microphone and without a second of wall time: the clock of the take is the one
//! of the test.
//!
//! The rule of these tests: **the clock of a test never runs ahead of a real
//! program.** The worker reads [`TestClock`], whose time no wait moves, and a test
//! moves that time only after its recorder has shown that it runs — it wrote its own
//! pid and the file of the take holds its bytes. A bound of the code, such as the
//! 120 seconds of a take or the two seconds of a stop, is then reached by the test
//! and not by a machine that starts `/bin/sh` more slowly. A bound that the worker
//! computes from an instant of its own is reached in steps while the worker reads it
//! more than once.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::process::Pid;

use super::{
    MAX_TAKE, POLL, SpeechEventKind, SpeechInput, SpeechKeyStore, SpeechWorker, TAKE_FOLDER, TakeBounds, TakePlan,
    Transcriber,
};
use crate::named_secrets::SecretName;
use crate::panel::PanelId;
use crate::speech::clock::TestClock;
use crate::speech::command::CommandLine;
use crate::speech::endpoint::Endpoint;

const TARGET: PanelId = PanelId(7);

/// The most time that a test waits for an event of the worker.
const PATIENCE: Duration = Duration::from_secs(20);

/// A home of one test, its clock and its worker.
pub(super) struct Take {
    pub(super) home: tempfile::TempDir,
    pub(super) clock: Arc<TestClock>,
    pub(super) input: SpeechInput,
    children: Vec<Child>,
}

impl Take {
    /// A take of the bounds of the code, whose clock only the test moves.
    pub(super) fn new(keys: Arc<dyn SpeechKeyStore>) -> Self {
        Self::with_bounds(keys, TakeBounds::default())
    }

    /// A take of the bounds of `bounds`, whose clock only the test moves.
    pub(super) fn with_bounds(keys: Arc<dyn SpeechKeyStore>, bounds: TakeBounds) -> Self {
        let home = tempfile::tempdir().expect("a home of a test");
        let clock = Arc::new(TestClock::new());
        let worker = SpeechWorker::new(home.path().to_path_buf(), keys)
            .with_clock(Arc::clone(&clock) as Arc<dyn super::super::Clock>, bounds);
        let input = SpeechInput::start(worker);
        Self {
            home,
            clock,
            input,
            children: Vec::new(),
        }
    }

    pub(super) fn folder(&self) -> PathBuf {
        self.home.path().join(TAKE_FOLDER)
    }

    /// The command line of a `/bin/sh` script of this test, in the home of the test.
    ///
    /// The line is `/bin/sh '<path>'`, so the kernel never executes the script file:
    /// the shell reads it. A test thread that forks while another test writes a
    /// script cannot then make that script refuse its own start with `ETXTBSY`; the
    /// supervisor of a recorder runs the command line as its program.
    pub(super) fn script(&self, name: &str, body: &str) -> String {
        let body = body.strip_prefix("#!/bin/sh\n").unwrap_or(body);
        crate::util::stand_in_shell_in(self.home.path(), name, body.trim_end_matches('\n')).expect("a script of a test")
    }

    /// A file that a recorder copies, so a take holds the bytes of a recording.
    pub(super) fn prepared(&self) -> PathBuf {
        let path = self.home.path().join("prepared.wav");
        let mut bytes = b"RIFF$\x00\x00\x00WAVEfmt ".to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        fs::write(&path, bytes).expect("a recording of a test");
        path
    }

    /// A recording of `data` bytes whose canonical header holds the length that a
    /// killed recorder leaves: zero in both length fields.
    fn unclosed(&self, data: usize) -> PathBuf {
        let path = self.home.path().join("unclosed.wav");
        let mut bytes = super::wav::header_of(0, 0);
        bytes.resize(super::wav::CANONICAL_HEADER + data, 0);
        fs::write(&path, bytes).expect("a recording of a test");
        path
    }

    /// The path of the pid of one recorder.
    fn pid_file(&self, name: &str) -> PathBuf {
        self.home.path().join(format!("{name}.pid"))
    }

    /// A recorder that copies the prepared file, writes its own pid and waits for
    /// its signal until it ends.
    pub(super) fn recorder(&self, name: &str, prepared: &Path) -> CommandLine {
        let pid = self.pid_file(name);
        let program = self.script(
            name,
            &format!(
                "#!/bin/sh\ncp '{}' \"$1\"\nprintf '%s\\n' \"$$\" > '{}'\ntrap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done\n",
                prepared.display(),
                pid.display()
            ),
        );
        CommandLine::parse(&format!("{program} {{file}}")).expect("a recorder line")
    }

    /// A recorder of `name` that copies the prepared file, writes its own pid, then
    /// keeps running, and that entered this process with the signals of `ignored` set
    /// to "ignore".
    ///
    /// The ignore is set in a wrapper that [`exec`](std::os::unix::process::CommandExt::exec)s
    /// the script, so the test states the disposition of a background job of a
    /// non-interactive shell (`&`, `nohup`, a launcher) itself: a shell that inherits
    /// an ignored signal cannot trap it, so a script that relies on the disposition
    /// it inherits says nothing about the same test in the foreground. `trap` of the
    /// script then holds for the signals that the wrapper left alone.
    fn ignoring(&self, name: &str, ignored: &str, prepared: &Path, traps: &str) -> CommandLine {
        let pid = self.pid_file(name);
        let program = self.script(
            name,
            &format!(
                "#!/bin/sh\ncp '{}' \"$1\"\nprintf '%s\\n' \"$$\" > '{}'\n{traps}\nwhile :; do sleep 0.02; done\n",
                prepared.display(),
                pid.display()
            ),
        );
        CommandLine::parse(&format!(
            "sh -c 'trap \"\" {ignored}; exec \"$@\"' sh {program} {{file}}"
        ))
        .expect("a recorder line")
    }

    /// Wait until `ready` answers, and report what the worker said when it does
    /// not.
    pub(super) fn wait_for(&self, what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        let mut seen = Vec::new();
        while !ready() {
            while let Some(event) = self.input.poll() {
                seen.push(event);
            }
            assert!(
                Instant::now() < deadline,
                "the test waited for {what} in vain, and the worker said {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Wait until the recorder of `name` runs: it wrote its own pid and the file of
    /// the take holds the bytes that it copied. Only then may a test move the clock,
    /// because a bound of the take may not pass a program that has not started.
    pub(super) fn wait_for_recorder(&self, name: &str) {
        self.wait_for("the recorder", || self.pid_file(name).exists() && self.file_bytes() > 0);
    }

    fn events(&self) -> Vec<SpeechEventKind> {
        let mut kinds = Vec::new();
        while let Some(event) = self.input.poll() {
            assert_eq!(event.target, TARGET);
            kinds.push(event.kind);
        }
        kinds
    }

    /// One event of one of `wanted`, or a failure that names what the worker said.
    pub(super) fn wait_for_event(&self, wanted: &[&str]) -> Vec<SpeechEventKind> {
        self.wait_for_event_on(None, wanted)
    }

    /// One event of one of `wanted`, with the clock of the test moved by `step`
    /// before every look at the worker.
    ///
    /// A bound that the worker computes from an instant of its own, such as the two
    /// seconds of a stop, is reached in steps: the worker sets its deadline when the
    /// signal goes out, so a single step would move that deadline with it. `None`
    /// leaves the clock where the test put it.
    fn wait_for_event_on(&self, step: Option<Duration>, wanted: &[&str]) -> Vec<SpeechEventKind> {
        let deadline = Instant::now() + PATIENCE;
        let mut seen = Vec::new();
        loop {
            if let Some(step) = step {
                self.clock.advance(step);
            }
            for kind in self.events() {
                let named = match &kind {
                    SpeechEventKind::Listening => "listening",
                    SpeechEventKind::Writing => "writing",
                    SpeechEventKind::Text(_) => "text",
                    SpeechEventKind::Failed(_) => "failed",
                    SpeechEventKind::Kept(_) => "kept",
                    SpeechEventKind::Dropped => "dropped",
                };
                if wanted.contains(&named) {
                    seen.push(kind);
                    if seen.len() == wanted.len() {
                        return seen;
                    }
                } else if let SpeechEventKind::Failed(message) | SpeechEventKind::Kept(message) = &kind {
                    panic!("the worker says {message:?} and not {wanted:?}");
                }
            }
            assert!(
                Instant::now() < deadline,
                "the test waited for {wanted:?} and saw {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A child of this test that no signal of a take may reach.
    fn bystander(&mut self) -> u32 {
        let child = Command::new("sleep").arg("30").spawn().expect("a bystander");
        let id = child.id();
        self.children.push(child);
        id
    }

    /// The bytes of every file of the folder of the takes.
    fn file_bytes(&self) -> u64 {
        self.files()
            .iter()
            .map(|name| fs::metadata(self.folder().join(name)).map_or(0, |metadata| metadata.len()))
            .sum()
    }

    /// The process ids of the recorders that this test started, as the stand-ins wrote
    /// them.
    fn recorder_pids(&self) -> Vec<u32> {
        let Ok(entries) = fs::read_dir(self.home.path()) else {
            return Vec::new();
        };
        (entries.flatten())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".pid"))
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|text| text.trim().parse().ok())
            .collect()
    }

    pub(super) fn files(&self) -> Vec<String> {
        fs::read_dir(self.folder())
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Take {
    fn drop(&mut self) {
        // A test that failed or panicked before it stopped its recorder must not leave
        // one that records: the drop ends every recorder of this test by its own
        // process id. A recorder that already ended makes a signal reach nothing.
        for pid in self.recorder_pids() {
            if alive(pid)
                && let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw)
            {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(super) fn plan(recorder: CommandLine, transcriber: Transcriber) -> TakePlan {
    TakePlan { recorder, transcriber }
}

/// A command that records that it ran and answers one text.
pub(super) fn transcriber_script(take: &Take, answer: &str) -> (CommandLine, PathBuf) {
    let marker = take.home.path().join("transcribed.txt");
    let program = take.script(
        "transcriber",
        &format!(
            "#!/bin/sh\nprintf 'ran' > '{}'\nprintf '%s' '{answer}'\n",
            marker.display()
        ),
    );
    (
        CommandLine::parse(&format!("{program} {{file}}")).expect("a transcriber line"),
        marker,
    )
}

fn alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

/// The pid that a recorder of this test wrote, once the write of it is complete.
fn pid_of(take: &Take, name: &str) -> u32 {
    pid_of_file(&take.pid_file(name))
}

/// The pid that a stand-in recorder wrote into `file`, once the write of it is
/// complete.
fn pid_of_file(file: &Path) -> u32 {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Ok(text) = fs::read_to_string(file)
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the pid in {} never arrived", file.display());
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_short_take_is_dropped_and_runs_no_transcriber() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    // The clock of the test did not move, so the take is shorter than 0.3 seconds.
    take.input.end();
    take.wait_for_event(&["dropped"]);
    assert!(!marker.exists(), "a short take runs no transcriber");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_take_of_one_second_is_transcribed_by_the_command_of_the_settings() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, marker) = transcriber_script(&take, "  hei\n");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_millis(1000));
    take.input.end();
    let events = take.wait_for_event(&["writing", "text"]);
    assert!(
        matches!(events[1], SpeechEventKind::Text(ref text) if text == "hei"),
        "{events:?}"
    );
    assert!(marker.exists(), "the transcriber of the settings ran");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn escape_cancels_the_take_and_deletes_the_file() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    let recorder_pid = pid_of(&take, "rec");
    take.clock.advance(Duration::from_secs(2));
    take.input.cancel();
    take.wait_for_event(&["dropped"]);
    assert!(!marker.exists(), "a cancelled take runs no transcriber");
    take.wait_for("the delete of the file", || take.files().is_empty());
    take.wait_for("the end of the recorder", || !alive(recorder_pid));
}

#[test]
fn a_take_that_reaches_its_bound_of_120_seconds_stops_by_itself() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, marker) = transcriber_script(&take, "long");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    // The recorder runs before the clock of the test moves, so the bound of the take
    // is reached by this test and not by a machine that starts `/bin/sh` after the
    // bound has passed.
    take.wait_for_recorder("rec");
    // The bound of the take is the bound of the code: 120 seconds of the clock of
    // the test. The test moves that clock, so no key event is sent and the take ends
    // alone.
    take.clock.advance(MAX_TAKE);
    let events = take.wait_for_event(&["writing", "text"]);
    assert!(
        matches!(events[1], SpeechEventKind::Text(ref text) if text == "long"),
        "{events:?}"
    );
    assert!(marker.exists());
    assert_eq!(MAX_TAKE, Duration::from_secs(120));
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_recorder_that_is_not_installed_names_the_program() {
    let take = Take::new(Arc::new(()));
    let recorder = CommandLine::parse("hz-no-such-recorder {file}").expect("a line");
    let (transcriber, _marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    let events = take.wait_for_event(&["failed"]);
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert!(message.contains("hz-no-such-recorder"), "{message}");
    assert!(message.contains("is not installed"), "{message}");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_recorder_that_ends_early_answers_its_last_line() {
    let take = Take::new(Arc::new(()));
    let program = take.script("rec", "#!/bin/sh\nprintf 'the device is busy\\n' >&2\nexit 1\n");
    let recorder = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    let (transcriber, _marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    let events = take.wait_for_event(&["failed"]);
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert!(message.contains("ended early"), "{message}");
    assert!(message.contains("the device is busy"), "{message}");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

/// A recorder that ends on its own is a take that ends while the hotkey is still
/// down: the person reads the sentence at once and not after the release, so no
/// second of speech goes into a recorder that is already gone.
#[test]
fn a_recorder_that_ends_while_the_key_is_held_fails_the_take_without_a_release() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let pid = take.pid_file("slow");
    let program = take.script(
        "slow",
        &format!(
            "#!/bin/sh\ncp '{}' \"$1\"\nprintf '%s\\n' \"$$\" > '{}'\nsleep 0.3\nprintf 'the sound server is away\\n' >&2\nexit 1\n",
            prepared.display(),
            pid.display()
        ),
    );
    let recorder = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    let (transcriber, _marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("slow");
    // No key event follows: the recorder ends by itself, three tenths of a second
    // after it started, and the worker reads that on its next look at the child.
    let started = Instant::now();
    let events = take.wait_for_event(&["failed"]);
    let noticed = started.elapsed();
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert!(message.contains("ended early"), "{message}");
    assert!(message.contains("the sound server is away"), "{message}");
    assert!(
        noticed < Duration::from_secs(1),
        "the person reads the failure while the key is still down, and not after {noticed:?}"
    );
    // The release of a key whose take already ended does nothing more: no second
    // event, no transcription and no file of a take.
    take.input.end();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(take.events(), Vec::new(), "a later release adds no event");
    assert!(take.files().is_empty(), "the file of the take is gone");
}

#[test]
fn a_recorder_that_ignores_the_interrupt_signal_is_stopped_by_the_termination_signal() {
    let mut take = Take::new(Arc::new(()));
    let bystander = take.bystander();
    let prepared = take.prepared();
    let terminated = take.home.path().join("terminated.txt");
    let traps = format!("trap 'printf term > \"{}\"; exit 0' TERM", terminated.display());
    let recorder = take.ignoring("signalled", "INT", &prepared, &traps);
    let (transcriber, _marker) = transcriber_script(&take, "kept");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("signalled");
    let recorder_pid = pid_of(&take, "signalled");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    // The recorder cannot trap `SIGINT`, and the stop steps to `SIGTERM` inside its
    // own bound: the take is transcribed and no kill was needed.
    let events = take.wait_for_event(&["writing", "text"]);
    assert!(
        matches!(events[1], SpeechEventKind::Text(ref text) if text == "kept"),
        "{events:?}"
    );
    take.wait_for("the end of the recorder", || !alive(recorder_pid));
    assert!(
        terminated.exists(),
        "the recorder ended on the termination signal and not on the kill of the bound"
    );
    assert!(
        alive(bystander),
        "the signal of the stop reached a program of this computer that the take did not start"
    );
}

#[test]
fn a_recorder_that_ignores_both_signals_ends_by_the_kill_of_the_bound() {
    let mut take = Take::new(Arc::new(()));
    let bystander = take.bystander();
    let prepared = take.prepared();
    let recorder = take.ignoring("stubborn", "INT TERM", &prepared, "");
    let (transcriber, _marker) = transcriber_script(&take, "kept");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("stubborn");
    let recorder_pid = pid_of(&take, "stubborn");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    // The recorder ignores both signals, so the stop of the take waits its bound of
    // two seconds and kills that one process id. The worker sets the deadline of the
    // stop from the instant of its own signal, so the test moves the clock in steps
    // and the worker reads each one.
    take.wait_for_event_on(Some(POLL), &["writing", "text"]);
    take.wait_for("the end of the stubborn recorder", || !alive(recorder_pid));
    assert!(
        alive(bystander),
        "the signal of the stop reached a program of this computer that the take did not start"
    );
}

#[test]
fn a_recording_that_its_recorder_did_not_close_is_still_sent() {
    let take = Take::new(Arc::new(()));
    let unclosed = take.unclosed(1024);
    // The recorder copies the file and ignores both signals, so the stop kills it and
    // it never writes the length of the recording, as a killed recorder never does.
    let recorder = take.ignoring("killed", "INT TERM", &unclosed, "");
    let copy = take.home.path().join("copied.wav");
    let program = take.script(
        "copy",
        &format!("#!/bin/sh\ncp \"$1\" '{}'\nprintf 'kept'", copy.display()),
    );
    let transcriber = CommandLine::parse(&format!("{program} {{file}}")).expect("a transcriber line");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("killed");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    let events = take.wait_for_event_on(Some(POLL), &["writing", "text"]);
    assert!(
        matches!(events[1], SpeechEventKind::Text(ref text) if text == "kept"),
        "{events:?}"
    );
    let sent = fs::read(&copy).expect("the file that the transcriber received");
    let riff = u32::from_le_bytes(sent[4..8].try_into().expect("the RIFF size"));
    let data = u32::from_le_bytes(sent[40..44].try_into().expect("the data size"));
    assert_eq!(
        (u64::from(riff), u64::from(data)),
        (sent.len() as u64 - 8, sent.len() as u64 - 44),
        "the take that the transcriber received is a closed recording"
    );
}

#[test]
fn a_recording_of_another_shape_is_sent_as_it_is() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let copy = take.home.path().join("copied.wav");
    let program = take.script(
        "copy",
        &format!("#!/bin/sh\ncp \"$1\" '{}'\nprintf 'kept'", copy.display()),
    );
    let transcriber = CommandLine::parse(&format!("{program} {{file}}")).expect("a transcriber line");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    take.wait_for_event(&["writing", "text"]);
    // The prepared file is not the canonical shape of a PCM header, so every byte of
    // it reaches the transcriber unchanged.
    assert_eq!(
        fs::read(&copy).expect("the file that the transcriber received"),
        fs::read(&prepared).expect("the prepared recording"),
        "a file of another shape is sent as it is"
    );
}

#[test]
fn no_file_of_a_take_stays_after_a_success() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, _marker) = transcriber_script(&take, "kept");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    take.wait_for_event(&["writing", "text"]);
    take.wait_for("the delete of the file", || take.files().is_empty());
    assert!(take.folder().exists(), "the folder of the takes stays");
}

#[test]
fn a_recording_above_the_bound_of_the_file_is_refused() {
    // The bound of this test is one kilobyte, so the sentence names one megabyte:
    // the production bound of 25 MB names 25.
    let take = Take::with_bounds(
        Arc::new(()),
        TakeBounds {
            max_file_bytes: 1024,
            ..TakeBounds::default()
        },
    );
    let program = take.script(
        "rec",
        "#!/bin/sh\nhead -c 4096 /dev/zero > \"$1\"\ntrap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done\n",
    );
    let recorder = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    let (transcriber, marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    // The recorder writes the whole file at its start, so the take is over its
    // bound before the key comes up.
    take.wait_for("the recording", || take.file_bytes() >= 4096);
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    let events = take.wait_for_event(&["failed"]);
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert!(message.contains("larger than 1 MB"), "{message}");
    assert!(!marker.exists(), "a refused file runs no transcriber");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_take_of_no_byte_is_dropped() {
    let take = Take::new(Arc::new(()));
    let started = take.pid_file("empty");
    let program = take.script(
        "rec",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > '{}'\ntrap 'exit 0' INT TERM\nwhile :; do sleep 0.02; done\n",
            started.display()
        ),
    );
    let recorder = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    let (transcriber, marker) = transcriber_script(&take, "never");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    // This recorder copies no byte, so its pid alone says that it runs and the clock
    // of the test may move.
    take.wait_for("the recorder", || started.exists());
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    take.wait_for_event(&["dropped"]);
    assert!(!marker.exists());
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_missing_secret_of_the_service_starts_no_request() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let service = Transcriber::Service {
        endpoint: Endpoint::parse("http://127.0.0.1:9/v1").expect("an endpoint of this computer"),
        model: "gpt-transcribe".to_owned(),
        language: None,
        key_secret: SecretName::parse("MY_SPEECH_KEY").expect("a name"),
    };
    take.input.begin(plan(recorder, service), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    let events = take.wait_for_event(&["failed"]);
    let SpeechEventKind::Failed(message) = &events[0] else {
        panic!("a failure: {events:?}");
    };
    assert_eq!(message, "No secret named MY_SPEECH_KEY is on this computer");
    take.wait_for("the delete of the file", || take.files().is_empty());
}

#[test]
fn a_transcriber_command_that_does_not_answer_hits_its_bound() {
    let take = Take::with_bounds(
        Arc::new(()),
        TakeBounds {
            transcribe: Duration::from_millis(200),
            ..TakeBounds::default()
        },
    );
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let program = take.script("slow", "#!/bin/sh\nsleep 5\nprintf 'late'\n");
    let transcriber = CommandLine::parse(&format!("{program} {{file}}")).expect("a line");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    // A command that a slow machine did not answer in time says nothing about
    // the take, so the file stays for one retry.
    let events = take.wait_for_event(&["kept"]);
    let SpeechEventKind::Kept(message) = &events[0] else {
        panic!("a kept take: {events:?}");
    };
    assert!(message.contains("did not answer in 0 seconds"), "{message}");
    assert_eq!(
        take.files().len(),
        1,
        "the take of a transcriber that timed out is kept"
    );
}

#[test]
fn the_folder_and_the_file_of_a_take_are_private() {
    let take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, _marker) = transcriber_script(&take, "kept");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    // The recorder copies its file at its start, so the file holds the recording
    // before the take ends and a take of this test is not an empty one.
    take.wait_for("the recording of the take", || take.file_bytes() > 0);
    let folder = fs::metadata(take.folder()).expect("the folder of the takes");
    assert_eq!(
        folder.permissions().mode() & 0o777,
        0o700,
        "the folder of the takes is private"
    );
    let file = take.folder().join(&take.files()[0]);
    let metadata = fs::metadata(&file).expect("the file of the take");
    assert_eq!(
        metadata.permissions().mode() & 0o777,
        0o600,
        "the file of the take is private"
    );
    assert!(
        file.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("take-")),
        "the file of a take is named take-<random>.wav"
    );
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    take.wait_for_event(&["writing", "text"]);
}

#[test]
fn a_start_keeps_the_three_newest_takes_of_a_day_and_sweeps_the_rest() {
    let home = tempfile::tempdir().expect("a home of a test");
    let folder = home.path().join(TAKE_FOLDER);
    fs::create_dir_all(&folder).expect("the folder of the takes");
    // Five takes that a person could send again, oldest first.
    for index in 0..5_u64 {
        let path = folder.join(format!("take-000{index}.wav"));
        fs::write(&path, b"kept").expect("a kept take");
        let modified = std::time::SystemTime::now() - Duration::from_secs(60 * (5 - index));
        let file = fs::File::options().write(true).open(&path).expect("the file");
        file.set_modified(modified).expect("the age of a take");
    }
    // One take of two days ago, and one file that is no take at all.
    let old = folder.join("take-old.wav");
    fs::write(&old, b"old").expect("an old take");
    fs::File::options()
        .write(true)
        .open(&old)
        .expect("the file")
        .set_modified(std::time::SystemTime::now() - Duration::from_hours(48))
        .expect("the age of an old take");
    let other = home.path().join("notes.txt");
    fs::write(&other, b"not a take").expect("another file");

    let worker = SpeechWorker::new(home.path().to_path_buf(), Arc::new(()) as Arc<dyn SpeechKeyStore>);
    let _input = SpeechInput::start(worker);
    let deadline = Instant::now() + PATIENCE;
    while old.exists() || folder.join("take-0000.wav").exists() {
        assert!(
            Instant::now() < deadline,
            "the start of the window never swept the takes"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut left: Vec<String> = fs::read_dir(&folder)
        .expect("the folder")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec!["take-0002.wav", "take-0003.wav", "take-0004.wav"],
        "the folder keeps the three newest takes of the last day"
    );
    assert!(other.exists(), "a file that is no take is left alone");
}

#[test]
fn a_take_that_records_when_the_window_closes_leaves_no_file_and_no_child() {
    let mut take = Take::new(Arc::new(()));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let (transcriber, _marker) = transcriber_script(&take, "kept");
    take.input
        .begin(plan(recorder, Transcriber::Command(transcriber)), TARGET);
    take.wait_for_recorder("rec");
    let recorder_pid = pid_of(&take, "rec");
    // The window closes: the handle is dropped while the take records.
    let input = std::mem::replace(
        &mut take.input,
        SpeechInput::start(SpeechWorker::new(take.home.path().to_path_buf(), Arc::new(()))),
    );
    drop(input);
    take.wait_for("the end of the recorder", || !alive(recorder_pid));
    take.wait_for("the delete of the file", || take.files().is_empty());
}

/// The variables of the helper of the test below: the home of its take, the recorder
/// stand-in of the take, and the file that the stand-in writes its process id into.
const HELPER_HOME: &str = "HZ_SPEECH_TAKE_HELPER_HOME";
const HELPER_RECORDER: &str = "HZ_SPEECH_TAKE_HELPER_RECORDER";
const HELPER_PID: &str = "HZ_SPEECH_TAKE_HELPER_PID";

/// The name of the helper test that a test starts in a process of its own.
const HELPER_TEST: &str = "speech::take::tests::a_helper_that_owns_a_take_waits_for_its_parent";

/// The helper of the test below: this process owns a take and records until its
/// parent kills it.
///
/// The take of a real window lives in the process of that window, and a window that a
/// kill ends runs no cleanup. Only a process of its own shows that the recorder of such
/// a take ends, so the test below starts this function in one, and a run of the suite
/// alone finds no variable of the helper and answers at once.
#[test]
fn a_helper_that_owns_a_take_waits_for_its_parent() {
    let (Some(home), Some(recorder), Some(pid)) = (
        std::env::var_os(HELPER_HOME),
        std::env::var_os(HELPER_RECORDER),
        std::env::var_os(HELPER_PID),
    ) else {
        return;
    };
    let pid = PathBuf::from(pid);
    let recorder = recorder.to_string_lossy().into_owned();
    let plan = plan(
        CommandLine::parse(&format!("{recorder} {{file}}")).expect("a recorder line"),
        Transcriber::Command(CommandLine::parse("/bin/true {file}").expect("a transcriber line")),
    );
    let input = SpeechInput::start(SpeechWorker::new(PathBuf::from(home), Arc::new(())));
    input.begin(plan, TARGET);
    let deadline = Instant::now() + PATIENCE;
    while !pid.exists() {
        assert!(Instant::now() < deadline, "the recorder of the helper never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The parent kills this process now, so the take never ends by itself.
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn a_take_that_its_window_kills_leaves_no_recorder() {
    let home = tempfile::tempdir().expect("a home of a test");
    let prepared = home.path().join("prepared.wav");
    fs::write(&prepared, b"RIFF$\x00\x00\x00WAVEfmt ").expect("a recording of a test");
    let pid_file = home.path().join("helper.pid");
    let recorder = crate::util::stand_in_shell_in(
        home.path(),
        "rec",
        &format!(
            "cp '{}' \"$1\"\nprintf '%s\\n' \"$$\" > '{}'\nwhile :; do sleep 0.02; done",
            prepared.display(),
            pid_file.display()
        ),
    )
    .expect("a recorder of a test");

    let mut helper = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", HELPER_TEST, "--nocapture", "--test-threads", "1"])
        .env(HELPER_HOME, home.path())
        .env(HELPER_RECORDER, &recorder)
        .env(HELPER_PID, &pid_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the helper of the test");

    // The recorder of the helper records before the helper is killed.
    let deadline = Instant::now() + PATIENCE;
    while !pid_file.exists() {
        assert!(
            helper.try_wait().expect("the state of the helper").is_none(),
            "the helper of the test ended before it recorded"
        );
        assert!(Instant::now() < deadline, "the recorder of the helper never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let recorder_pid = pid_of_file(&pid_file);
    assert!(alive(recorder_pid), "the recorder of the helper records");

    // The window is killed: no function of its process runs, so only the promise that
    // the recorder holds ends it.
    let _ = helper.kill();
    let _ = helper.wait();
    let deadline = Instant::now() + Duration::from_secs(1);
    while alive(recorder_pid) {
        assert!(
            Instant::now() < deadline,
            "the recorder of a killed window still runs after one second"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The key store of a test: one value for one name.
#[derive(Debug)]
struct Keys {
    name: String,
    value: String,
}

impl SpeechKeyStore for Keys {
    fn value(&self, name: &SecretName) -> Result<Option<zeroize::Zeroizing<String>>, String> {
        Ok((name.as_str() == self.name).then(|| zeroize::Zeroizing::new(self.value.clone())))
    }
}

#[test]
fn the_value_of_the_key_reaches_one_request_and_no_file_of_the_take() {
    // The request of this test goes to a stand-in of this computer, which records
    // the bytes that it received. The key reaches the header of that request and no
    // file of the take.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a stand-in service");
    let address = listener.local_addr().expect("the address of the stand-in").to_string();
    let server = std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let (mut stream, _) = listener.accept().expect("one request");
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        let mut expected = None;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&buffer[..read]),
            }
            if expected.is_none()
                && let Some(head) = request.windows(4).position(|window| window == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..head]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                expected = Some(head + 4 + length);
            }
            if expected.is_some_and(|expected| request.len() >= expected) {
                break;
            }
        }
        let body = br#"{"text":"tale"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
        String::from_utf8_lossy(&request).into_owned()
    });

    let take = Take::new(Arc::new(Keys {
        name: "MY_SPEECH_KEY".to_owned(),
        value: "sk-the-value-of-the-key".to_owned(),
    }));
    let prepared = take.prepared();
    let recorder = take.recorder("rec", &prepared);
    let service = Transcriber::Service {
        endpoint: Endpoint::parse(&format!("http://{address}/v1/audio/transcriptions")).expect("an endpoint"),
        model: "gpt-transcribe".to_owned(),
        language: Some("no".to_owned()),
        key_secret: SecretName::parse("MY_SPEECH_KEY").expect("a name"),
    };
    take.input.begin(plan(recorder, service), TARGET);
    take.wait_for_recorder("rec");
    take.clock.advance(Duration::from_secs(1));
    take.input.end();
    let events = take.wait_for_event(&["writing", "text"]);
    assert!(
        matches!(events[1], SpeechEventKind::Text(ref text) if text == "tale"),
        "{events:?}"
    );
    let request = server.join().expect("the request of the stand-in");
    assert!(
        request.contains("authorization: Bearer sk-the-value-of-the-key"),
        "{request}"
    );
    assert!(request.contains("name=\"language\"\r\n\r\nno"), "{request}");
    assert!(request.contains("name=\"file\"; filename=\"take-"), "{request}");
    take.wait_for("the delete of the file", || take.files().is_empty());
}
