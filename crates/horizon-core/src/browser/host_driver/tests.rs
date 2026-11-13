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
pub(super) struct FakePage {
    pub(super) commands: Arc<Mutex<Vec<BrowserCommand>>>,
    pub(super) events: Arc<Mutex<VecDeque<BrowserEvent>>>,
    pub(super) accepts: bool,
}

impl FakePage {
    /// A page that accepts every command of the wire and reports nothing.
    pub(super) fn silent() -> Self {
        Self {
            commands: Arc::default(),
            events: Arc::default(),
            accepts: true,
        }
    }
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
            outcomes.push(serve_link(&mut session, &mut link, "mail", HostDriverEngine::Firefox, None).outcome);
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

/// The far end of one link of a session of a test: the socket of the session,
/// and the messages that the session wrote on it.
struct Wire {
    stream: std::os::unix::net::UnixStream,
    incoming: mpsc::Receiver<HostDriverReply>,
    ended: Arc<Mutex<bool>>,
}

impl Wire {
    fn connect(path: &std::path::Path) -> Self {
        let stream = std::os::unix::net::UnixStream::connect(path).expect("the socket of the session");
        let reader = stream.try_clone().expect("a reader of the link");
        let (sender, incoming) = mpsc::channel();
        let ended = Arc::new(Mutex::new(false));
        let closed = Arc::clone(&ended);
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(reader);
            loop {
                let next = horizon_browser::host_driver::read_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES);
                let reply = match next {
                    Ok(Some(line)) => horizon_browser::host_driver::decode_reply(&line),
                    Ok(None) | Err(_) => break,
                };
                match reply {
                    Ok(reply) => {
                        if sender.send(reply).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            *lock(&closed) = true;
        });
        Self {
            stream,
            incoming,
            ended,
        }
    }

    fn send(&mut self, request: &HostDriverRequest) {
        let line = horizon_browser::host_driver::encode_line(request).expect("one line");
        self.stream.write_all(&line).expect("the session takes a request");
        self.stream.flush().expect("the session takes a request");
    }

    /// The next message of the session, and `None` when none came inside the
    /// bound or the session closed the link.
    fn next(&self, wait: Duration) -> Option<HostDriverReply> {
        self.incoming.recv_timeout(wait).ok()
    }

    /// The next message of the session that this test waits for.
    fn wait_for(&self, want: &str, wait: Duration) -> HostDriverReply {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(reply) = self.next(left) else {
                panic!("the session wrote no {want}: {:?}", lock(&self.ended));
            };
            let name = match &reply {
                HostDriverReply::Welcome(_) => "welcome",
                HostDriverReply::Frame(_) => "frame",
                HostDriverReply::Page(_) => "page",
                HostDriverReply::Refusal { .. } => "refusal",
                HostDriverReply::Closed => "end of the session",
                HostDriverReply::Action(_) => "action",
                HostDriverReply::Coordination(_) => "coordination",
                HostDriverReply::HandoffAck { .. } => "handoff acknowledgement",
                HostDriverReply::Audit(_) => "audit record",
            };
            if name == want {
                return reply;
            }
        }
    }

    fn welcome(&self, wait: Duration) -> horizon_browser::host_driver::HostDriverWelcome {
        match self.wait_for("welcome", wait) {
            HostDriverReply::Welcome(welcome) => welcome,
            other => panic!("a welcome, and the session wrote {other:?}"),
        }
    }

    /// Whether the session closed this link inside the bound.
    fn closed(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if *lock(&self.ended) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        *lock(&self.ended)
    }
}

/// A session of a test on a socket of this computer, with the accept loop, the
/// take-over rule and the silence rule of a host.
struct SessionOfTest {
    path: std::path::PathBuf,
    slot: Arc<FrameSlot>,
    commands: Arc<Mutex<Vec<BrowserCommand>>>,
    handle: Option<std::thread::JoinHandle<i32>>,
    _folder: tempfile::TempDir,
}

impl SessionOfTest {
    fn start(profile: &str, take_over: Duration, silent: Duration) -> Self {
        Self::start_with(profile, take_over, silent, None)
    }

    /// A session whose browser stops after `idle_stop` with no link attached.
    fn start_with(profile: &str, take_over: Duration, silent: Duration, idle_stop: Option<Duration>) -> Self {
        let folder = tempfile::tempdir().expect("a folder of a test");
        let path = folder.path().join("session.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("a socket of a test");
        listener.set_nonblocking(true).expect("a polling socket");
        let slot = Arc::new(FrameSlot::new());
        let commands: Arc<Mutex<Vec<BrowserCommand>>> = Arc::default();
        let options = HostDriverOptions::new(profile, HostDriverEngine::Firefox).expect("the options of a test");
        let socket = path.clone();
        let session_slot = Arc::clone(&slot);
        let session_commands = Arc::clone(&commands);
        let handle = std::thread::spawn(move || {
            let start = move |_options: &HostDriverOptions| {
                Ok(HostSession::over_page(
                    Box::new(FakePage {
                        commands: Arc::clone(&session_commands),
                        events: Arc::default(),
                        accepts: true,
                    }),
                    Arc::clone(&session_slot),
                ))
            };
            super::serve_links(
                &listener,
                &options,
                &socket,
                super::LinkRules {
                    take_over,
                    silent,
                    idle_stop,
                },
                start,
            )
        });
        Self {
            path,
            slot,
            commands,
            handle: Some(handle),
            _folder: folder,
        }
    }

    fn connect(&self) -> Wire {
        Wire::connect(&self.path)
    }

    /// Close the session, as the panel of a person does, and answer its exit
    /// code.
    fn close(mut self, wire: &mut Wire) -> i32 {
        wire.send(&HostDriverRequest::Close);
        let _ = wire.wait_for("end of the session", WAIT);
        self.handle
            .take()
            .expect("a session thread")
            .join()
            .expect("the session ends")
    }
}

fn signals() -> HostDriverRequest {
    HostDriverRequest::Signals {
        signals: HostDriverSignals {
            owner: Some("person".to_string()),
            handoff: None,
            actions: Vec::new(),
        },
    }
}

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

/// One stream of a link whose other end never writes and never closes, as the
/// input of a link whose `ssh` side is gone.
fn never_ends() -> std::os::unix::net::UnixStream {
    let (held, link) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    std::mem::forget(held);
    link
}

/// The words of a relay of a test, kept so the test reads what it copied.
#[derive(Clone, Default)]
struct Collected(Arc<Mutex<Vec<u8>>>);

impl Write for Collected {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_link_process_ends_when_its_session_ends_though_its_input_never_ends() {
    // A link of a person whose computer is gone: the other side of `ssh` never
    // closes its input, so the end of the link is the end of its session.
    let (session_end, link_end) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    let words = Collected::default();
    let relayed = words.clone();
    let handle = std::thread::spawn(move || super::relay_streams(never_ends(), relayed, link_end));
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
