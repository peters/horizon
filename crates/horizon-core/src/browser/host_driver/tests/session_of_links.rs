//! The links of one session: a link that vanished without a goodbye, the page
//! that comes back to the next link, the silence that ends a link, and the browser
//! that a session keeps with no link.

use std::io::Write;
use std::time::{Duration, Instant};

use horizon_browser::BrowserCommand;
use horizon_browser::host_driver::{HostDriverPageState, HostDriverReply, HostDriverRequest};

use super::super::pages::{MAX_PAGES, PRIVATE_PAGE_UNLINKED, SessionPages, StartPage};
use super::super::{HostDriverEngine, HostDriverOptions, LinkRules, relay_streams};
use super::fixtures::{Collected, SessionOfTest, WAIT, hello, hello_page, lock, never_ends, signals, store};

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
fn a_link_that_keeps_answering_keeps_its_page() {
    // The rule of a taken-over link is the rule of each page: the page of a panel
    // goes to a new link of that panel when the link that holds it went silent,
    // and never while it still answers. A link of a panel says something four
    // times a second (the signals of its manifest), so it is not taken over.
    let session = SessionOfTest::start("answering", Duration::from_secs(2), Duration::from_secs(60));
    let mut first = session.connect();
    first.send(&hello_page("panel-1", false, 60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);
    first.send(&signals());

    let mut second = session.connect();
    second.send(&hello_page("panel-1", false, 60));
    let HostDriverReply::Refusal { code, .. } = second.wait_for("refusal", WAIT) else {
        panic!("a link that still answers is not taken over");
    };
    assert_eq!(code, "link_busy");

    // The page is still the page of the link that answers.
    first.send(&HostDriverRequest::Navigate {
        url: "https://kept.test/".to_string(),
    });
    let deadline = Instant::now() + WAIT;
    while !session
        .commands_of(0)
        .iter()
        .any(|command| matches!(command, BrowserCommand::Navigate(url) if url == "https://kept.test/"))
    {
        assert!(Instant::now() < deadline, "the link that answers keeps its page");
        std::thread::sleep(Duration::from_millis(5));
    }

    let code = session.close(&mut first);
    assert_eq!(code, 0);
}

#[test]
fn a_second_link_of_the_same_page_reads_why_the_page_is_not_free() {
    // The take-over bound is longer than this test, so the link that holds the
    // page is a link of a panel that still answers: a second link of the same
    // panel waits instead of taking the page of the person.
    let session = SessionOfTest::start("busy", Duration::from_secs(60), Duration::from_secs(60));
    let mut first = session.connect();
    first.send(&hello(60));
    assert_eq!(first.welcome(WAIT).page, HostDriverPageState::New);

    let mut second = session.connect();
    second.send(&hello(60));
    let HostDriverReply::Refusal { code, message, fatal } = second.wait_for("refusal", WAIT) else {
        panic!("a second link of a page that a link drives reads why it waits");
    };
    assert_eq!(code, "link_busy");
    assert!(fatal, "a second link of one page needs a person");
    assert!(
        message.contains("another link") && message.contains("Retry"),
        "the words say what to do: {message}"
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

/// The bound of a private page with no link is ten minutes, and the option of a
/// driver shortens it for a check that cannot wait for the rule.
#[test]
fn the_private_page_bound_is_ten_minutes_and_an_option_shortens_it() {
    let options = HostDriverOptions::parse([]).expect("the default of the mode");
    assert_eq!(
        options.unlinked_private_page(),
        PRIVATE_PAGE_UNLINKED,
        "the session of a browser host closes a private page ten minutes after its link went"
    );
    let short = HostDriverOptions::parse(
        ["--private-unlinked", "20"]
            .map(std::ffi::OsString::from)
            .into_iter()
            .collect::<Vec<_>>(),
    )
    .expect("a driver with a short bound");
    assert_eq!(short.unlinked_private_page(), Duration::from_secs(20));
    assert!(
        short.session_arguments().contains("--private-unlinked 20"),
        "the session process of the profile takes the bound of the link that started it: {}",
        short.session_arguments()
    );
    assert!(
        !options.session_arguments().contains("--private-unlinked"),
        "a session with the bound of the rule writes no option"
    );
    for arguments in [
        &["--private-unlinked", "0"][..],
        &["--private-unlinked", "86401"],
        &["--private-unlinked", "later"],
        &["--private-unlinked"],
        &["--private-unlinked", "5", "--private-unlinked", "6"],
    ] {
        assert!(
            HostDriverOptions::parse(arguments.iter().map(std::ffi::OsString::from)).is_err(),
            "{arguments:?} must be refused"
        );
    }
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
    assert_eq!(bounded.idle_stop(), Some(Duration::from_hours(2)));
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

/// The panels of one profile are pages of one browser: a second panel is a new
/// page of the same profile, not a refusal and not the page of the first panel.
#[test]
fn two_panels_of_one_profile_are_two_pages_of_one_browser() {
    let session = SessionOfTest::start("two-pages", Duration::from_millis(100), Duration::from_secs(60));
    let mut first = session.connect();
    first.send(&hello_page("panel-1", false, 60));
    assert_eq!(
        first.welcome(WAIT).page,
        HostDriverPageState::New,
        "the first panel of a profile opens its page"
    );
    let mut second = session.connect();
    second.send(&hello_page("panel-2", false, 60));
    assert_eq!(
        second.welcome(WAIT).page,
        HostDriverPageState::New,
        "a second panel of the same profile opens another page of the same browser"
    );
    assert_eq!(session.pages(), 2, "one page for each panel of the profile");

    // The page of each panel is driven by its own link alone.
    first.send(&HostDriverRequest::Navigate {
        url: "https://one.test/".to_string(),
    });
    second.send(&HostDriverRequest::Navigate {
        url: "https://two.test/".to_string(),
    });
    let navigated = |commands: &[BrowserCommand], url: &str| {
        commands
            .iter()
            .any(|command| matches!(command, BrowserCommand::Navigate(page) if page == url))
    };
    let deadline = Instant::now() + WAIT;
    while !(navigated(&session.commands_of(0), "https://one.test/")
        && navigated(&session.commands_of(1), "https://two.test/"))
    {
        assert!(
            Instant::now() < deadline,
            "each page takes the request of its own link: {:?} and {:?}",
            session.commands_of(0),
            session.commands_of(1)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !navigated(&session.commands_of(0), "https://two.test/"),
        "the page of the first panel takes no request of the second panel"
    );

    // A panel whose link was lost gets its own page back, and the page of the
    // other panel is not touched. The link of the lost panel said nothing for the
    // take-over bound, which is what a link of a dropped connection looks like.
    drop(first);
    std::thread::sleep(Duration::from_millis(250));
    let mut back = session.connect();
    back.send(&hello_page("panel-1", false, 60));
    assert_eq!(
        back.welcome(WAIT).page,
        HostDriverPageState::Same,
        "the page of the panel that came back is the page that it had"
    );
    assert_eq!(
        session.pages(),
        2,
        "the page of the lost link came back instead of a new page"
    );
    assert!(
        navigated(&session.commands_of(1), "https://two.test/"),
        "the page of the other panel kept its own history"
    );

    // The close of one panel ends its page, and the browser stays for the page of
    // the other panel.
    second.send(&HostDriverRequest::Close);
    let _ = second.wait_for("end of the session", WAIT);
    let mut again = session.connect();
    again.send(&hello_page("panel-2", false, 60));
    assert_eq!(
        again.welcome(WAIT).page,
        HostDriverPageState::New,
        "the page of a panel that closed is gone, so its next panel opens a new one"
    );
    assert_eq!(session.pages(), 3, "one page for each open panel");

    // The last page ends the session, and its browser with it.
    back.send(&HostDriverRequest::Close);
    let _ = back.wait_for("end of the session", WAIT);
    let code = session.close(&mut again);
    assert_eq!(code, 0, "the last page of a session ends the session");
}

/// A private panel of a browser host is a page of the same browser that keeps
/// nothing of the profile.
#[test]
fn a_private_page_of_a_profile_is_a_page_that_keeps_nothing() {
    let session = SessionOfTest::start("private-page", Duration::from_millis(100), Duration::from_secs(60));
    let mut shared = session.connect();
    shared.send(&hello_page("panel-1", false, 60));
    let _ = shared.welcome(WAIT);
    let mut private = session.connect();
    private.send(&hello_page("panel-2", true, 60));
    assert_eq!(private.welcome(WAIT).page, HostDriverPageState::New);
    assert_eq!(
        session.private_of(0),
        Some(false),
        "a panel of the profile is a page of the profile"
    );
    assert_eq!(
        session.private_of(1),
        Some(true),
        "the page of a private panel is made in a context that keeps nothing"
    );
    assert_eq!(session.pages(), 2, "both pages are pages of one session of the profile");
    // A private page keeps nothing, so its panel never resumes a page.
    private.send(&HostDriverRequest::Close);
    let _ = private.wait_for("end of the session", WAIT);
    let mut again = session.connect();
    again.send(&hello_page("panel-2", true, 60));
    assert_eq!(
        again.welcome(WAIT).page,
        HostDriverPageState::New,
        "a private panel that comes back opens a page that keeps nothing again"
    );
    assert_eq!(session.pages(), 3, "a fresh page for each private panel");
    shared.send(&HostDriverRequest::Close);
    let _ = shared.wait_for("end of the session", WAIT);
    let code = session.close(&mut again);
    assert_eq!(code, 0);
}

/// The pages of one session are bounded, and the next panel reads the number and
/// the way out.
#[test]
fn a_session_refuses_a_page_beyond_the_bound_of_its_pages() {
    let session = SessionOfTest::start("many-pages", Duration::from_millis(100), Duration::from_secs(600));
    for index in 0..MAX_PAGES {
        let mut link = session.connect();
        link.send(&hello_page(&format!("panel-{index}"), false, 60));
        assert_eq!(
            link.welcome(WAIT).page,
            HostDriverPageState::New,
            "the page {index} of the profile"
        );
        // The link of that page goes on serving it, so the next link names
        // another panel.
        std::mem::forget(link);
    }
    let mut refused = session.connect();
    refused.send(&hello_page("panel-too-many", false, 60));
    let HostDriverReply::Refusal { code, message, fatal } = refused.wait_for("refusal", WAIT) else {
        panic!("a page beyond the bound of a session reads why");
    };
    assert_eq!(code, "page_limit_reached");
    assert!(fatal, "a person must close a panel of that profile");
    assert!(
        message.contains(&MAX_PAGES.to_string()) && message.contains("another profile"),
        "the words name the bound and the way out: {message}"
    );
    assert_eq!(session.pages(), MAX_PAGES, "no page beyond the bound was started");
}

/// A private page of a session that lost its link goes after the bound of the
/// session, so its context leaves the browser of the host. A shared page of the
/// same session stays: a person signed in there.
#[test]
fn a_private_page_with_no_link_goes_after_the_bound_and_a_shared_page_stays() {
    let session = SessionOfTest::start_with_unlinked(
        "private-bound",
        Duration::from_millis(50),
        Duration::from_millis(100),
        None,
        Duration::from_millis(300),
    );
    let mut private = session.connect();
    private.send(&hello_page("private-panel", true, 60));
    assert_eq!(private.welcome(WAIT).page, HostDriverPageState::New);
    let mut shared = session.connect();
    shared.send(&hello_page("shared-panel", false, 60));
    assert_eq!(shared.welcome(WAIT).page, HostDriverPageState::New);

    // Both links go silent, which is what the link of a panel looks like when
    // Horizon stops, so both pages lose their link.
    assert!(
        private.closed(Duration::from_secs(2)),
        "the session closes a silent link"
    );
    assert!(
        shared.closed(Duration::from_secs(2)),
        "the session closes a silent link"
    );
    std::thread::sleep(Duration::from_millis(700));

    // The private page went with its context, so the link of that panel starts a
    // page.
    let mut private_again = session.connect();
    private_again.send(&hello_page("private-panel", true, 60));
    assert_eq!(
        private_again.welcome(WAIT).page,
        HostDriverPageState::New,
        "a private page with no link for the bound is closed, and its context goes with it"
    );
    // The shared page stays, so the panel that comes back finds its page.
    let mut shared_again = session.connect();
    shared_again.send(&hello_page("shared-panel", false, 60));
    assert_eq!(
        shared_again.welcome(WAIT).page,
        HostDriverPageState::Same,
        "a shared page keeps the rule of today: it waits for the link of its panel"
    );
    let code = session.close(&mut shared_again);
    assert_eq!(code, 0);
}

/// The bound of a private page with no link reads the clock of its caller, so a
/// test judges the ten minutes of the rule without waiting for them.
#[test]
fn the_bound_of_a_private_page_with_no_link_is_one_constant() {
    assert_eq!(
        PRIVATE_PAGE_UNLINKED,
        Duration::from_secs(600),
        "a private page of a browser host goes after ten minutes with no link"
    );
    let options = HostDriverOptions::new("private-clock", HostDriverEngine::Firefox).expect("the options of a test");
    let rules = LinkRules {
        take_over: Duration::from_millis(50),
        silent: Duration::from_secs(60),
        idle_stop: None,
        unlinked_private_page: PRIVATE_PAGE_UNLINKED,
    };
    let start = StartPage::new(|_options, _group, _private| {
        Ok(super::super::session::HostSession::over_page(
            Box::new(super::fixtures::FakePage::silent()),
            std::sync::Arc::new(horizon_browser::FrameSlot::new()),
        ))
    });
    let pages = SessionPages::new(options, rules, start);
    let private = pages.acquire("private-panel", true).expect("a private page");
    let shared = pages.acquire("shared-panel", false).expect("a shared page");
    private.hold.release();
    shared.hold.release();
    let released_at = Instant::now();

    assert!(
        !pages.close_unlinked_private_pages(released_at + Duration::from_secs(599)),
        "the session closes no page before the bound"
    );
    assert!(pages.keeps_page("private-panel"), "the private page waits for a link");
    assert!(
        !pages.close_unlinked_private_pages(released_at + PRIVATE_PAGE_UNLINKED),
        "a shared page keeps the rule of today, so the session has a page left"
    );
    assert!(
        !pages.keeps_page("private-panel"),
        "the private page and its context go at the bound"
    );
    assert!(pages.keeps_page("shared-panel"), "the shared page stays");
}

/// A private page that a link still drives stays, however long it runs.
#[test]
fn a_private_page_with_a_link_keeps_its_context() {
    let options = HostDriverOptions::new("private-link", HostDriverEngine::Firefox).expect("the options of a test");
    let rules = LinkRules {
        take_over: Duration::from_millis(50),
        silent: Duration::from_secs(60),
        idle_stop: None,
        unlinked_private_page: PRIVATE_PAGE_UNLINKED,
    };
    let start = StartPage::new(|_options, _group, _private| {
        Ok(super::super::session::HostSession::over_page(
            Box::new(super::fixtures::FakePage::silent()),
            std::sync::Arc::new(horizon_browser::FrameSlot::new()),
        ))
    });
    let pages = SessionPages::new(options, rules, start);
    let _private = pages.acquire("private-panel", true).expect("a private page");
    assert!(
        !pages.close_unlinked_private_pages(Instant::now() + Duration::from_hours(6)),
        "a page that a link drives keeps its context"
    );
    assert!(pages.keeps_page("private-panel"));
}
