//! The program of a notification: the arguments of each system, the lookup, and
//! the worker that runs the program off the frame.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{BOUND, DesktopSink, Sink, System, arguments, find_program_in, markup_escaped, missing_sentence};
use crate::ProgramEnvironment;
use crate::bounded_program::ProgramFailure;
use crate::notify::{Notice, TITLE};

/// A folder of stand-in programs that no test of the person touches.
fn folder() -> tempfile::TempDir {
    tempfile::tempdir().expect("a folder of stand-in programs")
}

fn notice(body: &str) -> Notice {
    Notice::new(body)
}

#[test]
fn the_program_of_linux_reads_the_text_after_the_end_of_its_options() {
    let args = arguments(System::Linux, &notice("2 agents need you"));
    let args: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    assert_eq!(args[0], "--app-name=Horizon");
    assert_eq!(args[1], "--", "the options of the program end before the text");
    assert_eq!(args[2], TITLE);
    assert_eq!(args[3], "2 agents need you");
    assert_eq!(args.len(), 4);
}

#[test]
fn a_name_that_looks_like_an_option_arrives_as_text() {
    let args = arguments(System::Linux, &notice("--urgency=critical needs you"));
    let end = args.iter().position(|arg| arg == "--").expect("the end of the options");
    assert!(
        args[end + 1..].iter().any(|arg| arg == "--urgency=critical needs you"),
        "the name stands after the end of the options: {args:?}"
    );
    assert!(
        !args[..end].iter().any(|arg| arg.to_string_lossy().contains("urgency")),
        "no option of the program reads the name: {args:?}"
    );
}

#[test]
fn the_markup_of_a_notification_server_is_escaped() {
    assert_eq!(markup_escaped("Fix <b> & </b>"), "Fix &lt;b&gt; &amp; &lt;/b&gt;");
    let args = arguments(System::Linux, &notice("<b>Fix</b> & more needs you"));
    assert_eq!(args[3], "&lt;b&gt;Fix&lt;/b&gt; &amp; more needs you");
}

#[test]
fn the_script_of_macos_holds_no_name() {
    let args = arguments(System::MacOs, &notice("Fix the sync needs you \u{00B7} Lane"));
    let args: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    // The script is a constant: it reads the text from the argument list.
    assert_eq!(args[0], "-e");
    assert_eq!(args[1], "on run argv");
    assert_eq!(
        args[3],
        "display notification (item 2 of argv) with title (item 1 of argv)"
    );
    assert_eq!(args[5], "end run");
    assert_eq!(args[6], "--", "the arguments of the script start after its options");
    assert_eq!(args[7], TITLE);
    assert_eq!(args[8], "Fix the sync needs you \u{00B7} Lane");
    assert_eq!(args.len(), 9);
    let (script, text) = args.split_at(7);
    assert!(
        !script.iter().any(|part| part.contains("Fix")),
        "the name is no part of the script: {script:?}"
    );
    assert!(text.iter().any(|part| part.contains("Fix")), "{text:?}");
}

#[test]
fn the_program_is_looked_up_in_the_folders_of_the_path() {
    let folder = folder();
    let program = crate::util::stand_in_program_in(folder.path(), "notify-send", "exit 0").expect("a stand-in");
    let found = find_program_in(System::Linux, &ProgramEnvironment::with_path(folder.path().as_os_str()));
    assert_eq!(found, Some(program), "the folder of the stand-in holds the program");
    let empty = folder.path().join("empty");
    std::fs::create_dir(&empty).expect("an empty folder");
    assert_eq!(
        find_program_in(System::Linux, &ProgramEnvironment::with_path(empty.as_os_str())),
        None
    );
}

#[test]
fn the_program_is_looked_up_off_the_frame() {
    // The whole suite of the window runs the guard in strict mode: a lookup on a
    // frame panics here instead of reading the disks of the person on a frame.
    crate::ui_thread::use_strict_mode();
    let folder = folder();
    crate::util::stand_in_program_in(folder.path(), "notify-send", "exit 0").expect("a stand-in");
    let environment = ProgramEnvironment::with_path(folder.path().as_os_str());
    let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = crate::ui_thread::FrameScope::enter();
        find_program_in(System::Linux, &environment)
    }));
    let payload = frame.expect_err("a lookup on a frame is reported");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .expect("the guard reports a sentence");
    assert!(
        message.contains("the program of a desktop notification is looked up"),
        "the report names the lookup: {message}"
    );
    assert!(
        find_program_in(System::Linux, &environment).is_some(),
        "off the frame it answers"
    );
}

/// The line that a stand-in program writes, with its arguments, one per line.
fn recording_program(folder: &Path, log: &Path) -> PathBuf {
    let body = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display());
    crate::util::stand_in_program_in(folder, "notify-send", &body).expect("the stand-in program")
}

/// Waits until `path` holds the arguments of one notification.
fn wait_for_log(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "the stand-in program did not run");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_worker_runs_the_program_of_this_computer_with_the_text_of_the_notification() {
    let folder = folder();
    let log = folder.path().join("arguments");
    let program = recording_program(folder.path(), &log);
    let sink = DesktopSink::start_with_lookup(System::Linux, move || Some(program));
    sink.send(notice("Fix the sync needs you"));
    let lines: Vec<String> = wait_for_log(&log).lines().map(str::to_owned).collect();
    assert_eq!(lines, ["--app-name=Horizon", "--", TITLE, "Fix the sync needs you"]);
    assert_eq!(sink.available(), Some(true), "the run found its program");
}

#[test]
fn a_computer_without_the_program_shows_nothing_and_says_so() {
    let folder = folder();
    let log = folder.path().join("arguments");
    let sink = DesktopSink::start_with_lookup(System::Linux, || None);
    sink.send(notice("Fix the sync needs you"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while sink.available().is_none() {
        assert!(
            Instant::now() < deadline,
            "the worker answers whether a program is there"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sink.available(), Some(false));
    assert!(!log.exists());
    assert_eq!(missing_sentence(System::Linux), "notify-send is not installed");
}

#[test]
fn a_program_that_hangs_is_ended_at_the_bound() {
    let folder = folder();
    // `exec` makes the shell the sleeping program itself, so the pipes of the
    // child close when the bound ends that one process id.
    let program = crate::util::stand_in_program_in(folder.path(), "slow", "#!/bin/sh\nexec sleep 30\n")
        .expect("a stand-in that does not end");
    let start = Instant::now();
    let result = super::show(
        &program,
        System::Linux,
        &notice("Fix the sync needs you"),
        Duration::from_millis(200),
    );
    assert_eq!(
        result,
        Err(ProgramFailure::TimedOut),
        "the bound ended the program by its own pid"
    );
    let elapsed = start.elapsed();
    assert!(elapsed < BOUND, "the program did not run to its own end: {elapsed:?}");
    // `bounded_program::run` waits for the program that it ended, so no child of
    // this process stays behind.
    assert!(std::path::Path::new(&program).exists());
}

#[test]
fn a_frame_only_writes_the_notification_into_the_worker() {
    crate::ui_thread::use_strict_mode();
    let folder = folder();
    let log = folder.path().join("arguments");
    let program = recording_program(folder.path(), &log);
    let sink = DesktopSink::start_with_lookup(System::Linux, move || Some(program));
    // The frame ends the value of the notification, and the program runs on the
    // worker: a program that a frame started itself would panic here.
    let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = crate::ui_thread::FrameScope::enter();
        sink.send(notice("Fix the sync needs you"));
    }));
    assert!(frame.is_ok(), "the frame did not run the program");
    let lines: Vec<String> = wait_for_log(&log).lines().map(str::to_owned).collect();
    assert_eq!(lines.len(), 4, "the worker ran the program: {lines:?}");
}
