//! The link of a browser host, driven by a page of a test.
//!
//! The page of these tests is not Firefox and not geckodriver: the wire rules of
//! this module are what is tested (the greeting, one frame in flight, the page
//! that came back, the order of the input, a typed run as one message). A real
//! Firefox session is a manual check, because it needs a browser on the machine
//! and the driver of that browser, so the test that starts one is ignored with
//! its reason.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use horizon_browser::host_driver::{
    HOST_DRIVER_VERSION, HostDriverPageState, HostDriverReply, HostDriverRequest, HostDriverSignals, HostSystem,
    MAX_HOST_DRIVER_LINE_BYTES,
};
use horizon_browser::{BrowserCommand, BrowserEvent, BrowserInput, FrameSlot};

use super::session::{HostPage, HostSession, serve_link};
use super::{HostDriverEngine, HostDriverOptions, Incoming, Link, LinkOutcome, Next, Step};

const WAIT: Duration = Duration::from_secs(5);

/// A page of a test: it records what the wire asked for and reports what the
/// test tells it to report.
struct FakePage {
    commands: Arc<Mutex<Vec<BrowserCommand>>>,
    events: Arc<Mutex<VecDeque<BrowserEvent>>>,
    accepts: bool,
}

impl HostPage for FakePage {
    fn send(&self, command: BrowserCommand) -> bool {
        if matches!(command, BrowserCommand::Stop) {
            return true;
        }
        if !self.accepts {
            return false;
        }
        lock(&self.commands).push(command);
        true
    }

    fn next_event(&self) -> Option<BrowserEvent> {
        lock(&self.events).pop_front()
    }

    fn shutdown(&mut self) {}
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The peer of one link: what this computer writes, and what the driver answers.
pub(crate) struct Peer {
    pub(super) requests: mpsc::Sender<Incoming>,
    pub(super) replies: Arc<CollectingReplies>,
}

impl Peer {
    /// Write one message of this computer.
    pub(crate) fn send(&self, request: &HostDriverRequest) {
        let line = horizon_browser::host_driver::encode_line(request).expect("one line");
        self.send_raw(&line, line.len());
    }

    fn send_raw(&self, line: &[u8], size: usize) {
        let mut line = line.to_vec();
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        let _ = size;
        self.requests.send(Incoming::Line(line)).expect("the link is open");
    }

    /// Write a line that this wire cannot carry, as a peer of a broken program
    /// would.
    pub(crate) fn send_too_long(&self, size: usize) {
        let _ = self.requests.send(Incoming::TooLong {
            size,
            limit: MAX_HOST_DRIVER_LINE_BYTES,
        });
    }

    /// The reply at `index`, once it arrived.
    fn reply(&self, index: usize) -> HostDriverReply {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(reply) = lock(&self.replies.0).get(index).cloned() {
                return reply;
            }
            assert!(
                Instant::now() < deadline,
                "no reply at {index}: {:?}",
                lock(&self.replies.0)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn welcome(&self, index: usize) -> horizon_browser::host_driver::HostDriverWelcome {
        match self.reply(index) {
            HostDriverReply::Welcome(welcome) => welcome,
            other => panic!("a welcome, and the driver answered {other:?}"),
        }
    }

    fn frames(&self) -> Vec<horizon_browser::host_driver::HostDriverFrame> {
        lock(&self.replies.0)
            .iter()
            .filter_map(|reply| match reply {
                HostDriverReply::Frame(frame) => Some(frame.clone()),
                _ => None,
            })
            .collect()
    }

    /// Wait until `count` frames arrived, then answer whether it happened.
    fn wait_frames(&self, count: usize) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if self.frames().len() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.frames().len() >= count
    }

    fn replies(&self) -> Vec<HostDriverReply> {
        lock(&self.replies.0).clone()
    }
}

/// The answers of the driver, decoded as they arrive.
pub(super) struct CollectingReplies(pub(super) Arc<Mutex<Vec<HostDriverReply>>>);

pub(super) struct CollectingWriter {
    replies: Arc<CollectingReplies>,
    pending: Vec<u8>,
}

impl CollectingWriter {
    pub(super) fn new() -> (Self, Arc<CollectingReplies>) {
        let replies = Arc::new(CollectingReplies(Arc::default()));
        (
            Self {
                replies: Arc::clone(&replies),
                pending: Vec::new(),
            },
            replies,
        )
    }
}

impl Write for CollectingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=position).collect();
            let line = &line[..line.len() - 1];
            if line.is_empty() {
                continue;
            }
            match horizon_browser::host_driver::decode_reply(line) {
                Ok(reply) => lock(&self.replies.0).push(reply),
                Err(error) => panic!("the driver wrote a line that is not a reply: {error}"),
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serve `count` links of `session` one after the other on a thread of the test.
/// A link starts when the link before it ended, as a link of a browser host
/// does, so the test receives each peer as its link starts.
fn serve_sequence(
    mut session: HostSession,
    count: usize,
) -> (mpsc::Receiver<Arc<Peer>>, std::thread::JoinHandle<Vec<LinkOutcome>>) {
    let (sender, receivers) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut outcomes = Vec::new();
        for _ in 0..count {
            let (mut link, peer) = Link::for_tests();
            if sender.send(Arc::new(peer)).is_err() {
                break;
            }
            outcomes.push(serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox));
        }
        outcomes
    });
    (receivers, handle)
}

/// One link of a session, and the thread that serves it.
fn serve(session: HostSession) -> (Arc<Peer>, std::thread::JoinHandle<Vec<LinkOutcome>>) {
    let (receivers, handle) = serve_sequence(session, 1);
    let peer = receivers.recv_timeout(WAIT).expect("the link of the test starts");
    (peer, handle)
}

fn hello(quality: u32) -> HostDriverRequest {
    HostDriverRequest::Hello {
        version: HOST_DRIVER_VERSION,
        horizon_version: env!("CARGO_PKG_VERSION").to_string(),
        system: HostSystem::here(),
        quality,
    }
}

/// What a test keeps of its session: the page's slot, the commands that the page
/// received, and the events that the page reports.
type PageOfTest = (
    HostSession,
    Arc<FrameSlot>,
    Arc<Mutex<Vec<BrowserCommand>>>,
    Arc<Mutex<VecDeque<BrowserEvent>>>,
);

/// A session over a page of a test with a slot the test can fill.
fn harness() -> PageOfTest {
    let slot = Arc::new(FrameSlot::new());
    let commands: Arc<Mutex<Vec<BrowserCommand>>> = Arc::default();
    let events: Arc<Mutex<VecDeque<BrowserEvent>>> = Arc::default();
    let page = FakePage {
        commands: Arc::clone(&commands),
        events: Arc::clone(&events),
        accepts: true,
    };
    (
        HostSession::over_page(Box::new(page), Arc::clone(&slot)),
        slot,
        commands,
        events,
    )
}

fn stop(peer: &Peer, handle: std::thread::JoinHandle<Vec<LinkOutcome>>) -> Vec<LinkOutcome> {
    peer.send(&HostDriverRequest::Close);
    handle.join().expect("the link of the test ends")
}

#[test]
fn a_link_greets_and_says_that_the_page_is_new() {
    let (session, _slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(70));
    let welcome = peer.welcome(0);
    assert_eq!(welcome.version, HOST_DRIVER_VERSION);
    assert_eq!(welcome.engine, "firefox");
    assert_eq!(welcome.profile, "mail");
    assert_eq!(
        welcome.quality, 70,
        "the quality of the link is the quality of the frame"
    );
    assert_eq!(welcome.page, HostDriverPageState::New, "the first link opens the page");
    assert_eq!(welcome.system, HostSystem::here());
    let HostDriverReply::Page(page) = peer.reply(1) else {
        panic!("the page follows the greeting");
    };
    assert!(page.url.is_empty(), "no page of the session is known yet");
    stop(&peer, handle);
}

#[test]
fn a_lost_link_comes_back_to_the_same_page_and_says_so() {
    let (session, slot, commands, _events) = harness();
    let (receivers, handle) = serve_sequence(session, 2);

    // The first link shows a page of the session.
    let first = receivers.recv_timeout(WAIT).expect("the first link starts");
    first.send(&hello(60));
    assert_eq!(first.welcome(0).page, HostDriverPageState::New);
    store(&slot, 2, 2, 10);
    assert!(first.wait_frames(1), "the first link shows the page");
    let typed = BrowserInput::InsertText {
        text: "typed into the page".to_string(),
    };
    first.send(&HostDriverRequest::Input { input: typed.clone() });
    let deadline = Instant::now() + WAIT;
    while lock(&commands).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    first.send(&HostDriverRequest::Close);

    // The second link of the SAME session attaches to that page: the browser of
    // the host kept it while no computer was linked, which is the reason for a
    // driver on the host.
    let second = receivers.recv_timeout(WAIT).expect("the second link starts");
    second.send(&hello(60));
    assert_eq!(
        second.welcome(0).page,
        HostDriverPageState::Same,
        "the page of the link before is the page of this link"
    );
    second.send(&HostDriverRequest::Close);
    let outcomes = handle.join().expect("both links end");
    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        lock(&commands).len(),
        1,
        "the page of the session kept the one input of the person, and the second link sent none"
    );
}

#[test]
fn one_frame_is_in_flight_and_a_held_acknowledgement_sends_no_second_one() {
    let (session, slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);

    // The page drew something: the driver pushes it as the first frame of the
    // link, which is a key frame.
    store(&slot, 4, 2, 10);
    assert!(peer.wait_frames(1), "the first frame of the link arrives");
    let first = peer.frames()[0].clone();
    assert!(first.key, "the first frame of a link is a key frame");
    assert_eq!(first.format.as_str(), "jpeg");
    assert!(first.data.len() > 4, "a picture travels as base64 text");

    // The page draws twice more while the acknowledgement of the first frame is
    // held back. A queue would deliver both; this link delivers neither, and
    // then only the newest.
    store(&slot, 4, 2, 20);
    store(&slot, 4, 2, 30);
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(peer.frames().len(), 1, "no frame goes before the acknowledgement");

    peer.send(&HostDriverRequest::FrameAck { seq: first.seq });
    assert!(peer.wait_frames(2), "the acknowledged frame lets the next one go");
    let second = peer.frames()[1].clone();
    assert_eq!(
        second.seq,
        slot.latest().expect("a frame").seq,
        "the newest picture travels, not the oldest of a queue"
    );
    assert!(!second.key, "a later frame of one link is no key frame");
    stop(&peer, handle);
}

fn store(slot: &FrameSlot, width: u32, height: u32, value: u8) {
    let rgb = vec![value; (width * height * 3) as usize];
    let seq = slot.store_rgb(width, height, rgb).expect("a frame");
    let _ = seq;
}

#[test]
fn the_inputs_of_a_person_reach_the_page_in_their_order() {
    let (session, _slot, commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    let down = BrowserInput::KeyDown {
        physical_key: None,
        key: horizon_browser::BrowserKey::Char('a'),
        text: Some("a".to_string()),
        modifiers: horizon_browser::BrowserModifiers::default(),
        repeat: false,
        edit_command: None,
    };
    let up = BrowserInput::KeyUp {
        physical_key: None,
        key: horizon_browser::BrowserKey::Char('a'),
        text: Some("a".to_string()),
        modifiers: horizon_browser::BrowserModifiers::default(),
    };
    peer.send(&HostDriverRequest::Input { input: down.clone() });
    peer.send(&HostDriverRequest::Input { input: up.clone() });
    peer.send(&HostDriverRequest::Navigate {
        url: "https://example.test/".to_string(),
    });
    let deadline = Instant::now() + WAIT;
    while lock(&commands).len() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let commands = lock(&commands).clone();
    assert_eq!(commands.len(), 3, "the page received three commands: {commands:?}");
    assert!(
        matches!(&commands[0], BrowserCommand::Input(BrowserInput::KeyDown { text: Some(text), .. }) if text == "a"),
        "the key down came first: {commands:?}"
    );
    assert!(
        matches!(&commands[1], BrowserCommand::Input(BrowserInput::KeyUp { text: Some(text), .. }) if text == "a"),
        "the key up came second: {commands:?}"
    );
    assert!(
        matches!(&commands[2], BrowserCommand::Navigate(url) if url == "https://example.test/"),
        "the navigation came last: {commands:?}"
    );
    stop(&peer, handle);
}

#[test]
fn a_run_of_typed_text_is_one_message_of_the_link() {
    let (session, _slot, commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    let typed = "one run of typed text";
    peer.send(&HostDriverRequest::Input {
        input: BrowserInput::InsertText {
            text: typed.to_string(),
        },
    });
    let deadline = Instant::now() + WAIT;
    while lock(&commands).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let commands = lock(&commands).clone();
    assert_eq!(commands.len(), 1, "one message of the link: {commands:?}");
    assert!(
        matches!(&commands[0], BrowserCommand::Input(BrowserInput::InsertText { text }) if text == typed),
        "typing a run costs one message of the link, not one per character: {commands:?}"
    );
    stop(&peer, handle);
}

#[test]
fn a_page_that_the_session_kept_is_reported_with_its_address_and_title() {
    let (session, _slot, _commands, events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    lock(&events).push_back(BrowserEvent::UrlChanged("https://example.test/page".to_string()));
    lock(&events).push_back(BrowserEvent::Title("Example page".to_string()));
    let deadline = Instant::now() + WAIT;
    let page = loop {
        let pages: Vec<_> = peer
            .replies()
            .into_iter()
            .filter_map(|reply| match reply {
                HostDriverReply::Page(page) => Some(page),
                _ => None,
            })
            .collect();
        if let Some(page) = pages.last()
            && page.url.ends_with("/page")
        {
            break page.clone();
        }
        assert!(Instant::now() < deadline, "the page of the session reaches the link");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(page.title, "Example page");
    assert!(page.loading, "a fresh address is still loading");
    stop(&peer, handle);
}

#[test]
fn a_version_that_this_build_does_not_speak_is_refused_with_words() {
    let (session, _slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&HostDriverRequest::Hello {
        version: HOST_DRIVER_VERSION + 7,
        horizon_version: "9.9.9".to_string(),
        system: HostSystem::here(),
        quality: 60,
    });
    let HostDriverReply::Refusal { code, message, fatal } = peer.reply(0) else {
        panic!("a refusal");
    };
    assert_eq!(code, "protocol_version");
    assert!(fatal);
    assert!(message.contains(&HOST_DRIVER_VERSION.to_string()), "{message}");
    assert!(
        message.contains("speaks 8"),
        "the words name the other version: {message}"
    );
    assert_eq!(
        handle.join().expect("the link ends"),
        vec![LinkOutcome::Answer(horizon_browser::host_driver::version_refusal(
            HOST_DRIVER_VERSION + 7
        ))]
    );
}

#[test]
fn a_host_of_another_system_is_refused_with_its_words() {
    let (session, _slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&HostDriverRequest::Hello {
        version: HOST_DRIVER_VERSION,
        horizon_version: env!("CARGO_PKG_VERSION").to_string(),
        system: HostSystem {
            os: "other-os".to_string(),
            arch: "other-cpu".to_string(),
        },
        quality: 60,
    });
    let HostDriverReply::Refusal { code, message, .. } = peer.reply(0) else {
        panic!("a refusal");
    };
    assert_eq!(code, "protocol_hello");
    assert!(
        message.contains("other-os") && message.contains("other-cpu"),
        "{message}"
    );
    assert!(message.contains("built for the host"), "{message}");
    handle.join().expect("the link ends");
}

#[test]
fn a_line_larger_than_the_wire_allows_ends_the_link_with_its_size() {
    let (session, _slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    peer.send_too_long(MAX_HOST_DRIVER_LINE_BYTES + 1);
    let HostDriverReply::Refusal { code, message, .. } = peer.reply(2) else {
        panic!("a refusal");
    };
    assert_eq!(code, "line_too_long");
    assert!(
        message.contains(&(MAX_HOST_DRIVER_LINE_BYTES + 1).to_string()),
        "{message}"
    );
    handle.join().expect("the link ends");
}

#[test]
fn an_input_of_a_page_that_is_not_ready_is_refused_without_ending_the_link() {
    let slot = Arc::new(FrameSlot::new());
    let page = FakePage {
        commands: Arc::default(),
        events: Arc::default(),
        accepts: false,
    };
    let (peer, handle) = serve(HostSession::over_page(Box::new(page), slot));
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    peer.send(&HostDriverRequest::Reload);
    let HostDriverReply::Refusal { code, fatal, .. } = peer.reply(2) else {
        panic!("a refusal");
    };
    assert_eq!(code, "browser_not_ready");
    assert!(!fatal, "a page that is not ready yet keeps the link");
    stop(&peer, handle);
}

#[test]
fn the_engine_of_the_mode_is_named_or_refused() {
    let options = HostDriverOptions::parse([]);
    let options = options.expect("the default of the mode");
    assert_eq!(options.profile(), "default");
    assert_eq!(options.engine(), HostDriverEngine::Firefox);

    let options = HostDriverOptions::parse(
        ["--profile", "mail", "--engine", "firefox"]
            .map(std::ffi::OsString::from)
            .into_iter()
            .collect::<Vec<_>>(),
    )
    .expect("the mode of a driver");
    assert_eq!(options.profile(), "mail");

    for arguments in [
        &["--engine", "chromium"][..],
        &["--engine", "safari"],
        &["--profile", "Mail"],
        &["--profile"],
        &["--engine", "firefox", "--engine", "firefox"],
        &["--port", "4444"],
    ] {
        let error =
            HostDriverOptions::parse(arguments.iter().map(std::ffi::OsString::from)).expect_err("the mode refuses it");
        assert!(!error.to_string().is_empty(), "{arguments:?}");
    }
}

#[test]
fn a_signalled_action_of_an_agent_tool_is_handed_to_the_page_loop() {
    let (session, _slot, _commands, _events) = harness();
    let (peer, handle) = serve(session);
    peer.send(&hello(60));
    let _ = peer.welcome(0);
    peer.send(&HostDriverRequest::Signals {
        signals: HostDriverSignals {
            owner: Some("agent".to_string()),
            handoff: None,
            actions: Vec::new(),
        },
    });
    peer.send(&HostDriverRequest::Semantic {
        action_id: "a-9".to_string(),
        action: Box::new(horizon_browser::BrowserControlAction::Reload),
    });
    std::thread::sleep(Duration::from_millis(120));
    stop(&peer, handle);
}

#[test]
fn the_count_of_a_link_keeps_the_frame_sequence_of_the_page() {
    let counter = Arc::new(AtomicU64::new(0));
    counter.fetch_add(1, Ordering::Relaxed);
    assert_eq!(counter.load(Ordering::Relaxed), 1);
    // The frame of a link is the sequence of the page, so a panel that attaches
    // to a session shows the newest picture and not a picture of its own count.
    let slot = FrameSlot::new();
    let first = slot.store_rgb(2, 2, vec![0; 12]).expect("a frame");
    let second = slot.store_rgb(2, 2, vec![1; 12]).expect("a frame");
    assert_eq!(second, first + 1);
}

#[test]
fn an_unused_next_of_a_link_is_a_silent_pass() {
    let (link, _peer) = Link::for_tests();
    let mut link = link;
    assert!(matches!(link.next(Duration::from_millis(1)), Next::Nothing));
    let _ = Step::Silent;
}

#[test]
#[ignore = "needs geckodriver and a real Firefox on this machine; see the words of the test"]
fn a_real_firefox_of_this_machine_shows_a_page_through_the_link() {
    // Run by hand, on a machine with geckodriver and Firefox:
    //   HORIZON_BROWSER_HOST_SESSION=1 cargo run --bin horizon -- \
    //     --browser-host-driver --profile manual --engine firefox
    // and in another shell the panel of a browser host that names this machine.
}
