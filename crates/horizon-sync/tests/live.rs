//! Shared terminals over real links: three computers on one server, each with a
//! live link of the socket of the server.
//!
//! A test never waits to see that a frame does not come. It sends a frame that
//! must come after it, and reads that this one is the next frame of the link.

use std::{os::unix::net::UnixStream, thread, time::Duration};

use horizon_sync::{
    DeviceRemove, DeviceRemoved, Frame, Hello, LinkErrorCode, LiveBytes, LiveEvent, LivePanel, LiveRequest,
    PROTOCOL_VERSION, Pull, ServerInfo, SocketServer, Store, read_frame, run_server_on_unix_socket, write_frame,
};

/// The time that a test gives one frame to arrive.
const PATIENCE: Duration = Duration::from_secs(60);

const A: &str = "laptop-a";
const B: &str = "laptop-b";
const C: &str = "laptop-c";

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

    /// One greeted link of `device`, which `ssh` would let in.
    fn link(&self, device: &str) -> Link {
        let stream = UnixStream::connect(self.directory.path().join("server.sock")).expect("the socket of the server");
        stream.set_read_timeout(Some(PATIENCE)).expect("a read bound");
        stream.set_write_timeout(Some(PATIENCE)).expect("a write bound");
        let mut link = Link { stream };
        link.write(&Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: device.to_owned(),
            store: None,
            token: None,
        }));
        let greeting = link.frame();
        assert!(matches!(greeting, Frame::HelloOk(_)), "{greeting:?}");
        link
    }

    /// One live link of `device`, with the device name as its label.
    fn live(&self, device: &str) -> Link {
        let mut link = self.link(device);
        link.send(LiveRequest::Open {
            label: device.to_owned(),
        });
        let opened = link.event();
        assert!(
            matches!(&opened, LiveEvent::Opened { device: named, .. } if named == device),
            "{opened:?}"
        );
        link
    }

    /// The live links of three computers. The first shares `p1`, where another
    /// computer may type, and `p2`, where it may not. Every link has read the list.
    fn three(&self) -> (Link, Link, Link) {
        let (mut a, mut b, mut c) = (self.live(A), self.live(B), self.live(C));
        a.send(panels(&[("p1", true), ("p2", false)]));
        let (two, three) = ("list laptop-a[] laptop-b[]", "list laptop-a[] laptop-b[] laptop-c[]");
        let list = "list laptop-a[p1,p2] laptop-b[] laptop-c[]";
        assert_eq!([a.next(), a.next(), a.next()], [two, three, list]);
        assert_eq!([b.next(), b.next()], [three, list]);
        assert_eq!(c.next(), list);
        (a, b, c)
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
}

impl Link {
    fn write(&mut self, frame: &Frame) {
        write_frame(&mut self.stream, frame).expect("the frame leaves");
    }

    fn send(&mut self, request: LiveRequest) {
        self.write(&Frame::Live(request));
    }

    fn frame(&mut self) -> Frame {
        read_frame(&mut self.stream)
            .expect("the frame reads")
            .expect("one frame")
    }

    fn event(&mut self) -> LiveEvent {
        match self.frame() {
            Frame::LiveEvent(event) => event,
            other => panic!("the frame is not an event: {other:?}"),
        }
    }

    /// The next event of this link, as one line.
    fn next(&mut self) -> String {
        show(&self.event())
    }

    /// The code of the error that is the next frame of this link.
    fn error(&mut self) -> &'static str {
        match self.frame() {
            Frame::Error(error) => error.code.as_str(),
            other => panic!("the frame is not an error: {other:?}"),
        }
    }

    /// Whether the link gives no more frame.
    fn is_over(&mut self) -> bool {
        !matches!(read_frame(&mut self.stream), Ok(Some(_)))
    }
}

/// One event as one line, so a test compares what a link read with a sentence.
fn show(event: &LiveEvent) -> String {
    let text = |data: &LiveBytes| String::from_utf8_lossy(&data.0).into_owned();
    match event {
        LiveEvent::Opened { computers, .. } | LiveEvent::Computers { computers } => {
            let computer = |computer: &horizon_sync::LiveComputer| {
                let ids: Vec<&str> = computer.panels.iter().map(|panel| panel.id.as_str()).collect();
                format!("{}[{}]", computer.device, ids.join(","))
            };
            format!("list {}", computers.iter().map(computer).collect::<Vec<_>>().join(" "))
        }
        LiveEvent::Watchers {
            panel,
            watchers,
            refresh,
        } => {
            let names: Vec<&str> = watchers.iter().map(|watcher| watcher.device.as_str()).collect();
            format!("watchers {panel} [{}] refresh={refresh}", names.join(","))
        }
        LiveEvent::Screen {
            owner,
            panel,
            cols,
            rows,
        } => format!("screen {owner}/{panel} {cols}x{rows}"),
        LiveEvent::Output { owner, panel, data } => format!("output {owner}/{panel} {}", text(data)),
        LiveEvent::Ended { owner, panel, reason } => format!("ended {owner}/{panel} {reason:?}"),
        LiveEvent::Typed {
            panel,
            from,
            label,
            data,
        } => format!("typed {panel} from {from} ({label}) {}", text(data)),
        LiveEvent::State(state) => format!("state {state:?}"),
    }
}

fn panel(id: &str, typing: bool) -> LivePanel {
    LivePanel {
        id: id.to_owned(),
        title: format!("the title of {id}"),
        kind: "shell".to_owned(),
        state: "idle".to_owned(),
        workspace: "main".to_owned(),
        cols: 80,
        rows: 24,
        typing,
    }
}

fn panels(shared: &[(&str, bool)]) -> LiveRequest {
    LiveRequest::Panels {
        panels: shared.iter().map(|(id, typing)| panel(id, *typing)).collect(),
    }
}

fn follow(owner: &str, panel: &str) -> LiveRequest {
    let (owner, panel) = (owner.to_owned(), panel.to_owned());
    LiveRequest::Follow { owner, panel }
}

fn unfollow(owner: &str, panel: &str) -> LiveRequest {
    let (owner, panel) = (owner.to_owned(), panel.to_owned());
    LiveRequest::Unfollow { owner, panel }
}

fn screen(panel: &str, cols: u16) -> LiveRequest {
    let panel = panel.to_owned();
    LiveRequest::Screen { panel, cols, rows: 24 }
}

fn output(panel: &str, bytes: &[u8]) -> LiveRequest {
    let (panel, data) = (panel.to_owned(), LiveBytes(bytes.into()));
    LiveRequest::Output { panel, data }
}

fn typed(owner: &str, panel: &str, bytes: &[u8]) -> LiveRequest {
    let (owner, panel, data) = (owner.to_owned(), panel.to_owned(), LiveBytes(bytes.into()));
    LiveRequest::Typed { owner, panel, data }
}

#[test]
fn the_list_reaches_every_link_and_changes_when_a_link_ends() {
    let server = Server::start();
    let mut a = server.link(A);
    a.send(LiveRequest::Open {
        label: format!("Laptop\u{7} A{}", "!".repeat(60)),
    });
    let LiveEvent::Opened { device, computers } = a.event() else {
        panic!("the answer to an open");
    };
    assert_eq!((device.as_str(), computers.len()), (A, 1));
    assert_eq!(
        computers[0].label,
        format!("Laptop A{}", "!".repeat(52)),
        "the label loses its control character and is cut at 60 characters"
    );

    let mut b = server.live(B);
    assert_eq!(a.next(), "list laptop-a[] laptop-b[]");
    let mut c = server.live(C);
    for link in [&mut a, &mut b] {
        assert_eq!(link.next(), "list laptop-a[] laptop-b[] laptop-c[]");
    }

    // A list of 70 panels is cut at 64, and each text at its bound. The same
    // list again changes nothing, so the next list is the one after it.
    let mut many: Vec<LivePanel> = (10..80).map(|index| panel(&format!("p{index}"), true)).collect();
    many[0].title = "t".repeat(200);
    a.send(LiveRequest::Panels { panels: many });
    for link in [&mut a, &mut b, &mut c] {
        let LiveEvent::Computers { computers } = link.event() else {
            panic!("the list after a change");
        };
        assert_eq!(computers[0].panels.len(), 64);
        assert_eq!(computers[0].panels[0].title.len(), 120);
    }
    a.send(panels(&[("p1", true)]));
    a.send(panels(&[("p1", true)]));
    a.send(LiveRequest::Ended { panel: "p1".to_owned() });
    for link in [&mut a, &mut b, &mut c] {
        assert_eq!(link.next(), "list laptop-a[p1] laptop-b[] laptop-c[]");
        assert_eq!(link.next(), "list laptop-a[] laptop-b[] laptop-c[]");
    }

    drop(c);
    for link in [&mut a, &mut b] {
        assert_eq!(link.next(), "list laptop-a[] laptop-b[]");
    }
}

#[test]
fn a_screen_and_its_output_reach_the_followers_of_the_panel_and_nobody_else() {
    let server = Server::start();
    let (mut a, mut b, mut c) = server.three();

    b.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");
    a.send(screen("p1", 80));
    a.send(output("p1", b"one"));
    assert_eq!(
        [b.next(), b.next()],
        ["screen laptop-a/p1 80x24", "output laptop-a/p1 one"]
    );

    // A second follower asks the owner for a new screen. The output before that
    // screen reaches the first follower alone, and the screen reaches both.
    c.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b,laptop-c] refresh=true");
    a.send(output("p1", b"two"));
    a.send(screen("p1", 100));
    a.send(output("p1", b"three"));
    assert_eq!(b.next(), "output laptop-a/p1 two");
    for link in [&mut b, &mut c] {
        assert_eq!(
            [link.next(), link.next()],
            ["screen laptop-a/p1 100x24", "output laptop-a/p1 three"]
        );
    }

    // The output of a panel that nobody follows goes nowhere: the next frame of
    // every link is the list that the owner changed after it.
    a.send(output("p2", b"nobody"));
    a.send(panels(&[("p1", true), ("p2", false), ("p3", false)]));
    for link in [&mut a, &mut b, &mut c] {
        assert_eq!(link.next(), "list laptop-a[p1,p2,p3] laptop-b[] laptop-c[]");
    }

    // A follower that leaves gets no more output, and the owner reads who is left.
    b.send(unfollow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-c] refresh=false");
    a.send(output("p1", b"four"));
    assert_eq!(c.next(), "output laptop-a/p1 four");
    c.send(unfollow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [] refresh=false");
    a.send(output("p1", b"five"));

    // A panel that leaves the list ends its follows with `closed`.
    b.send(follow(A, "p3"));
    assert_eq!(a.next(), "watchers p3 [laptop-b] refresh=true");
    a.send(panels(&[("p1", true)]));
    assert_eq!(
        [b.next(), b.next()],
        ["ended laptop-a/p3 Closed", "list laptop-a[p1] laptop-b[] laptop-c[]"]
    );
    assert_eq!(c.next(), "list laptop-a[p1] laptop-b[] laptop-c[]");
}

#[test]
fn what_is_typed_reaches_the_owner_only_from_a_follower_and_only_where_it_may_type() {
    let server = Server::start();
    let (mut a, mut b, mut c) = server.three();
    b.send(follow(A, "p1"));
    b.send(follow(A, "p2"));
    assert_eq!(
        [a.next(), a.next()],
        [
            "watchers p1 [laptop-b] refresh=true",
            "watchers p2 [laptop-b] refresh=true"
        ]
    );

    // A computer that does not follow the panel types nothing, a follower types
    // nothing in a panel where it may not, and a frame over 16 KiB is dropped.
    // Each link reads the answer to its own later request, so the server has
    // read what it typed before the follower types.
    c.send(typed(A, "p1", b"not a follower"));
    c.send(follow(A, "nope"));
    assert_eq!(c.next(), "ended laptop-a/nope NotFound");
    b.send(typed(A, "p2", b"may not type"));
    b.send(typed(A, "p1", &vec![b'x'; 16 * 1024 + 1]));
    b.send(follow(A, "nope"));
    assert_eq!(b.next(), "ended laptop-a/nope NotFound");
    b.send(typed(A, "p1", b"ls\r"));
    assert_eq!(a.next(), "typed p1 from laptop-b (laptop-b) ls\r");
    b.send(typed(A, "p1", &vec![b'y'; 16 * 1024]));
    assert_eq!(a.next().len(), "typed p1 from laptop-b (laptop-b) ".len() + 16 * 1024);

    // The owner takes the typing away: what the follower types after that is
    // dropped, and the next frame of the owner is the follower that leaves.
    a.send(panels(&[("p1", false), ("p2", false)]));
    for link in [&mut a, &mut b] {
        assert_eq!(link.next(), "list laptop-a[p1,p2] laptop-b[] laptop-c[]");
    }
    b.send(typed(A, "p1", b"too late"));
    b.send(unfollow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [] refresh=false");
}

#[test]
fn a_frame_over_its_bound_is_dropped_and_the_link_stays() {
    let server = Server::start();
    let (mut a, mut b, _c) = server.three();
    b.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");
    a.send(LiveRequest::Screen {
        panel: "p1".to_owned(),
        cols: 0,
        rows: u16::MAX,
    });
    a.send(output("p1", &vec![b'x'; 48 * 1024 + 1]));
    a.send(output("p1", b""));
    a.send(output("p1", &vec![b'y'; 48 * 1024]));
    a.send(output("p1", b"after"));
    assert_eq!(b.next(), "screen laptop-a/p1 1x1000", "a grid stays inside its bounds");
    assert_eq!(b.next().len(), "output laptop-a/p1 ".len() + 48 * 1024);
    assert_eq!(b.next(), "output laptop-a/p1 after");
}

#[test]
fn a_removed_device_loses_its_live_link_and_its_followers_read_that_it_is_offline() {
    let server = Server::start();
    let (mut a, mut b, mut c) = server.three();
    b.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");
    a.send(follow(C, "nope"));
    assert_eq!(a.next(), "ended laptop-c/nope NotFound");

    let mut window = server.link("laptop-d");
    window.write(&Frame::DeviceRemove(DeviceRemove { device: A.to_owned() }));
    let removed = window.frame();
    assert!(
        matches!(removed, Frame::DeviceRemoved(DeviceRemoved { removed: true, .. })),
        "{removed:?}"
    );
    assert_eq!(a.error(), "removed", "the live link ends like the sync link");
    assert!(a.is_over());
    assert_eq!(
        [b.next(), b.next()],
        ["ended laptop-a/p1 Offline", "list laptop-b[] laptop-c[]"]
    );
    assert_eq!(c.next(), "list laptop-b[] laptop-c[]");
}

#[test]
fn a_second_live_link_of_one_device_ends_the_older_one() {
    let server = Server::start();
    let (mut a, mut b, mut c) = server.three();
    b.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");
    a.send(follow(B, "nope"));
    assert_eq!(a.next(), "ended laptop-b/nope NotFound");

    // The newer link starts with nothing: the panels of the older one leave the
    // list, and the follower of one of them reads that it is offline.
    let mut newer = server.live(A);
    assert_eq!(a.error(), "live_replaced");
    assert!(a.is_over());
    assert_eq!(
        [b.next(), b.next()],
        ["ended laptop-a/p1 Offline", "list laptop-a[] laptop-b[] laptop-c[]"]
    );
    assert_eq!(c.next(), "list laptop-a[] laptop-b[] laptop-c[]");

    // The end of the older link does not take the newer one with it.
    newer.send(panels(&[("p9", false)]));
    for link in [&mut newer, &mut b, &mut c] {
        assert_eq!(link.next(), "list laptop-a[p9] laptop-b[] laptop-c[]");
    }
}

#[test]
fn a_live_link_carries_no_sync_request_and_a_sync_link_no_live_request() {
    let server = Server::start();
    let pull = || {
        Frame::Pull(Pull {
            since: 0,
            tip: String::new(),
        })
    };

    // The sync link of a computer answers while its live link is open.
    let mut live = server.live(A);
    let mut sync = server.link(A);
    sync.write(&pull());
    assert!(matches!(sync.frame(), Frame::Changes(_)));

    // A sync request on the live link ends that link with a typed error.
    live.write(&pull());
    assert_eq!(live.error(), "live_link");
    assert!(live.is_over());

    // A link that answered a sync request does not become a live link.
    sync.send(LiveRequest::Open { label: A.to_owned() });
    assert_eq!(sync.error(), "not_live");
    assert!(sync.is_over());

    // A live request that is not `open` needs a live link, and an event of the
    // server is not a request of a computer.
    let mut fresh = server.link(B);
    fresh.send(panels(&[("p1", true)]));
    assert_eq!(fresh.error(), "not_live");
    assert!(fresh.is_over());
    let mut fresh = server.link(B);
    fresh.write(&Frame::LiveEvent(LiveEvent::Computers { computers: Vec::new() }));
    assert_eq!(fresh.error(), "bad_frame");
    assert_eq!(LinkErrorCode::LiveLimit.as_str(), "live_limit");
}

#[test]
fn a_follower_that_does_not_read_does_not_hold_the_owner_and_is_drawn_again_when_it_reads() {
    let server = Server::start();
    let (mut a, mut b, _c) = server.three();
    b.send(follow(A, "p1"));
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");

    // The follower reads nothing while the owner sends 9.6 MiB of output. The
    // owner then changes its list and reads the list back, so the server has
    // read every frame of the owner while the follower read none.
    let sent = 200 * 48 * 1024;
    a.send(screen("p1", 80));
    for _ in 0..200 {
        a.send(output("p1", &vec![b'x'; 48 * 1024]));
    }
    a.send(panels(&[("p1", true)]));
    let mut asked = false;
    loop {
        let event = a.event();
        asked |= matches!(event, LiveEvent::Watchers { refresh: true, .. });
        if matches!(event, LiveEvent::Computers { .. }) {
            break;
        }
    }

    // The follower reads again. The server held about one mebibyte for it, and
    // the rest of the output never comes: the owner is asked for a new screen,
    // and that screen and the output after it are what the follower reads last.
    let reading = thread::spawn(move || {
        let (mut bytes, mut last) = (0, Vec::new());
        loop {
            match b.event() {
                LiveEvent::Screen { .. } => last.clear(),
                LiveEvent::Output { data, .. } => {
                    bytes += data.0.len();
                    last.push(data.0.len());
                    if &*data.0 == b"fresh" {
                        return (bytes, last);
                    }
                }
                _ => {}
            }
        }
    });
    while !asked {
        asked = matches!(a.event(), LiveEvent::Watchers { refresh: true, .. });
    }
    a.send(screen("p1", 100));
    a.send(output("p1", b"fresh"));
    let (bytes, last) = reading.join().expect("the follower reads");
    assert!(bytes < sent / 3, "the server dropped what waited: {bytes} of {sent}");
    assert_eq!(last, [5], "the relay starts again at the new screen");
}
