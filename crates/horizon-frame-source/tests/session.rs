//! One session of a real stand-in program: its pictures, its control messages, its
//! descriptor 3, its exact process id and its end.
//!
//! The program of every test is `scripts/device-smoke/frame_source_standin.py`, which
//! speaks the contract and draws a pattern of its own. No test starts a screen
//! capture, a program that reads this machine's screen or a real frame source, and
//! no test writes outside its own temporary folder.
use std::{
    ffi::OsString,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use horizon_frame_source::{
    Button, Config, InputState, Key, Point, STOP_WAIT, Session, Status, VIEW_DEBOUNCE, keys::NamedKey,
};

/// The most time that one test waits for the stand-in to say something.
const PATIENCE: Duration = Duration::from_secs(20);

fn standin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/device-smoke/frame_source_standin.py")
}

/// One session of the stand-in, with the races of a real program taken out of the
/// test: every test waits for the state that it expects.
struct Viewer {
    session: Session,
    folder: tempfile::TempDir,
    wakes: mpsc::Receiver<()>,
    children: Vec<u32>,
}

impl Viewer {
    fn start(arguments: &[&str]) -> Self {
        let folder = tempfile::tempdir().expect("a folder of a test");
        let root = folder.path().to_string_lossy().into_owned();
        // Every path of an argument is a path of the folder of this test, so the
        // program of the test writes nothing outside it.
        let arguments: Vec<OsString> = arguments
            .iter()
            .map(|argument| OsString::from(argument.replace("{folder}", &root)))
            .collect();
        let (wakes, woken) = mpsc::channel();
        let config = Config {
            name: "frame_source_standin".into(),
            program: PathBuf::from("python3"),
            arguments: [
                OsString::from("-B"),
                standin().into_os_string(),
                OsString::from("--rate"),
                OsString::from("50"),
            ]
            .into_iter()
            .chain(arguments)
            .collect(),
        };
        let session = Session::start(config, move |_wake| {
            let _ = wakes.send(());
        })
        .expect("a session of the stand-in");
        Self {
            session,
            folder,
            wakes: woken,
            children: Vec::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.folder.path().join(name)
    }

    /// The process id that the stand-in wrote into its own report.
    fn child_pid(&self) -> u32 {
        let report = self.path("report.json");
        let deadline = Instant::now() + PATIENCE;
        while !report.exists() {
            assert!(Instant::now() < deadline, "the stand-in never wrote its report");
            std::thread::sleep(Duration::from_millis(5));
        }
        let text = std::fs::read_to_string(&report).expect("the report of the stand-in");
        let value: serde_json::Value = serde_json::from_str(text.trim()).expect("one JSON report");
        u32::try_from(value["pid"].as_u64().expect("a process id")).expect("a process id")
    }

    /// Wait until the session has a picture, and answer it.
    fn wait_for_picture(&mut self) -> horizon_vnc::Image {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(image) = self.session.take_updates().image {
                return image;
            }
            assert!(Instant::now() < deadline, "the stand-in never sent a picture");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait until the session answers a state other than the first "connecting".
    fn wait_for_status(&mut self) -> Status {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.session.observation().status
                && status != Status::Connecting
            {
                return status;
            }
            assert!(Instant::now() < deadline, "the stand-in never answered a state");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait until `ready` answers, and take the waiting updates as a frame does,
    /// so a picture that the frame did not take never waits for a conversion that
    /// this test would then expect.
    fn wait_for(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let _ = self.session.take_updates();
            if ready(self) {
                return;
            }
            assert!(Instant::now() < deadline, "the test waited for {what} in vain");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// End the viewer and wait until every child of the test is gone.
    fn end(mut self) {
        self.session.detach();
        for pid in std::mem::take(&mut self.children) {
            wait_until_gone(pid);
        }
    }
}

/// Whether `pid` is a process that still runs.
fn alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    rustix::process::test_kill_process(pid).is_ok()
}

/// Wait until a stopped program is gone, so a test never races the kernel.
fn wait_until_gone(pid: u32) {
    let deadline = Instant::now() + PATIENCE;
    while alive(pid) {
        if Instant::now() >= deadline {
            let _ = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap_or(0))
                .map(|pid| rustix::process::kill_process(pid, rustix::process::Signal::KILL));
            panic!("the program {pid} still ran after {PATIENCE:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_viewer_shows_the_pictures_of_its_program() {
    let mut viewer = Viewer::start(&["--width", "64", "--height", "32", "--report", "{folder}/report.json"]);
    assert_eq!(viewer.wait_for_status(), Status::Connected, "hello is the connection");
    let image = viewer.wait_for_picture();
    assert_eq!(image.frame.size(), [64, 32]);
    assert!(viewer.session.has_latest_full());
    assert_eq!(viewer.session.latest_full().map(|frame| frame.size()), Some([64, 32]));

    // The pictures of a stream that moves are new pictures, and each of them asks
    // for one frame of the UI.
    let before = viewer.session.stream_evidence().sequence;
    viewer.wait_for("two more pictures", |viewer| {
        viewer.session.stream_evidence().sequence > before + 1
    });
    let mut wakes = 0;
    while viewer.wakes.try_recv().is_ok() {
        wakes += 1;
    }
    assert!(wakes > 0, "a new picture asks for one repaint");
    let pid = viewer.child_pid();
    viewer.children.push(pid);
    viewer.end();
}

#[test]
fn a_still_stream_asks_for_no_frame_of_the_ui() {
    let mut viewer = Viewer::start(&[
        "--width",
        "32",
        "--height",
        "16",
        "--rate",
        "0.2",
        "--report",
        "{folder}/report.json",
    ]);
    viewer.wait_for_picture();
    // Drain the wakes of the pictures that arrived, and then prove that a stream
    // that sends nothing asks for nothing: no repaint loop, and no second thread
    // that keeps a frame busy.
    while viewer.wakes.try_recv().is_ok() {}
    std::thread::sleep(Duration::from_millis(300));
    while viewer.session.take_updates().image.is_some() {}
    assert!(
        viewer.wakes.try_recv().is_err(),
        "a viewer whose program sends no picture asks for no frame"
    );
    let pid = viewer.child_pid();
    viewer.children.push(pid);
    viewer.end();
}

#[test]
fn the_child_gets_the_picture_socket_as_descriptor_three() {
    let mut viewer = Viewer::start(&["--width", "32", "--height", "16", "--report", "{folder}/report.json"]);
    let pid = viewer.child_pid();
    let report = std::fs::read_to_string(viewer.path("report.json")).expect("the report");
    let value: serde_json::Value = serde_json::from_str(report.trim()).expect("one JSON report");
    assert_eq!(value["socket_fd"], 3, "the socket is descriptor 3 of the child");
    assert_eq!(value["socket"], true, "descriptor 3 is one stream socket");
    assert_eq!(u32::try_from(value["pid"].as_u64().unwrap_or(0)).ok(), Some(pid));
    viewer.children.push(pid);
    viewer.end();
}

#[test]
fn close_sends_bye_and_stops_the_exact_program() {
    let mut viewer = Viewer::start(&["--width", "32", "--height", "16", "--report", "{folder}/report.json"]);
    viewer.wait_for_picture();
    let pid = viewer.child_pid();
    assert!(alive(pid), "the stand-in runs");
    let started = Instant::now();
    viewer.session.detach();
    wait_until_gone(pid);
    assert!(
        started.elapsed() < STOP_WAIT * 2,
        "the program that reads bye ends at once, and no signal was needed"
    );
}

#[test]
fn a_dropped_session_stops_its_program() {
    let mut viewer = Viewer::start(&["--width", "32", "--height", "16", "--report", "{folder}/report.json"]);
    viewer.wait_for_picture();
    let pid = viewer.child_pid();
    assert!(alive(pid));
    drop(viewer.session);
    wait_until_gone(pid);
}

#[test]
fn a_header_that_breaks_a_bound_ends_the_viewer_and_stops_the_child() {
    let viewer = Viewer::start(&[
        "--width",
        "32",
        "--height",
        "16",
        "--garbage-header",
        "--report",
        "{folder}/report.json",
    ]);
    let pid = viewer.child_pid();
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(Status::Disconnected { message, .. }) = viewer.session.observation().status {
            break message;
        }
        assert!(Instant::now() < deadline, "a picture of another magic ends the viewer");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(status.contains("NV12FRM1"), "{status}");
    wait_until_gone(pid);
}

#[test]
fn a_program_that_is_not_on_this_computer_is_one_sentence() {
    let (wakes, _woken) = mpsc::channel();
    let config = Config {
        name: "frame_source_standin".into(),
        program: PathBuf::from("/nonexistent/frame-tool-of-a-test"),
        arguments: Vec::new(),
    };
    let session = Session::start(config, move |_wake| {
        let _ = wakes.send(());
    })
    .expect("a session without a program");
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = session.observation().status
            && status != Status::Connecting
        {
            break status;
        }
        assert!(Instant::now() < deadline, "a missing program says so");
        std::thread::sleep(Duration::from_millis(5));
    };
    let Status::Disconnected { message, .. } = status else {
        panic!("a program that no folder holds does not connect: {status:?}");
    };
    assert!(message.contains("not installed on this computer"), "{message}");
}

#[test]
fn a_busy_program_is_the_sentence_of_its_own_answer() {
    let mut viewer = Viewer::start(&["--busy"]);
    let status = viewer.wait_for_status();
    let Status::Disconnected { message, .. } = status else {
        panic!("a busy source does not connect: {status:?}");
    };
    assert!(message.contains("another viewer"), "{message}");
}

#[test]
fn the_program_reads_no_input_when_its_hello_says_so() {
    let viewer = Viewer::start(&["--width", "32", "--height", "16"]);
    let deadline = Instant::now() + PATIENCE;
    let input = loop {
        if let Some(input) = viewer.session.take_updates().input {
            break input;
        }
        assert!(Instant::now() < deadline, "hello says whether the program reads input");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!input.allowed, "the stand-in reads no input without --input");
    assert_eq!(input.reason.as_deref(), Some("this stand-in reads no input"));
}

#[test]
fn each_input_event_reaches_the_program_in_the_pixels_of_the_picture() {
    let mut viewer = Viewer::start(&[
        "--width",
        "64",
        "--height",
        "32",
        "--rate",
        "0",
        "--input",
        "{folder}/input.jsonl",
        "--report",
        "{folder}/report.json",
    ]);
    let deadline = Instant::now() + PATIENCE;
    let input = loop {
        if let Some(input) = viewer.session.take_updates().input {
            break input;
        }
        assert!(Instant::now() < deadline, "the stand-in never said hello");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(input.allowed, "this stand-in reads input");

    let mut state = InputState::default();
    let mut events = state.pointer(Some(Point { x: 5, y: 6 }));
    events.extend(state.button(Button::Left, true, Some(Point { x: 5, y: 6 })));
    events.extend(state.wheel(0, 1));
    events.extend(state.key(Key::Named(NamedKey::Enter), true));
    events.extend(state.key(Key::Character('a'), true));
    events.extend(state.key(Key::Character('a'), false));
    events.extend(InputState::text("hei"));
    events.extend(state.button(Button::Left, false, None));
    viewer.session.send_input(events);

    let log = viewer.path("input.jsonl");
    viewer.wait_for("the input of the test", |_| {
        log.exists() && std::fs::read_to_string(&log).map_or(0, |text| text.lines().count()) >= 7
    });
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .expect("the log of the input")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON line"))
        .collect();
    assert_eq!(
        lines[0],
        serde_json::json!({"type": "input", "event": "pointer", "x": 5, "y": 6})
    );
    assert_eq!(lines[1]["event"], "button");
    assert_eq!(lines[1]["button"], "left");
    assert_eq!(lines[1]["down"], true);
    assert_eq!(lines[2]["event"], "wheel");
    assert_eq!((lines[2]["dx"].clone(), lines[2]["dy"].clone()), (0.into(), 1.into()));
    assert_eq!(
        lines[3],
        serde_json::json!({"type": "input", "event": "key", "key": "enter", "down": true})
    );
    assert_eq!(
        lines[4],
        serde_json::json!({"type": "input", "event": "key", "key": "a", "down": true})
    );
    assert_eq!(
        lines[5],
        serde_json::json!({"type": "input", "event": "key", "key": "a", "down": false})
    );
    assert_eq!(
        lines[6],
        serde_json::json!({"type": "input", "event": "text", "text": "hei"})
    );
    let pid = viewer.child_pid();
    viewer.children.push(pid);
    viewer.end();
}

#[test]
fn the_program_is_asked_for_the_size_of_the_viewer_once_it_was_stable() {
    let mut viewer = Viewer::start(&[
        "--width",
        "128",
        "--height",
        "64",
        "--rate",
        "20",
        "--old-size-pictures",
        "1",
        "--report",
        "{folder}/report.json",
    ]);
    viewer.wait_for_picture();
    // A size that changes again within the wait is one request, for the size that
    // the drag ended at.
    viewer.session.set_view_size(Some([64, 32]));
    std::thread::sleep(VIEW_DEBOUNCE / 3);
    viewer.session.set_view_size(Some([64, 32]));
    viewer.wait_for("a picture of the size that the viewer draws at", |viewer| {
        viewer.session.latest_full().map(|frame| frame.size()) == Some([64, 32])
    });
    // A request above the size of the source asks for the size of the source: the
    // program captures 128 by 64 and cannot send more.
    viewer.session.set_view_size(Some([4096, 4096]));
    viewer.wait_for("a size that the source can send", |viewer| {
        viewer.session.latest_full().map(|frame| frame.size()) == Some([128, 64])
    });
    let pid = viewer.child_pid();
    viewer.children.push(pid);
    viewer.end();
}

#[test]
fn a_hidden_viewer_is_told_so_and_still_reads_the_counters() {
    let mut viewer = Viewer::start(&[
        "--width",
        "64",
        "--height",
        "32",
        "--rate",
        "50",
        "--report",
        "{folder}/report.json",
    ]);
    viewer.wait_for_picture();
    viewer.session.set_visible(false);
    // The program stops its pictures and keeps its counters, so an inspection of
    // the panel stays live while the frame draws nothing.
    let deadline = Instant::now() + PATIENCE;
    let mut counters = false;
    while !counters {
        counters = viewer.session.take_updates().stats.is_some();
        assert!(
            Instant::now() < deadline,
            "the counters of a hidden viewer come each second"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    viewer.session.set_visible(true);
    let before = viewer.session.stream_evidence().sequence;
    viewer.wait_for("a picture after the viewer shows again", |viewer| {
        viewer.session.stream_evidence().sequence > before
    });
    let pid = viewer.child_pid();
    viewer.children.push(pid);
    viewer.end();
}

/// The variables of the helper of the test below: its folder, the stand-in and the
/// file that the stand-in reports its process id into.
const HELPER_FOLDER: &str = "HZ_FRAME_SOURCE_HELPER_FOLDER";
const HELPER_REPORT: &str = "HZ_FRAME_SOURCE_HELPER_REPORT";

/// The name of the helper test that a test starts in a process of its own.
const HELPER_TEST: &str = "a_helper_that_shows_a_frame_source_waits_for_its_parent";

/// The helper of the test below: this process owns a viewer and runs until its
/// parent kills it.
///
/// A window that a kill ends runs no cleanup, so only a process of its own shows that
/// the program of a viewer ends with it. A run of the suite alone finds no variable
/// of the helper and answers at once.
#[test]
fn a_helper_that_shows_a_frame_source_waits_for_its_parent() {
    let (Some(folder), Some(report)) = (std::env::var_os(HELPER_FOLDER), std::env::var_os(HELPER_REPORT)) else {
        return;
    };
    let folder = PathBuf::from(folder);
    let report = PathBuf::from(report);
    let (wakes, _woken) = mpsc::channel();
    let config = Config {
        name: "frame_source_standin".into(),
        program: PathBuf::from("python3"),
        arguments: vec![
            OsString::from("-B"),
            standin().into_os_string(),
            OsString::from("--rate"),
            OsString::from("50"),
            OsString::from("--width"),
            OsString::from("32"),
            OsString::from("--height"),
            OsString::from("16"),
            OsString::from("--report"),
            report.clone().into_os_string(),
        ],
    };
    let session = Session::start(config, move |_wake| {
        let _ = wakes.send(());
    })
    .expect("a session of the helper");
    let deadline = Instant::now() + PATIENCE;
    while !report.exists() {
        assert!(Instant::now() < deadline, "the program of the helper never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = folder;
    // The parent kills this process now, so no function of this process ends the
    // program, and only the handles that this process holds end it.
    let _session = session;
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn the_program_ends_when_the_window_that_shows_it_is_killed() {
    let folder = tempfile::tempdir().expect("a folder of a test");
    let report = folder.path().join("report.json");
    let mut helper = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", HELPER_TEST, "--nocapture", "--test-threads", "1"])
        .env(HELPER_FOLDER, folder.path())
        .env(HELPER_REPORT, &report)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the helper of the test");
    let deadline = Instant::now() + PATIENCE;
    while !report.exists() {
        assert!(Instant::now() < deadline, "the program of the helper never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let text = std::fs::read_to_string(&report).expect("the report of the stand-in");
    let value: serde_json::Value = serde_json::from_str(text.trim()).expect("one JSON report");
    let pid = u32::try_from(value["pid"].as_u64().expect("a process id")).expect("a process id");
    assert!(alive(pid), "the program of the helper runs");
    let _ = helper.kill();
    let _ = helper.wait();
    wait_until_gone(pid);
}

#[test]
fn a_file_that_cannot_run_is_one_sentence() {
    // A line of the settings is split into arguments and never given to a shell, so
    // a file that is not a program is one sentence and no shell error.
    let folder = tempfile::tempdir().expect("a folder of a test");
    let script = folder.path().join("frame-tool");
    std::fs::write(&script, b"#!/bin/sh\nexit 0\n").expect("a file of a test");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).expect("a mode of a test");
    let (wakes, _woken) = mpsc::channel();
    let config = Config {
        name: "a file that is not a program".into(),
        program: script,
        arguments: Vec::new(),
    };
    let session = Session::start(config, move |_wake| {
        let _ = wakes.send(());
    })
    .expect("a session");
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = session.observation().status
            && status != Status::Connecting
        {
            break status;
        }
        assert!(Instant::now() < deadline, "a file that cannot run says so");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(status, Status::Disconnected { .. }), "{status:?}");
}
