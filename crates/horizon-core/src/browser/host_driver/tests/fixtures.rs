//! The page, the peers and the sessions that the tests of a browser host drive.
//!
//! Nothing here is Firefox: a page records what the wire asked for, a peer writes
//! the messages of this computer and reads the answers of the driver, and a
//! session of a test serves a socket of this computer with the accept loop of a
//! host and the bounds of a test.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use horizon_browser::host_driver::{
    HOST_DRIVER_VERSION, HostDriverReply, HostDriverRequest, HostDriverSignals, HostSystem, MAX_HOST_DRIVER_LINE_BYTES,
};
use horizon_browser::{BrowserCommand, BrowserEvent, FrameSlot};

use super::super::pages::StartPage;
use super::super::session::{HostPage, HostSession, serve_link};
use super::super::{HostDriverEngine, HostDriverOptions, Incoming, Link, LinkOutcome, LinkRules, serve_links};

pub(crate) const WAIT: Duration = Duration::from_secs(5);

/// A page of a test: it records what the wire asked for and reports what the
/// test tells it to report.
pub(crate) struct FakePage {
    pub(crate) commands: Arc<Mutex<Vec<BrowserCommand>>>,
    pub(crate) events: Arc<Mutex<VecDeque<BrowserEvent>>>,
    pub(crate) accepts: bool,
}

impl FakePage {
    /// A page that accepts every command of the wire and reports nothing.
    pub(crate) fn silent() -> Self {
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

pub(crate) fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The peer of one link: what this computer writes, and what the driver answers.
pub(crate) struct Peer {
    pub(crate) requests: mpsc::Sender<Incoming>,
    pub(crate) replies: Arc<CollectingReplies>,
}

impl Peer {
    /// Write one message of this computer.
    pub(crate) fn send(&self, request: &HostDriverRequest) {
        let line = horizon_browser::host_driver::encode_line(request).expect("one line");
        self.send_raw(&line, line.len());
    }

    pub(crate) fn send_raw(&self, line: &[u8], size: usize) {
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
    pub(crate) fn reply(&self, index: usize) -> HostDriverReply {
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

    pub(crate) fn welcome(&self, index: usize) -> horizon_browser::host_driver::HostDriverWelcome {
        match self.reply(index) {
            HostDriverReply::Welcome(welcome) => welcome,
            other => panic!("a welcome, and the driver answered {other:?}"),
        }
    }

    pub(crate) fn frames(&self) -> Vec<horizon_browser::host_driver::HostDriverFrame> {
        lock(&self.replies.0)
            .iter()
            .filter_map(|reply| match reply {
                HostDriverReply::Frame(frame) => Some(frame.clone()),
                _ => None,
            })
            .collect()
    }

    /// Wait until `count` frames arrived, then answer whether it happened.
    pub(crate) fn wait_frames(&self, count: usize) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if self.frames().len() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.frames().len() >= count
    }

    pub(crate) fn replies(&self) -> Vec<HostDriverReply> {
        lock(&self.replies.0).clone()
    }
}

/// The answers of the driver, decoded as they arrive.
pub(crate) struct CollectingReplies(pub(crate) Arc<Mutex<Vec<HostDriverReply>>>);

pub(crate) struct CollectingWriter {
    replies: Arc<CollectingReplies>,
    pending: Vec<u8>,
}

impl CollectingWriter {
    pub(crate) fn new() -> (Self, Arc<CollectingReplies>) {
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
pub(crate) fn serve_sequence(
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
pub(crate) fn serve(session: HostSession) -> (Arc<Peer>, std::thread::JoinHandle<Vec<LinkOutcome>>) {
    let (receivers, handle) = serve_sequence(session, 1);
    let peer = receivers.recv_timeout(WAIT).expect("the link of the test starts");
    (peer, handle)
}

pub(crate) fn hello(quality: u32) -> HostDriverRequest {
    hello_page("page-1", false, quality)
}

/// The greeting of a link that drives `page`.
pub(crate) fn hello_page(page: &str, private: bool, quality: u32) -> HostDriverRequest {
    HostDriverRequest::Hello {
        version: HOST_DRIVER_VERSION,
        horizon_version: env!("CARGO_PKG_VERSION").to_string(),
        system: HostSystem::here(),
        quality,
        page: page.to_string(),
        private,
    }
}

/// What a test keeps of its session: the page's slot, the commands that the page
/// received, and the events that the page reports.
pub(crate) type PageOfTest = (
    HostSession,
    Arc<FrameSlot>,
    Arc<Mutex<Vec<BrowserCommand>>>,
    Arc<Mutex<VecDeque<BrowserEvent>>>,
);

/// A session over a page of a test with a slot the test can fill.
pub(crate) fn harness() -> PageOfTest {
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

pub(crate) fn stop(peer: &Peer, handle: std::thread::JoinHandle<Vec<LinkOutcome>>) -> Vec<LinkOutcome> {
    peer.send(&HostDriverRequest::Close);
    handle.join().expect("the link of the test ends")
}

pub(crate) fn store(slot: &FrameSlot, width: u32, height: u32, value: u8) {
    let rgb = vec![value; (width * height * 3) as usize];
    let seq = slot.store_rgb(width, height, rgb).expect("a frame");
    let _ = seq;
}

/// The far end of one link of a session of a test: the socket of the session,
/// and the messages that the session wrote on it.
pub(crate) struct Wire {
    pub(crate) stream: std::os::unix::net::UnixStream,
    incoming: mpsc::Receiver<HostDriverReply>,
    ended: Arc<Mutex<bool>>,
    /// The kinds of every reply that this link already read, for words of a
    /// failure.
    seen: Arc<Mutex<Vec<String>>>,
}

impl Wire {
    pub(crate) fn connect(path: &std::path::Path) -> Self {
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
            seen: Arc::default(),
        }
    }

    pub(crate) fn send(&mut self, request: &HostDriverRequest) {
        let line = horizon_browser::host_driver::encode_line(request).expect("one line");
        self.stream.write_all(&line).expect("the session takes a request");
        self.stream.flush().expect("the session takes a request");
    }

    /// The next message of the session, and `None` when none came inside the
    /// bound or the session closed the link.
    pub(crate) fn next(&self, wait: Duration) -> Option<HostDriverReply> {
        self.incoming.recv_timeout(wait).ok()
    }

    /// The next message of the session that this test waits for.
    pub(crate) fn wait_for(&self, want: &str, bound: Duration) -> HostDriverReply {
        let deadline = Instant::now() + bound;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(reply) = self.next(left) else {
                panic!(
                    "the session wrote no {want} (ended: {:?}, seen: {:?})",
                    lock(&self.ended),
                    lock(&self.seen)
                );
            };
            let name = match &reply {
                HostDriverReply::Refusal { code, .. } => {
                    lock(&self.seen).push(format!("refusal {code}"));
                    if want == "refusal" {
                        return reply;
                    }
                    "refusal"
                }
                HostDriverReply::Welcome(_) => "welcome",
                HostDriverReply::Frame(_) => "frame",
                HostDriverReply::Page(_) => "page",
                HostDriverReply::Closed => "end of the session",
                HostDriverReply::Action(_) => "action",
                HostDriverReply::Coordination(_) => "coordination",
                HostDriverReply::HandoffAck { .. } => "handoff acknowledgement",
                HostDriverReply::Audit(_) => "audit record",
            };
            lock(&self.seen).push(name.to_string());
            if name == want {
                return reply;
            }
        }
    }

    pub(crate) fn welcome(&self, wait: Duration) -> horizon_browser::host_driver::HostDriverWelcome {
        match self.wait_for("welcome", wait) {
            HostDriverReply::Welcome(welcome) => welcome,
            other => panic!("a welcome, and the session wrote {other:?}"),
        }
    }

    /// Whether the session closed this link inside the bound.
    pub(crate) fn closed(&self, wait: Duration) -> bool {
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

/// One page that a session of a test started, and the facts of its start.
pub(crate) struct StartedPage {
    pub(crate) private: bool,
    pub(crate) commands: Arc<Mutex<Vec<BrowserCommand>>>,
}

/// A session of a test on a socket of this computer, with the accept loop, the
/// take-over rule and the silence rule of a host.
pub(crate) struct SessionOfTest {
    pub(crate) path: std::path::PathBuf,
    /// The slot of the first page that this session started.
    pub(crate) slot: Arc<FrameSlot>,
    /// The commands of the first page that this session started.
    pub(crate) commands: Arc<Mutex<Vec<BrowserCommand>>>,
    /// Every page that this session started, in the order of its links.
    pub(crate) started: Arc<Mutex<Vec<StartedPage>>>,
    pub(crate) handle: Option<std::thread::JoinHandle<i32>>,
    _folder: tempfile::TempDir,
}

impl SessionOfTest {
    pub(crate) fn start(profile: &str, take_over: Duration, silent: Duration) -> Self {
        Self::start_with(profile, take_over, silent, None)
    }

    /// A session whose browser stops after `idle_stop` with no link attached.
    pub(crate) fn start_with(
        profile: &str,
        take_over: Duration,
        silent: Duration,
        idle_stop: Option<Duration>,
    ) -> Self {
        Self::start_with_unlinked(
            profile,
            take_over,
            silent,
            idle_stop,
            crate::browser::host_driver::pages::PRIVATE_PAGE_UNLINKED,
        )
    }

    /// A session whose private page with no link goes after
    /// `unlinked_private_page`.
    pub(crate) fn start_with_unlinked(
        profile: &str,
        take_over: Duration,
        silent: Duration,
        idle_stop: Option<Duration>,
        unlinked_private_page: Duration,
    ) -> Self {
        let folder = tempfile::tempdir().expect("a folder of a test");
        let path = folder.path().join("session.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("a socket of a test");
        listener.set_nonblocking(true).expect("a polling socket");
        let slot = Arc::new(FrameSlot::new());
        let commands: Arc<Mutex<Vec<BrowserCommand>>> = Arc::default();
        let started: Arc<Mutex<Vec<StartedPage>>> = Arc::default();
        let options = HostDriverOptions::new(profile, HostDriverEngine::Firefox).expect("the options of a test");
        let socket = path.clone();
        let session_slot = Arc::clone(&slot);
        let session_commands = Arc::clone(&commands);
        let session_started = Arc::clone(&started);
        let handle = std::thread::spawn(move || {
            let start = StartPage::new(move |_options, _group, private| {
                // The first page of a session of a test fills the slot and the
                // commands of the session, so a test of one page reads that page.
                let first = lock(&session_started).is_empty();
                let page_slot = if first {
                    Arc::clone(&session_slot)
                } else {
                    Arc::new(FrameSlot::new())
                };
                let page_commands = if first {
                    Arc::clone(&session_commands)
                } else {
                    Arc::<Mutex<Vec<BrowserCommand>>>::default()
                };
                lock(&session_started).push(StartedPage {
                    private,
                    commands: Arc::clone(&page_commands),
                });
                Ok(HostSession::over_page(
                    Box::new(FakePage {
                        commands: page_commands,
                        events: Arc::default(),
                        accepts: true,
                    }),
                    page_slot,
                ))
            });
            serve_links(
                &listener,
                &options,
                &socket,
                LinkRules {
                    take_over,
                    silent,
                    idle_stop,
                    unlinked_private_page,
                },
                start,
            )
        });
        Self {
            path,
            slot,
            commands,
            started,
            handle: Some(handle),
            _folder: folder,
        }
    }

    /// The commands of the page that this session started at `index`.
    pub(crate) fn commands_of(&self, index: usize) -> Vec<BrowserCommand> {
        lock(&self.started)
            .get(index)
            .map(|page| lock(&page.commands).clone())
            .unwrap_or_default()
    }

    /// Whether the page that this session started at `index` keeps nothing of the
    /// profile.
    pub(crate) fn private_of(&self, index: usize) -> Option<bool> {
        lock(&self.started).get(index).map(|page| page.private)
    }

    /// How many pages this session started.
    pub(crate) fn pages(&self) -> usize {
        lock(&self.started).len()
    }

    pub(crate) fn connect(&self) -> Wire {
        Wire::connect(&self.path)
    }

    /// Close the session, as the panel of a person does, and answer its exit
    /// code.
    pub(crate) fn close(mut self, wire: &mut Wire) -> i32 {
        wire.send(&HostDriverRequest::Close);
        let _ = wire.wait_for("end of the session", WAIT);
        self.handle
            .take()
            .expect("a session thread")
            .join()
            .expect("the session ends")
    }
}

pub(crate) fn signals() -> HostDriverRequest {
    HostDriverRequest::Signals {
        signals: HostDriverSignals {
            owner: Some("person".to_string()),
            handoff: None,
            actions: Vec::new(),
        },
    }
}

/// One stream of a link whose other end never writes and never closes, as the
/// input of a link whose `ssh` side is gone.
pub(crate) fn never_ends() -> std::os::unix::net::UnixStream {
    let (held, link) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    std::mem::forget(held);
    link
}

/// The words of a relay of a test, kept so the test reads what it copied.
#[derive(Clone, Default)]
pub(crate) struct Collected(pub(crate) Arc<Mutex<Vec<u8>>>);

impl Write for Collected {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
