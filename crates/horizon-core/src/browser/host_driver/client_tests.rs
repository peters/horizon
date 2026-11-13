//! This computer's end of the link, against the driver of the host in this
//! process.
//!
//! The two ends of the link are the real code: the panel's end is
//! [`DriverLink`], and the host's end is the session of `super::session` over a
//! page of a test. Only the transport differs from a real browser host: a socket
//! of this computer stands where `ssh` would stand.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_browser::FrameSlot;
use horizon_browser::host_driver::{HostDriverReply, HostDriverRequest, HostSystem};
use horizon_browser_control::ssh_forward::Ssh;

use super::HostDriverEngine;
use super::client::{DriverLink, HostDriverHost};
use super::session::{HostSession, serve_link};
use super::tests::FakePage;
use crate::browser::AttachedHostFailureKind;

const WAIT: Duration = Duration::from_secs(5);

fn host() -> HostDriverHost {
    HostDriverHost::new("browser-host", "mail", HostDriverEngine::Firefox, None).expect("a host of a test")
}

/// Start the driver of the host in this process over one socket of this
/// computer, and answer this computer's end of the same link.
fn pair(quality: u32) -> (DriverLink, std::thread::JoinHandle<()>) {
    let (ours, theirs) = UnixStream::pair().expect("a socket pair of this computer");
    let handle = std::thread::spawn(move || {
        let slot = Arc::new(FrameSlot::new());
        let session = HostSession::over_page(Box::new(FakePage::silent()), slot);
        let mut session = session;
        let mut link = super::Link::over_socket(&theirs);
        serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox, None);
    });
    let link = DriverLink::over_stream(&ours, &host(), quality).expect("the driver greets this computer");
    (link, handle)
}

/// The next reply of a link, and a panic with words when none arrives.
fn next(link: &mut DriverLink) -> HostDriverReply {
    match link.recv(WAIT) {
        Ok(reply) => reply,
        Err(failure) => panic!("no answer of the driver: {failure:?}"),
    }
}

#[test]
fn the_command_on_the_host_names_the_driver_the_profile_and_the_engine() {
    let host = host();
    assert_eq!(
        host.remote_command(),
        "horizon --browser-host-driver --profile mail --engine firefox"
    );
    let named = HostDriverHost::new(
        "peters@10.0.0.5",
        "work",
        HostDriverEngine::Firefox,
        Some("/opt/horizon/bin/horizon --browser-host-driver"),
    )
    .expect("a host with its own path");
    assert_eq!(
        named.remote_command(),
        "/opt/horizon/bin/horizon --browser-host-driver --profile work --engine firefox",
        "an entry with its own command keeps it, and the profile and engine follow"
    );
    for (target, profile, command) in [
        ("-oProxyCommand=evil", "mail", None),
        ("two hosts", "mail", None),
        ("browser-host", "Mail", None),
        ("browser-host", "mail", Some("horizon; rm -rf /")),
    ] {
        assert!(
            HostDriverHost::new(target, profile, HostDriverEngine::Firefox, command).is_err(),
            "{target} {profile} {command:?} must be refused"
        );
    }
}

#[test]
fn the_greeting_says_the_page_is_new_and_the_link_shows_it() {
    let (link, handle) = pair(60);
    assert_eq!(
        link.welcome().version,
        horizon_browser::host_driver::HOST_DRIVER_VERSION
    );
    assert_eq!(link.welcome().engine, "firefox");
    assert_eq!(link.welcome().quality, 60);
    assert!(
        !link.welcome().page.resumed(),
        "the first link of a session opens the page"
    );
    drop(link);
    let _ = handle.join();
}

#[test]
fn a_pushed_picture_reaches_the_frame_slot_and_is_acknowledged_once() {
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    let slot = Arc::new(FrameSlot::new());
    let host_slot = Arc::clone(&slot);
    let handle = std::thread::spawn(move || {
        let mut session = HostSession::over_page(Box::new(FakePage::silent()), Arc::clone(&host_slot));
        let mut link = super::Link::over_socket(&theirs);
        serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox, None);
    });
    let mut link = DriverLink::over_stream(&ours, &host(), 60).expect("a greeted link");

    // The page of the host drew something.
    let first = store(&slot, 4, 2, 10);
    let HostDriverReply::Frame(frame) = next(&mut link) else {
        panic!("a pushed frame");
    };
    assert_eq!(frame.seq, first);
    assert!(frame.key, "the first frame of a link is a key frame");
    let panel_slot = Arc::new(FrameSlot::new());
    let ack = link.accept_frame(&panel_slot, &frame).expect("an acknowledgement");
    assert_eq!(ack, HostDriverRequest::FrameAck { seq: first });
    let picture = panel_slot.latest().expect("the picture of the panel");
    assert_eq!((picture.width, picture.height), (4, 2));
    assert!(
        link.accept_frame(&panel_slot, &frame).is_none(),
        "one frame is acknowledged once, or the driver would count two pictures"
    );
    link.request(&ack).expect("the acknowledgement of the link");
    drop(link);
    let _ = handle.join();
}

fn store(slot: &FrameSlot, width: u32, height: u32, value: u8) -> u64 {
    slot.store_rgb(width, height, vec![value; (width * height * 3) as usize])
        .expect("a frame of the page")
}

#[test]
fn a_link_that_comes_back_says_that_the_page_is_the_same_one() {
    // One session of the host serves two links in a row, which is what a lost
    // link and its return look like from this computer.
    let (first_of_ours, first_of_theirs) = UnixStream::pair().expect("a socket pair");
    let (second_of_ours, second_of_theirs) = UnixStream::pair().expect("a second socket pair");
    let handle = std::thread::spawn(move || {
        let slot = Arc::new(FrameSlot::new());
        let mut session = HostSession::over_page(Box::new(FakePage::silent()), slot);
        for stream in [first_of_theirs, second_of_theirs] {
            let mut link = super::Link::over_socket(&stream);
            serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox, None);
        }
    });

    let mut first = DriverLink::over_stream(&first_of_ours, &host(), 60).expect("the first link");
    assert!(!first.welcome().page.resumed(), "the first link opens the page");
    first
        .request(&HostDriverRequest::Close)
        .expect("a close of the session");
    drop(first);

    let mut second = DriverLink::over_stream(&second_of_ours, &host(), 60).expect("the second link");
    assert!(
        second.welcome().page.resumed(),
        "the page of the link before is the page of this link, so typed text is still there"
    );
    second
        .request(&HostDriverRequest::Close)
        .expect("a close of the session");
    drop(second);
    handle.join().expect("both links end");
}

#[test]
fn a_host_that_ssh_cannot_reach_keeps_the_words_of_ssh() {
    let directory = tempfile::tempdir().expect("a folder of a test");
    let program = stand_in(directory.path(), "echo 'Host key verification failed.' >&2\nexit 255");
    let ssh = Ssh::default().with_program(program);
    let failure = DriverLink::open(&ssh, &host(), 60).expect_err("a refused host");
    assert_eq!(failure.kind(), AttachedHostFailureKind::Unreachable);
    assert!(
        failure.message().contains("Host key verification failed"),
        "the words of ssh are kept: {}",
        failure.message()
    );
}

#[test]
fn a_host_without_a_horizon_names_the_program_that_is_missing() {
    let directory = tempfile::tempdir().expect("a folder of a test");
    let program = stand_in(directory.path(), "echo 'sh: 1: horizon: not found' >&2\nexit 127");
    let ssh = Ssh::default().with_program(program);
    let failure = DriverLink::open(&ssh, &host(), 60).expect_err("no horizon on the host");
    assert_eq!(failure.kind(), AttachedHostFailureKind::NoDriver);
    assert!(
        failure.message().contains("runs no horizon"),
        "the panel names the program that is missing: {}",
        failure.message()
    );
    assert!(
        failure.detail().is_none() && failure.message().contains("copy the horizon binary"),
        "the sentence says what the person can do: {}",
        failure.message()
    );
}

/// A driver of a test that reads the greeting of this computer and then writes
/// `lines`, so the greeting of this computer reaches a link that answers.
fn answering_driver(theirs: UnixStream, lines: Vec<Vec<u8>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::{BufRead as _, BufReader, Write as _};
        let mut reader = BufReader::new(theirs.try_clone().expect("a reader of the link"));
        let mut greeting = Vec::new();
        let _ = reader.read_until(b'\n', &mut greeting);
        let mut writer = theirs;
        for line in lines {
            let _ = writer.write_all(&line);
        }
        let _ = writer.flush();
        std::thread::sleep(Duration::from_millis(50));
    })
}

/// One `sh` program that stands in for `ssh`, so no test opens a connection.
fn stand_in(directory: &std::path::Path, body: &str) -> String {
    let path = crate::util::stand_in_program_in(directory, "ssh", body).expect("the stand-in ssh");
    path.to_string_lossy().into_owned()
}

#[test]
fn the_words_of_a_driver_that_refused_the_link_reach_the_panel() {
    // The driver of a host that has no Firefox refuses the link with its words,
    // and the panel shows them instead of a link that silently never starts.
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    let handle = answering_driver(
        theirs,
        vec![
            b"{\"type\":\"refusal\",\"code\":\"browser_start\",\"message\":\"the browser of this machine did not start in Firefox: geckodriver was not found on PATH; install geckodriver and Firefox\",\"fatal\":true}\n"
                .to_vec(),
        ],
    );
    let failure = DriverLink::over_stream(&ours, &host(), 60).expect_err("a refusal of the driver");
    assert_eq!(
        failure.kind(),
        AttachedHostFailureKind::Refused,
        "the refusal of the driver: {} / {:?}",
        failure.message(),
        failure.detail()
    );
    assert!(
        failure.message().contains("geckodriver was not found"),
        "the words of the driver reach the panel: {}",
        failure.message()
    );
    let _ = handle.join();
}

#[test]
fn a_driver_of_another_version_or_system_is_refused_with_its_words() {
    for (welcome, kind, needle) in [
        (
            horizon_browser::host_driver::HostDriverWelcome {
                version: horizon_browser::host_driver::HOST_DRIVER_VERSION + 1,
                horizon_version: env!("CARGO_PKG_VERSION").to_string(),
                system: HostSystem::here(),
                engine: "firefox".to_string(),
                profile: "mail".to_string(),
                page: horizon_browser::host_driver::HostDriverPageState::New,
                width: 1280,
                height: 800,
                quality: 60,
            },
            AttachedHostFailureKind::NotThisVersion,
            "speaks version",
        ),
        (
            horizon_browser::host_driver::HostDriverWelcome {
                version: horizon_browser::host_driver::HOST_DRIVER_VERSION,
                horizon_version: "0.0.0".to_string(),
                system: HostSystem::here(),
                engine: "firefox".to_string(),
                profile: "mail".to_string(),
                page: horizon_browser::host_driver::HostDriverPageState::New,
                width: 1280,
                height: 800,
                quality: 60,
            },
            AttachedHostFailureKind::NotThisVersion,
            "is Horizon 0.0.0",
        ),
        (
            horizon_browser::host_driver::HostDriverWelcome {
                version: horizon_browser::host_driver::HOST_DRIVER_VERSION,
                horizon_version: env!("CARGO_PKG_VERSION").to_string(),
                system: HostSystem {
                    os: "other-os".to_string(),
                    arch: "other-cpu".to_string(),
                },
                engine: "firefox".to_string(),
                profile: "mail".to_string(),
                page: horizon_browser::host_driver::HostDriverPageState::New,
                width: 1280,
                height: 800,
                quality: 60,
            },
            AttachedHostFailureKind::OtherSystem,
            "other-os",
        ),
    ] {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        let line = horizon_browser::host_driver::encode_line(&HostDriverReply::Welcome(welcome)).expect("one line");
        let handle = answering_driver(theirs, vec![line]);
        let failure = DriverLink::over_stream(&ours, &host(), 60).expect_err("a refused welcome");
        assert_eq!(failure.kind(), kind, "{}", failure.message());
        assert!(
            failure.message().contains(needle),
            "the panel says {needle}: {}",
            failure.message()
        );
        let _ = handle.join();
    }
}

#[test]
fn a_driver_that_ends_the_link_is_a_lost_link_with_no_words_of_its_own() {
    let (_ours, theirs) = UnixStream::pair().expect("a socket pair");
    let handle = answering_driver(theirs, Vec::new());
    let directory = tempfile::tempdir().expect("a folder of a test");
    let program = stand_in(directory.path(), "exit 0");
    let ssh = Ssh::default().with_program(program);
    let _ = handle;
    let failure = DriverLink::open(&ssh, &host(), 60).expect_err("a link that ended");
    assert_eq!(failure.kind(), AttachedHostFailureKind::Unreachable);
    assert!(
        failure.message().contains("could not be reached"),
        "{}",
        failure.message()
    );
}

#[test]
fn a_request_of_this_computer_reaches_the_page_of_the_host() {
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&commands);
    let handle = std::thread::spawn(move || {
        let page = RecordingPage {
            commands: Arc::clone(&recorded),
        };
        let mut session = HostSession::over_page(Box::new(page), Arc::new(FrameSlot::new()));
        let mut link = super::Link::over_socket(&theirs);
        serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox, None);
    });
    let mut link = DriverLink::over_stream(&ours, &host(), 60).expect("a greeted link");
    link.request(&HostDriverRequest::Navigate {
        url: "https://example.test/".to_string(),
    })
    .expect("a navigation of the link");
    let deadline = Instant::now() + WAIT;
    while commands.lock().map_or(0, |commands| commands.len()) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        commands.lock().is_ok_and(|commands| commands
            .iter()
            .any(|command| matches!(command, horizon_browser::BrowserCommand::Navigate(url) if url == "https://example.test/"))),
        "the page of the host received the navigation"
    );
    drop(link);
    let _ = handle.join();
}

struct RecordingPage {
    commands: Arc<Mutex<Vec<horizon_browser::BrowserCommand>>>,
}

impl super::session::HostPage for RecordingPage {
    fn send(&self, command: horizon_browser::BrowserCommand) -> bool {
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command);
        true
    }

    fn next_event(&self) -> Option<horizon_browser::BrowserEvent> {
        None
    }

    fn shutdown(&mut self) {}
}
