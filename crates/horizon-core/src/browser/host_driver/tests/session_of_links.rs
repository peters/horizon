//! The links of one session: a link that vanished without a goodbye, the page
//! that comes back to the next link, the silence that ends a link, and the browser
//! that a session keeps with no link.

use std::io::Write;
use std::time::{Duration, Instant};

use horizon_browser::BrowserCommand;
use horizon_browser::host_driver::{HostDriverPageState, HostDriverReply, HostDriverRequest};

use super::super::{HostDriverOptions, relay_streams};
use super::fixtures::{Collected, SessionOfTest, WAIT, hello, lock, never_ends, signals, store};

#[test]
fn a_new_link_takes_the_page_over_from_a_link_that_vanished_without_a_goodbye() {
    // The first link of a panel whose other end is gone: its socket stays open,
    // because the other side of `ssh` holds it, and it says nothing.
    let session = SessionOfTest::start("vanished", Duration::from_millis(100), Duration::from_secs(60));
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);
    store(&session.slot, 4, 2, 10);
    let key = first.wait_for("frame", WAIT);
    let HostDriverReply::Frame(key) = key else {
        panic!("a frame of the first link");
    };
    assert!(key.key, "the first picture of a link is a key frame");

    // The link of that panel says nothing for longer than the take-over bound,
    // which is what a link whose other end vanished looks like.
    std::thread::sleep(Duration::from_millis(250));
    let mut second = session.connect();
    second.send(&hello(60));
    let welcome = second.welcome(WAIT);
    assert_eq!(
        welcome.page,
        HostDriverPageState::Same,
        "the page of the link before is the page of this link, so typed text is still there"
    );
    let HostDriverReply::Frame(frame) = second.wait_for("frame", WAIT) else {
        panic!("the picture of the page reaches the link that came back");
    };
    assert!(
        frame.key,
        "a link that attaches to a session has no picture of its own, so the first one is a key frame"
    );
    assert_eq!(
        frame.seq, key.seq,
        "the newest picture of the page is the one that travels"
    );
    assert!(
        first.closed(WAIT),
        "the link that vanished ends, so its process ends with it"
    );

    let code = session.close(&mut second);
    assert_eq!(code, 0);
}

#[test]
fn a_link_that_says_nothing_for_the_bound_of_the_session_is_closed() {
    // A link whose other end is gone keeps its socket open, so its silence is
    // the only end that the session can see. Closing it ends the process of the
    // link, and the session keeps the page for the next one.
    let session = SessionOfTest::start("silent", Duration::from_millis(50), Duration::from_millis(300));
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);
    assert!(
        first.closed(Duration::from_secs(2)),
        "a link that says nothing for the bound of the session is closed"
    );
    let mut second = session.connect();
    second.send(&hello(60));
    assert_eq!(second.welcome(WAIT).page, HostDriverPageState::Same);
    let code = session.close(&mut second);
    assert_eq!(code, 0);
}

#[test]
fn a_second_link_of_a_live_panel_reads_why_the_page_is_not_free() {
    let session = SessionOfTest::start("busy", Duration::from_millis(100), Duration::from_secs(60));
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);

    // The first link is the link of a panel: it says something four times a
    // second, so it is never silent long enough to be taken over.
    first.send(&signals());
    let mut second = session.connect();
    second.send(&hello(60));
    let HostDriverReply::Refusal { code, message, fatal } = second.wait_for("refusal", WAIT) else {
        panic!("a second link of a profile that is in use reads why it waits");
    };
    assert_eq!(code, "link_busy");
    assert!(fatal, "a second panel of one profile needs a person");
    assert!(
        message.contains("another link") && message.contains("Retry"),
        "the words say what to close and what to do: {message}"
    );

    // The link of the panel keeps the page.
    store(&session.slot, 4, 2, 20);
    let _ = first.wait_for("frame", WAIT);
    let code = session.close(&mut first);
    assert_eq!(code, 0);
}

#[test]
fn a_request_that_follows_the_welcome_at_once_reaches_the_page() {
    let session = SessionOfTest::start("at-once", Duration::from_millis(100), Duration::from_secs(60));
    let mut wire = session.connect();
    // One write, as a panel that answers the welcome at once does: the greeting,
    // the size of the panel and the page that it wants.
    let mut batch = Vec::new();
    for request in [
        hello(60),
        HostDriverRequest::Viewport {
            width: 1000,
            height: 700,
        },
        HostDriverRequest::Navigate {
            url: "https://example.test/".to_string(),
        },
    ] {
        batch.extend(horizon_browser::host_driver::encode_line(&request).expect("one line"));
    }
    wire.stream.write_all(&batch).expect("the requests of the link");
    wire.stream.flush().expect("the requests of the link");
    let _ = wire.welcome(WAIT);
    let deadline = Instant::now() + WAIT;
    loop {
        let commands = lock(&session.commands).clone();
        let size = commands.iter().any(|command| {
            matches!(
                command,
                BrowserCommand::SetViewport {
                    width: 1000,
                    height: 700
                }
            )
        });
        let navigated = commands
            .iter()
            .any(|command| matches!(command, BrowserCommand::Navigate(url) if url == "https://example.test/"));
        if size && navigated {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "both requests that follow the welcome reach the page: {commands:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let code = session.close(&mut wire);
    assert_eq!(code, 0);
}

#[test]
fn a_link_process_ends_when_its_session_ends_though_its_input_never_ends() {
    // A link of a person whose computer is gone: the other side of `ssh` never
    // closes its input, so the end of the link is the end of its session.
    let (session_end, link_end) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    let words = Collected::default();
    let relayed = words.clone();
    let handle = std::thread::spawn(move || relay_streams(never_ends(), relayed, link_end));
    let mut writer = session_end;
    writer
        .write_all(b"{\"type\":\"closed\"}\n")
        .expect("the session writes one line");
    writer.flush().expect("the session writes one line");
    let deadline = Instant::now() + WAIT;
    while lock(&words.0).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        String::from_utf8_lossy(&lock(&words.0)).trim(),
        "{\"type\":\"closed\"}",
        "the words of the session reach this computer"
    );
    // The session ends the link, and the link process ends with it.
    let _ = writer.shutdown(std::net::Shutdown::Both);
    assert!(
        handle.join().is_ok(),
        "the link process of a session that ended does not wait on an input that never ends"
    );
}

#[test]
fn a_session_with_no_link_keeps_its_browser_until_the_idle_bound_stops_it() {
    // The person closed the laptop for the night: the session and its page stay,
    // because a panel that comes back attaches to the page that it had.
    let session = SessionOfTest::start("overnight", Duration::from_millis(50), Duration::from_millis(100));
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);
    store(&session.slot, 4, 2, 30);
    let _ = first.wait_for("frame", WAIT);
    // The link goes away without a goodbye, which leaves the session with no
    // link and its browser running.
    assert!(first.closed(Duration::from_secs(2)));
    std::thread::sleep(Duration::from_millis(500));
    let mut second = session.connect();
    second.send(&hello(60));
    assert_eq!(
        second.welcome(WAIT).page,
        HostDriverPageState::Same,
        "no link is not the end of a session: the page is here in the morning"
    );
    let code = session.close(&mut second);
    assert_eq!(code, 0);
}

#[test]
fn a_session_stops_its_browser_at_the_idle_bound_of_its_entry() {
    let mut session = SessionOfTest::start_with(
        "idle-bound",
        Duration::from_millis(50),
        Duration::from_millis(100),
        Some(Duration::from_millis(300)),
    );
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);
    assert!(first.closed(Duration::from_secs(2)), "the session closes a silent link");
    let code = session
        .handle
        .take()
        .expect("a session thread")
        .join()
        .expect("the session ends at its idle bound");
    assert_eq!(code, 0, "a session that reached its idle bound stops its browser");
}

#[test]
fn the_idle_bound_of_a_host_entry_is_minutes_and_off_without_it() {
    let options = HostDriverOptions::parse([]).expect("the default of the mode");
    assert_eq!(
        options.idle_stop(),
        None,
        "a session keeps its browser unless the entry of the host names a bound"
    );
    let bounded = HostDriverOptions::parse(
        ["--idle-stop", "120"]
            .map(std::ffi::OsString::from)
            .into_iter()
            .collect::<Vec<_>>(),
    )
    .expect("a host with an idle bound");
    assert_eq!(bounded.idle_stop(), Some(Duration::from_secs(120 * 60)));
    assert!(bounded.session_arguments().contains("--idle-stop 120"));
    for arguments in [
        &["--idle-stop", "0"][..],
        &["--idle-stop", "10081"],
        &["--idle-stop", "later"],
        &["--idle-stop"],
        &["--idle-stop", "5", "--idle-stop", "6"],
    ] {
        assert!(
            HostDriverOptions::parse(arguments.iter().map(std::ffi::OsString::from)).is_err(),
            "{arguments:?} must be refused"
        );
    }
}
