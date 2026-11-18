//! Shared terminals over real links: three computers on one server, each with a
//! live link of the socket of the server, and the handle of one computer over
//! the listener.
//!
//! A test never waits to see that a frame does not come. It sends a frame that
//! must come after it, and reads that this one is the next frame of the link.
//! The newest list replaces a list that still waits for a link, so a test reads
//! one list for one change, or it reads lists until the one that it expects.

use std::{
    io::Read,
    net::{SocketAddr, TcpListener},
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use horizon_core::secret_store::{self, SecretStore};
use horizon_sync::{
    DeviceRemove, DeviceRemoved, Frame, Hello, HelloOk, LinkErrorCode, ListenerInfo, LiveBytes, LiveEvent, LiveHandle,
    LivePanel, LiveRequest, LiveState, LiveSupervisor, PROTOCOL_VERSION, Pull, SERVER_DEVICE, ServerHome, ServerInfo,
    SocketServer, Store, SupervisorConfig, TcpServer, first_invitation, join, read_frame, run_server_on_tcp,
    run_server_on_unix_socket, write_frame,
};

/// The time that a test gives one frame to arrive.
const PATIENCE: Duration = Duration::from_secs(60);

const A: &str = "laptop-a";
const B: &str = "laptop-b";
const C: &str = "laptop-c";

/// One server of a test: its home, its store, its socket and its listener.
struct Server {
    directory: tempfile::TempDir,
    store: Store,
    listener: ListenerInfo,
    tcp: Option<TcpServer>,
    socket: Option<SocketServer>,
}

impl Server {
    fn start() -> Self {
        let directory = tempfile::tempdir().expect("a directory");
        let store = Store::open(&directory.path().join("server.sqlite")).expect("the store");
        let listener = ListenerInfo::free(None);
        let info = ServerInfo::with_listener(Some(listener.clone()));
        let tcp = run_server_on_tcp(&listener, store.clone(), &info).expect("the listener");
        let socket =
            run_server_on_unix_socket(&directory.path().join("server.sock"), store.clone(), &info).expect("the socket");
        Self {
            directory,
            store,
            listener,
            tcp: Some(tcp),
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
        for link in [&mut a, &mut b, &mut c] {
            link.settle("list laptop-a[p1,p2] laptop-b[] laptop-c[]");
        }
        (a, b, c)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tcp) = self.tcp.take() {
            tcp.stop();
        }
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

    /// Reads lists until the list of this link is `list`. A link reads fewer
    /// lists than there were changes when a newer list replaced one that waited.
    fn settle(&mut self, list: &str) {
        loop {
            let line = self.next();
            assert!(line.starts_with("list "), "only a list comes before {list}: {line}");
            if line == list {
                return;
            }
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
            ..
        } => format!("screen {owner}/{panel} {cols}x{rows}"),
        LiveEvent::Output { owner, panel, data, .. } => format!("output {owner}/{panel} {}", text(data)),
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
    LiveRequest::Screen {
        panel,
        cols,
        rows: 24,
        fresh: false,
        everyone: true,
    }
}

fn output(panel: &str, bytes: &[u8]) -> LiveRequest {
    let (panel, data) = (panel.to_owned(), LiveBytes(bytes.into()));
    LiveRequest::Output {
        panel,
        data,
        replay: false,
    }
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
    // list again changes nothing: the next frame of its computer is the answer
    // to the request that it sent after it.
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
    for link in [&mut a, &mut b, &mut c] {
        assert_eq!(link.next(), "list laptop-a[p1] laptop-b[] laptop-c[]");
    }
    a.send(panels(&[("p1", true)]));
    a.send(follow(C, "nope"));
    assert_eq!(a.next(), "ended laptop-c/nope NotFound", "the same list is no change");
    a.send(LiveRequest::Ended { panel: "p1".to_owned() });
    for link in [&mut a, &mut b, &mut c] {
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
        fresh: false,
        everyone: true,
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

/// A secrets file of one computer, which holds the token of its join.
fn secrets(directory: &Path) -> Arc<SecretStore> {
    SecretStore::open(directory.join("secrets.sqlite")).expect("the secrets file")
}

/// The settings of the link of `device` to the listener at `address`.
fn config(directory: &Path, device: &str, address: SocketAddr, values: Arc<SecretStore>) -> SupervisorConfig {
    use horizon_core::named_secrets::NamedSecretVault;
    use horizon_core::secret::{CredentialVault, SecretVault};

    horizon_sync::write_device_name(directory, device).expect("the name of this computer");
    let memory = || Box::new(SecretStore::in_memory().expect("a store in memory"));
    let credentials = Arc::new(CredentialVault::new(memory()));
    let named = Arc::new(NamedSecretVault::new(memory()));
    let keys = horizon_core::MachineKeys::new(directory.join("keys"));
    let target = Arc::new(SecretVault::new(keys, credentials, named));
    let mut config = SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", directory, target)
        .expect("the settings of the link");
    config.address = Some(address.to_string());
    config.token_store = Some(values);
    config
}

/// Takes the events of `handle` until `found` names one, and answers that one.
fn wait_for(handle: &LiveHandle, woken: &mpsc::Receiver<()>, found: impl Fn(&LiveEvent) -> bool) -> LiveEvent {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(event) = handle.take_events().into_iter().find(|event| found(event)) {
            return event;
        }
        assert!(Instant::now() < deadline, "the event did not come");
        let _ = woken.recv_timeout(Duration::from_millis(50));
    }
}

/// The computer B of a test, with a live link over the listener of a server.
struct Joined {
    supervisor: LiveSupervisor,
    handle: LiveHandle,
    woken: mpsc::Receiver<()>,
    _directory: tempfile::TempDir,
}

/// Joins `server` as B with the code of an invitation, and starts its live link.
fn join_b(server: &Server) -> Joined {
    let address = SocketAddr::from(([127, 0, 0, 1], server.listener.port()));
    let home = ServerHome::at(server.directory.path().to_path_buf());
    let invited = first_invitation(&home, SERVER_DEVICE).expect("an invitation");
    let directory = tempfile::tempdir().expect("a directory of B");
    let values = secrets(directory.path());
    join(&invited.link, B, &values).expect("the join of B");
    let (wake, woken) = mpsc::channel();
    let supervisor = LiveSupervisor::start(config(directory.path(), B, address, values), "Laptop B", move || {
        let _ = wake.send(());
    })
    .expect("the live link of B");
    let handle = supervisor.handle();
    Joined {
        supervisor,
        handle,
        woken,
        _directory: directory,
    }
}

#[test]
fn the_handle_of_a_computer_follows_a_panel_types_in_it_and_shares_its_own() {
    let server = Server::start();
    let mut a = server.live(A);
    a.send(panels(&[("p1", true)]));
    assert_eq!(a.next(), "list laptop-a[p1]");
    let Joined { handle, woken, .. } = &join_b(&server);

    // What the application wants before the link is open goes with the link.
    handle.set_panels(vec![panel("q1", true)]);
    handle.follow(A, "p1");
    let list = wait_for(handle, woken, |event| show(event) == "list laptop-a[p1] laptop-b[q1]");
    let LiveEvent::Computers { computers } = list else {
        panic!("a list");
    };
    assert_eq!(
        [computers[0].own, computers[1].own],
        [false, true],
        "this computer is marked"
    );
    assert_eq!(
        (handle.state(), computers[1].label.as_str()),
        (LiveState::Open, "Laptop B")
    );
    a.settle("list laptop-a[p1] laptop-b[q1]");
    assert_eq!(a.next(), "watchers p1 [laptop-b] refresh=true");

    // This computer follows a panel: it reads the screen and the output, and
    // what its person types reaches the owner.
    a.send(screen("p1", 80));
    a.send(output("p1", b"hello"));
    wait_for(handle, woken, |event| show(event) == "output laptop-a/p1 hello");
    handle.typed(A, "p1", b"ls\r");
    handle.typed(A, "p2", b"not followed");
    assert_eq!(a.next(), "typed p1 from laptop-b (Laptop B) ls\r");
    handle.follow(A, "p1");
    assert_eq!(
        a.next(),
        "watchers p1 [laptop-b] refresh=true",
        "a follow that is asked again asks the owner for a new screen"
    );

    // This computer shares a panel: it is asked for a screen, and its screen and
    // output reach the follower in frames of at most 48 KiB.
    a.send(follow(B, "q1"));
    wait_for(handle, woken, |event| {
        show(event) == "watchers q1 [laptop-a] refresh=true"
    });
    assert!(handle.screen("q1", 120, 40, false, true));
    assert!(handle.output("q1", &vec![b'z'; 100 * 1024], false));
    assert!(
        !handle.output("q1", &vec![b'z'; 1024 * 1024], false),
        "bytes that would pass the ceiling of the queue are refused, and none is sent"
    );
    assert!(handle.output("q1", b"end", false));
    let sizes = [a.next(), a.next(), a.next(), a.next(), a.next()].map(|line| line.len());
    let head = "output laptop-b/q1 ".len();
    assert_eq!(
        sizes,
        [
            "screen laptop-b/q1 120x40".len(),
            head + 48 * 1024,
            head + 48 * 1024,
            head + 4 * 1024,
            head + 3
        ]
    );
}

#[test]
fn the_handle_sends_its_state_again_on_a_new_link_and_nothing_while_the_link_is_away() {
    let server = Server::start();
    let mut a = server.live(A);
    a.send(panels(&[("p1", true)]));
    let joined = join_b(&server);
    let (handle, woken) = (&joined.handle, &joined.woken);
    handle.set_panels(vec![panel("q1", true)]);
    handle.follow(A, "p1");
    let until_followed = |a: &mut Link| {
        let mut lines = Vec::new();
        while lines.last().map(String::as_str) != Some("watchers p1 [laptop-b] refresh=true") {
            lines.push(a.next());
        }
        lines
    };
    until_followed(&mut a);
    handle.refresh(A, "p1");
    handle.refresh(B, "q1");
    assert_eq!(
        a.next(),
        "watchers p1 [laptop-b] refresh=true",
        "a follower asks the owner for a new screen"
    );
    a.send(follow(B, "q1"));
    wait_for(handle, woken, |event| {
        show(event) == "watchers q1 [laptop-a] refresh=true"
    });

    // Another live link of this device ends the link of the handle. The handle
    // opens a new link, and the server has its label, its panel and its follow again.
    let mut other = server.live(B);
    wait_for(
        handle,
        woken,
        |event| matches!(event, LiveEvent::State(LiveState::Down(reason)) if reason.starts_with("live_replaced")),
    );
    assert!(
        !handle.output("q1", b"away", false),
        "nothing is sent while the link is away"
    );
    let list = wait_for(handle, woken, |event| show(event) == "list laptop-a[p1] laptop-b[q1]");
    assert!(
        matches!(&list, LiveEvent::Computers { computers } if computers[1].label == "Laptop B"),
        "{list:?}"
    );
    assert_eq!(other.error(), "live_replaced");
    let lines = until_followed(&mut a);
    assert!(
        lines.contains(&"ended laptop-b/q1 Offline".to_owned()),
        "the follow of the panel of the older link ended: {lines:?}"
    );
    assert_eq!(lines[lines.len() - 2], "list laptop-a[p1] laptop-b[q1]");

    // The follow of this computer ends with its panel, and a panel of this
    // computer that is gone leaves the list.
    a.send(panels(&[]));
    wait_for(handle, woken, |event| show(event) == "ended laptop-a/p1 Closed");
    handle.ended("q1");
    a.settle("list laptop-a[] laptop-b[]");
    joined.supervisor.stop();
    assert_eq!(a.next(), "list laptop-a[]");
    assert!(
        !handle.screen("q1", 80, 24, false, true),
        "nothing is sent after a stop"
    );
}

#[test]
fn an_older_server_reads_no_live_frame_and_the_handle_says_that_it_is_unsupported() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a listener of the test");
    let address = listener.local_addr().expect("the address of the listener");
    let serving = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the link of the computer");
        stream.set_read_timeout(Some(PATIENCE)).expect("a read bound");
        let hello = read_frame(&mut stream).expect("the hello reads").expect("a hello");
        let answer = Frame::HelloOk(HelloOk {
            protocol: 5,
            store: "older-store".to_owned(),
            now_millis: 0,
            secrets_lost: false,
        });
        write_frame(&mut stream, &answer).expect("the greeting of an older server");
        // The link ends with no frame after the hello: one byte would be a frame
        // that an older server cannot read.
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
        (hello, rest)
    });

    let directory = tempfile::tempdir().expect("a directory");
    let values = secrets(directory.path());
    let token = horizon_sync::new_token().expect("a token");
    secret_store::write_token(&values, &token).expect("the token of this computer");
    let (wake, woken) = mpsc::channel();
    let supervisor = LiveSupervisor::start(config(directory.path(), B, address, values), "Laptop B", move || {
        let _ = wake.send(());
    })
    .expect("the live link");
    let handle = supervisor.handle();
    handle.set_panels(vec![panel("q1", true)]);
    wait_for(&handle, &woken, |event| {
        *event == LiveEvent::State(LiveState::Unsupported)
    });
    assert_eq!(handle.state(), LiveState::Unsupported);
    assert!(!handle.output("q1", b"nothing", false));

    let (hello, rest) = serving.join().expect("the older server");
    assert!(
        matches!(&hello, Frame::Hello(hello) if hello.protocol == PROTOCOL_VERSION && hello.device == B),
        "the hello is the hello of a sync link: {hello:?}"
    );
    assert_eq!(rest, Vec::<u8>::new(), "no live frame reaches an older server");
    supervisor.stop();
}
