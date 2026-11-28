//! The frames of the session mirror over real links: one server, a publishing
//! computer and a computer that reads its sessions.
//!
//! A test never waits to see that a frame does not come. It sends a frame that must
//! come after it, and reads that this one is the next frame of the link.

use std::{os::unix::net::UnixStream, time::Duration};

use horizon_sync::{
    Frame, Hello, LiveEvent, LivePanel, LiveRequest, MirrorComputer, MirrorEntry, MirrorProgram, MirrorSession,
    PROTOCOL_VERSION, ServerInfo, SocketServer, Store, read_frame, run_server_on_unix_socket, write_frame,
};

/// The time that a test gives one frame to arrive.
const PATIENCE: Duration = Duration::from_secs(60);

const A: &str = "laptop-a";
const B: &str = "laptop-b";

/// One server of a test: its home, its store and its socket.
struct Server {
    directory: tempfile::TempDir,
    store: Store,
    socket: Option<SocketServer>,
}

impl Server {
    fn start() -> Self {
        let directory = tempfile::tempdir().expect("a directory");
        let store = Store::open(&directory.path().join("server.sqlite")).expect("the store");
        let info = ServerInfo::default();
        let socket =
            run_server_on_unix_socket(&directory.path().join("server.sock"), store.clone(), &info).expect("the socket");
        Self {
            directory,
            store,
            socket: Some(socket),
        }
    }

    fn path(&self) -> std::path::PathBuf {
        self.directory.path().join("server.sock")
    }

    /// One live link of `device`, with the device name as its label.
    fn live(&self, device: &str) -> Link {
        let stream = UnixStream::connect(self.path()).expect("the socket of the server");
        stream.set_read_timeout(Some(PATIENCE)).expect("a read bound");
        stream.set_write_timeout(Some(PATIENCE)).expect("a write bound");
        let mut link = Link {
            stream,
            device: device.to_owned(),
        };
        link.write(&Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: device.to_owned(),
            store: None,
            token: None,
        }));
        let greeting = link.frame();
        assert!(matches!(greeting, Frame::HelloOk(_)), "{greeting:?}");
        link.send(LiveRequest::Open {
            label: device.to_owned(),
        });
        let opened = link.event();
        assert!(matches!(opened, LiveEvent::Opened { .. }), "{opened:?}");
        link
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(socket) = self.socket.take() {
            socket.stop();
        }
        self.store.clone().close();
    }
}

/// One link of a computer of a test.
struct Link {
    stream: UnixStream,
    device: String,
}

impl Link {
    fn device(&self) -> &str {
        &self.device
    }

    fn write(&mut self, frame: &Frame) {
        write_frame(&mut self.stream, frame).expect("the frame leaves");
    }

    fn send(&mut self, request: LiveRequest) {
        self.write(&Frame::Live(request));
    }

    /// The next frame of this link that is not the heartbeat of an idle link.
    fn frame(&mut self) -> Frame {
        loop {
            let frame = read_frame(&mut self.stream)
                .expect("the frame reads")
                .expect("one frame");
            if frame != Frame::LiveEvent(LiveEvent::Heartbeat) {
                return frame;
            }
        }
    }

    fn event(&mut self) -> LiveEvent {
        match self.frame() {
            Frame::LiveEvent(event) => event,
            other => panic!("the frame is not an event: {other:?}"),
        }
    }

    /// Takes part in the session mirror. The server answers with no frame of its
    /// own, so [`List::sync`] proves that the link reached the server.
    fn mirror(&mut self) {
        self.send(LiveRequest::MirrorOpen);
    }

    /// Proves that every frame that this link sent before this call reached the
    /// server: a request of the live link has an answer, and the two frames of one
    /// link arrive in their order.
    fn sync(&mut self) {
        self.send(LiveRequest::Panels {
            panels: vec![panel("sync")],
        });
        loop {
            self.send(LiveRequest::Heartbeat);
            match self.event() {
                LiveEvent::Computers { computers } => {
                    if computers
                        .iter()
                        .any(|computer| computer.device == self.device() && computer.panels.len() == 1)
                    {
                        return;
                    }
                }
                // The list of the mirror of a link that opened before this one.
                LiveEvent::MirrorComputers { .. } => {}
                other => panic!("the frame is not a list of the live link: {other:?}"),
            }
        }
    }

    /// The next event of this link, past the lists of the live link and of the
    /// mirror, which come on their own and are no answer of one request.
    fn past_lists(&mut self) -> LiveEvent {
        loop {
            match self.event() {
                LiveEvent::Computers { .. } | LiveEvent::MirrorComputers { .. } => {}
                other => return other,
            }
        }
    }

    /// The next list of the mirror that this link reads.
    fn computers(&mut self) -> Vec<MirrorComputer> {
        loop {
            match self.event() {
                LiveEvent::MirrorComputers { computers } => return computers,
                LiveEvent::Computers { .. } => {}
                other => panic!("the frame is not a list of the mirror: {other:?}"),
            }
        }
    }

    /// The next list of the mirror that `want` accepts. The two opens of the pair of
    /// a test reach the server in no order, so a link reads one more empty list.
    fn computers_where(&mut self, want: impl Fn(&[MirrorComputer]) -> bool) -> Vec<MirrorComputer> {
        for _ in 0..8 {
            // A link that waits says that it is there, so a slow machine never ends
            // the link of a test for its silence.
            self.send(LiveRequest::Heartbeat);
            let computers = self.computers();
            if want(&computers) {
                return computers;
            }
        }
        panic!("the list of the mirror that the test waits for did not come");
    }

    /// The next answer of the publisher to a follow.
    fn watched(&mut self) -> (String, String, bool) {
        match self.past_lists() {
            LiveEvent::MirrorWatched {
                program,
                session,
                watched,
            } => (program, session, watched),
            other => panic!("the frame is not an answer of a follow: {other:?}"),
        }
    }

    /// The next entries that this link reads.
    fn entries(&mut self) -> (String, String, String, Vec<MirrorEntry>, bool) {
        match self.past_lists() {
            LiveEvent::MirrorEntries {
                owner,
                program,
                session,
                entries,
                tail,
            } => (owner, program, session, entries, tail),
            other => panic!("the frame is not a row of the mirror: {other:?}"),
        }
    }

    /// The next answer of the server to a follow that does not start.
    fn ended(&mut self) -> (String, horizon_sync::LiveEndReason) {
        match self.past_lists() {
            LiveEvent::MirrorEnded { owner, reason, .. } => (owner, reason),
            other => panic!("the frame is not the end of a follow: {other:?}"),
        }
    }
}

fn session(index: u32, live: bool) -> MirrorSession {
    MirrorSession {
        program: MirrorProgram::Claude,
        session: format!("00000000-0000-0000-0000-{index:012}"),
        title: format!("made-up session {index}"),
        folder: "repo".to_owned(),
        parent: "~/github".to_owned(),
        updated_at: 1_700_000_000_000 + i64::from(index),
        live,
    }
}

/// One panel that a computer shares, so a list of the live link changes.
fn panel(id: &str) -> LivePanel {
    LivePanel {
        id: id.to_owned(),
        title: format!("the title of {id}"),
        kind: "shell".to_owned(),
        state: "idle".to_owned(),
        workspace: "main".to_owned(),
        cols: 80,
        rows: 24,
        typing: false,
    }
}

/// One publishing computer and one viewer, both in the mirror.
fn pair(server: &Server) -> (Link, Link) {
    let mut a = server.live(A);
    let mut b = server.live(B);
    a.mirror();
    b.mirror();
    b.sync();
    a.sync();
    (a, b)
}

#[test]
fn a_viewer_reads_the_list_of_a_publisher_and_the_entries_of_one_session() {
    let server = Server::start();
    let (mut a, mut b) = pair(&server);

    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, true)],
        unreadable: vec![MirrorProgram::Antigravity],
    });
    let computers = b.computers_where(|computers| b_reads(computers, A) == 1);
    let publisher = computers
        .iter()
        .find(|computer| computer.device == A)
        .unwrap_or_else(|| panic!("the publisher is in {computers:?}"));
    assert_eq!(publisher.label, A);
    assert_eq!(publisher.sessions.len(), 1);
    assert_eq!(publisher.sessions[0].title, "made-up session 1");
    assert_eq!(b_reads(&computers, A), 1, "the viewer reads the list of the publisher");

    b.send(LiveRequest::MirrorFollow {
        owner: A.to_owned(),
        program: "claude".to_owned(),
        session: session(1, true).session,
    });
    let (program, followed, watched) = a.watched();
    assert_eq!(
        (program.as_str(), followed.as_str(), watched),
        ("claude", session(1, true).session.as_str(), true),
        "the publisher reads who follows it"
    );

    let entries = vec![
        MirrorEntry::Person {
            text: "made-up request".to_owned(),
        },
        MirrorEntry::Tool {
            name: "Bash".to_owned(),
            request: "made-up command".to_owned(),
        },
    ];
    a.send(LiveRequest::MirrorEntries {
        program: "claude".to_owned(),
        session: session(1, true).session,
        entries: entries.clone(),
        tail: true,
    });
    let (owner, program, _session, read, tail) = b.entries();
    assert_eq!((owner.as_str(), program.as_str(), tail), (A, "claude", true));
    assert_eq!(read, entries, "the whole tail of the follow");

    a.send(LiveRequest::MirrorEntries {
        program: "claude".to_owned(),
        session: session(1, true).session,
        entries: vec![MirrorEntry::Agent {
            text: "made-up answer".to_owned(),
        }],
        tail: false,
    });
    let (_, _, _, read, tail) = b.entries();
    assert!(!tail, "a later frame holds the new entries alone");
    assert_eq!(read.len(), 1, "{read:?}");
}

fn b_reads(computers: &[MirrorComputer], device: &str) -> usize {
    computers
        .iter()
        .find(|computer| computer.device == device)
        .map_or(0, |computer| computer.sessions.len())
}

#[test]
fn the_bounds_of_a_list_and_of_a_frame_cut_what_a_publisher_sends() {
    let server = Server::start();
    let (mut a, mut b) = pair(&server);

    let mut many: Vec<MirrorSession> = Vec::new();
    for index in 0..51 {
        many.push(session(index, false));
    }
    a.send(LiveRequest::MirrorSessions {
        sessions: many,
        unreadable: Vec::new(),
    });
    let computers = b.computers_where(|computers| b_reads(computers, A) == horizon_sync::MAX_MIRROR_SESSIONS);
    assert_eq!(
        b_reads(&computers, A),
        horizon_sync::MAX_MIRROR_SESSIONS,
        "the list is cut at its bound"
    );

    a.send(LiveRequest::MirrorSessions {
        sessions: vec![MirrorSession {
            title: "t".repeat(5_000),
            ..session(1, false)
        }],
        unreadable: Vec::new(),
    });
    let computers = b.computers_where(|computers| {
        computers
            .iter()
            .find(|computer| computer.device == A)
            .is_some_and(|computer| computer.sessions[0].title.chars().count() == 160)
    });
    let title = &computers
        .iter()
        .find(|c| c.device == A)
        .expect("the publisher")
        .sessions[0]
        .title;
    assert_eq!(title.chars().count(), 160, "the title is cut at its bound");

    b.send(LiveRequest::MirrorFollow {
        owner: A.to_owned(),
        program: "claude".to_owned(),
        session: session(1, false).session,
    });
    let _ = a.watched();
    let mut entries = Vec::new();
    for index in 0..80 {
        entries.push(MirrorEntry::Person {
            text: format!("made-up request {index}"),
        });
    }
    a.send(LiveRequest::MirrorEntries {
        program: "claude".to_owned(),
        session: session(1, false).session,
        entries,
        tail: true,
    });
    let (_, _, _, read, _) = b.entries();
    assert_eq!(read.len(), horizon_sync::MAX_MIRROR_ENTRIES, "60 entries at most");
}

#[test]
fn a_computer_that_never_sent_mirror_open_reads_no_mirror_frame() {
    let server = Server::start();
    let mut a = server.live(A);
    let mut older = server.live(B);
    a.mirror();
    a.sync();

    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, false)],
        unreadable: Vec::new(),
    });
    // The older computer sends a frame of a shared terminal, and the next frame it
    // reads is the answer of that frame: no mirror frame stands before it.
    older.send(LiveRequest::Panels {
        panels: vec![panel("p1")],
    });
    match older.event() {
        // The next frame is the answer of the request, so no frame of the mirror
        // stood before it: the older computer reads the live link only.
        LiveEvent::Computers { .. } => {}
        other => panic!("the frame is not a list of the live link: {other:?}"),
    }
}

#[test]
fn the_server_relays_a_session_only_inside_one_server() {
    let one = Server::start();
    let two = Server::start();
    let (mut a, mut b) = pair(&one);
    let mut other = two.live(B);
    other.mirror();
    other.sync();

    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, false)],
        unreadable: Vec::new(),
    });
    let computers = b.computers_where(|computers| b_reads(computers, A) == 1);
    assert_eq!(b_reads(&computers, A), 1, "the publisher reaches its own server");

    // The link of the other server reads the answer of its own request next, so no
    // frame of the first server reached it.
    other.send(LiveRequest::Panels {
        panels: vec![panel("p2")],
    });
    match other.event() {
        LiveEvent::Computers { .. } => {}
        other => panic!("a session of another server reached this one: {other:?}"),
    }
}

#[test]
fn a_follow_of_a_session_that_is_not_listed_ends_with_not_found() {
    let server = Server::start();
    let (mut a, mut b) = pair(&server);
    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, false)],
        unreadable: Vec::new(),
    });
    let _ = b.computers_where(|computers| b_reads(computers, A) == 1);

    b.send(LiveRequest::MirrorFollow {
        owner: A.to_owned(),
        program: "claude".to_owned(),
        session: session(9, false).session,
    });
    let (owner, reason) = b.ended();
    assert_eq!(owner, A);
    assert_eq!(
        reason,
        horizon_sync::LiveEndReason::NotFound,
        "the follow does not start"
    );
}

#[test]
fn the_server_keeps_no_session_in_its_store() {
    let server = Server::start();
    let (mut a, mut b) = pair(&server);
    a.send(LiveRequest::MirrorSessions {
        sessions: vec![MirrorSession {
            title: "made-up marker for the store".to_owned(),
            ..session(1, false)
        }],
        unreadable: Vec::new(),
    });
    let _ = b.computers_where(|computers| b_reads(computers, A) == 1);
    b.send(LiveRequest::MirrorFollow {
        owner: A.to_owned(),
        program: "claude".to_owned(),
        session: session(1, false).session,
    });
    let _ = a.watched();
    a.send(LiveRequest::MirrorEntries {
        program: "claude".to_owned(),
        session: session(1, false).session,
        entries: vec![MirrorEntry::Agent {
            text: "made-up marker for the log".to_owned(),
        }],
        tail: true,
    });
    let _ = b.entries();

    let file = std::fs::read(server.directory.path().join("server.sqlite")).expect("the store of the server");
    let text = String::from_utf8_lossy(&file);
    assert!(!text.contains("made-up marker"), "no session reaches the store");
}

#[test]
fn a_list_is_sent_again_only_when_the_publisher_lists_again() {
    let server = Server::start();
    let (mut a, mut b) = pair(&server);
    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, false)],
        unreadable: Vec::new(),
    });
    assert_eq!(
        b_reads(&b.computers_where(|computers| b_reads(computers, A) == 1), A),
        1
    );
    // A second frame with the same list changes nothing, so the next frame that the
    // viewer reads is the list of the live link.
    a.send(LiveRequest::MirrorSessions {
        sessions: vec![session(1, false)],
        unreadable: Vec::new(),
    });
    b.send(LiveRequest::Panels {
        panels: vec![panel("p1")],
    });
    match b.event() {
        LiveEvent::Computers { computers } => {
            assert!(computers.iter().any(|computer| computer.device == B));
        }
        other => panic!("no second list of the mirror comes: {other:?}"),
    }
}
