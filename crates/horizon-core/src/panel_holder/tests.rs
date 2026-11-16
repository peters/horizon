//! The holder of a panel, proven with a stand-in program and a plain client.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::frames::{self, Frame, Hello};
use super::{HolderError, Launch, PANEL_HOLDER_ARG, Paths, holder_command, start_with};
use crate::HorizonHome;

/// The longest time that a test waits for one condition of a holder.
const TEST_BOUND: Duration = Duration::from_secs(10);

/// A home in a folder of a short path. The path of a Unix socket is bounded, and
/// the temp folder of a system is long on a Mac.
fn temp_home() -> (tempfile::TempDir, HorizonHome) {
    let folder = tempfile::Builder::new()
        .prefix("hz-holder")
        .tempdir_in("/tmp")
        .expect("a folder for a home");
    let home = HorizonHome::from_root(folder.path().to_path_buf());
    (folder, home)
}

fn launch(local_id: &str, script: &str) -> Launch {
    Launch {
        local_id: local_id.to_owned(),
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: None,
        env: std::collections::HashMap::from([("TEST_MARK".to_owned(), "the environment of the holder".to_owned())]),
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
    }
}

/// Waits until `condition` holds, and fails the test when it does not.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + TEST_BOUND;
    while !condition() {
        assert!(Instant::now() < deadline, "waited for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// A holder that serves on a thread of the test.
struct Serving {
    socket: PathBuf,
    result: Receiver<Result<(), HolderError>>,
    thread: thread::JoinHandle<()>,
}

impl Serving {
    /// Starts a holder and waits until it answers a hello.
    fn start(home: &HorizonHome, launch: &Launch, ring_limit: usize) -> Self {
        let socket = Paths::of(home, &launch.local_id)
            .expect("the socket of the holder")
            .socket()
            .to_path_buf();
        let (send, result) = mpsc::channel();
        let owned = home.clone();
        let served = launch.clone();
        let thread = thread::spawn(move || {
            let _ = send.send(super::serve::serve(&owned, &served, ring_limit));
        });
        let serving = Self { socket, result, thread };
        let deadline = Instant::now() + TEST_BOUND;
        loop {
            match super::probe(home, &launch.local_id) {
                Ok(Some(_)) => return serving,
                _ if Instant::now() >= deadline => panic!("the holder did not answer a hello"),
                _ => thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    /// Waits for the holder to end, and fails the test when it ended with a problem.
    fn ended(self) {
        let result = self.result.recv_timeout(TEST_BOUND).expect("the holder ended");
        self.thread.join().expect("the thread of the holder");
        assert!(result.is_ok(), "the holder ended with {result:?}");
    }
}

/// A plain client of a holder.
struct Client {
    stream: UnixStream,
}

impl Client {
    fn attach(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("the socket of the holder");
        stream.set_read_timeout(Some(TEST_BOUND)).expect("a read bound");
        Self { stream }
    }

    fn next(&mut self) -> Frame {
        frames::read_one(&mut self.stream).expect("a frame of the holder")
    }

    fn send(&mut self, frame: &Frame) {
        frames::write(&mut self.stream, frame).expect("the write of a frame");
    }

    /// Reads output until it holds `wanted`, and answers everything it read.
    fn text_until(&mut self, wanted: &str) -> String {
        let mut seen = String::new();
        let deadline = Instant::now() + TEST_BOUND;
        while !seen.contains(wanted) {
            assert!(Instant::now() < deadline, "waited for {wanted:?}, read {seen:?}");
            match self.next() {
                Frame::Output(bytes) | Frame::Replay(bytes) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("the holder sent {other:?} while the test waited for {wanted:?}"),
            }
        }
        seen
    }
}

/// Attaches, and retries while the holder still serves the client that left.
fn attach_when_free(socket: &Path) -> (Client, Hello) {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        let mut client = Client::attach(socket);
        match client.next() {
            Frame::Hello(hello) => return (client, hello),
            Frame::Refusal => {}
            other => panic!("the holder answered {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "the holder frees its socket for the next client"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// Ends the holder of a test through its own `Close` frame.
fn close(client: &mut Client, serving: Serving) {
    client.send(&Frame::Close);
    serving.ended();
}

#[test]
fn the_hello_names_the_pid_of_a_live_program() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("hello", "exec sleep 60"), 1024);
    let (mut client, hello) = attach_when_free(&serving.socket);

    assert_ne!(hello.pid, std::process::id(), "the program is no thread of the test");
    assert!(crate::process::is_alive(hello.pid), "the hello names a live program");
    assert!(
        hello.start_time.is_some(),
        "the hello names the start time of the program"
    );
    assert_eq!(hello.ring_len, 0, "a program that printed nothing holds no replay");

    close(&mut client, serving);
}

#[test]
fn the_output_of_the_program_reaches_the_client() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch("output", "printf 'hello from the program\\n'; exec cat"),
        1024,
    );
    let (mut client, _hello) = attach_when_free(&serving.socket);

    let seen = client.text_until("hello from the program");
    assert!(seen.contains("hello from the program"), "{seen:?}");
    close(&mut client, serving);
}

#[test]
fn the_input_of_the_client_reaches_the_program() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch("input", "read line; printf 'read %s\\n' \"$line\""),
        1024,
    );
    let (mut client, _hello) = attach_when_free(&serving.socket);

    client.send(&Frame::Input(b"ping\n".to_vec()));
    let seen = client.text_until("read ping");
    assert!(
        seen.contains("read ping"),
        "the program read the bytes of the client: {seen:?}"
    );
    assert_eq!(client.next(), Frame::Exit(Some(0)), "the program of the read ended");
    drop(client);
    serving.ended();
}

#[test]
fn a_resize_reaches_the_program() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("resize", "read line; stty size; exec cat"), 1024);
    let (mut client, _hello) = attach_when_free(&serving.socket);

    client.send(&Frame::Resize {
        rows: 40,
        cols: 100,
        cell_width: 9,
        cell_height: 18,
    });
    client.send(&Frame::Input(b"go\n".to_vec()));
    let seen = client.text_until("40 100");
    assert!(seen.contains("40 100"), "the program read the new size: {seen:?}");
    close(&mut client, serving);
}

#[test]
fn the_program_and_its_text_survive_a_detach() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch("detach", "printf 'first\\n'; sleep 1; printf 'second\\n'; exec cat"),
        1024,
    );
    let (mut client, hello) = attach_when_free(&serving.socket);
    client.text_until("first");
    drop(client);

    let (mut client, again) = attach_when_free(&serving.socket);
    assert_eq!(again.pid, hello.pid, "the detached client finds the same program");
    let seen = client.text_until("second");
    assert!(
        seen.contains("first"),
        "the replay holds the text from before the attach: {seen:?}"
    );
    assert!(
        seen.contains("second"),
        "the client reads the text of the detach: {seen:?}"
    );
    close(&mut client, serving);
}

#[test]
fn a_second_client_while_one_is_attached_gets_the_refusal() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch("refusal", "read line; printf 'read %s\\n' \"$line\""),
        1024,
    );
    let (mut client, _hello) = attach_when_free(&serving.socket);

    let mut second = Client::attach(&serving.socket);
    assert_eq!(second.next(), Frame::Refusal, "one client at a time");

    client.send(&Frame::Input(b"still mine\n".to_vec()));
    let seen = client.text_until("read still mine");
    assert!(
        seen.contains("read still mine"),
        "the attached client keeps the program: {seen:?}"
    );
    assert_eq!(client.next(), Frame::Exit(Some(0)), "the program of the read ended");
    drop(client);
    serving.ended();
}

#[test]
fn a_close_ends_the_program_and_removes_the_socket() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("close", "exec sleep 120"), 1024);
    let (mut client, hello) = attach_when_free(&serving.socket);
    let socket = serving.socket.clone();

    close(&mut client, serving);
    wait_until("the end of the program", || !crate::process::is_alive(hello.pid));
    assert!(!socket.exists(), "the end of the holder removes its socket");
}

#[test]
fn the_status_of_a_program_that_ended_reaches_the_client() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("status", "read line; exit 3"), 1024);
    let (mut client, hello) = attach_when_free(&serving.socket);

    client.send(&Frame::Input(b"end\n".to_vec()));
    let mut status = None;
    while status.is_none() {
        match client.next() {
            Frame::Exit(code) => status = Some(code),
            Frame::Output(_) | Frame::Replay(_) => {}
            other => panic!("the holder sent {other:?}"),
        }
    }
    assert_eq!(status, Some(Some(3)), "the client reads the status of the program");
    assert!(!crate::process::is_alive(hello.pid), "the program ended");

    drop(client);
    let socket = serving.socket.clone();
    serving.ended();
    assert!(!socket.exists(), "the end of the holder removes its socket");
}

#[test]
fn the_ring_holds_only_its_newest_bytes() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            "ring",
            "printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; printf 'zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz'; exec cat",
        ),
        32,
    );
    let (mut client, _hello) = attach_when_free(&serving.socket);
    client.text_until("zzzz");
    drop(client);

    let (mut client, hello) = attach_when_free(&serving.socket);
    assert_eq!(hello.ring_len, 32, "the ring keeps its bound and no more");
    let Frame::Replay(replay) = client.next() else {
        panic!("the new client reads the ring first");
    };
    assert_eq!(replay.len(), 32, "the replay holds the bound of the ring");
    assert!(
        replay.starts_with(b"zz"),
        "the replay holds the newest bytes: {replay:?}"
    );
    assert!(
        !replay.contains(&b'a'),
        "the replay dropped the bytes above its bound: {replay:?}"
    );
    close(&mut client, serving);
}

#[test]
fn the_socket_and_its_folder_are_private() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("private", "exec sleep 60"), 1024);
    let (mut client, _hello) = attach_when_free(&serving.socket);

    let socket = std::fs::metadata(&serving.socket).expect("the socket");
    assert_eq!(
        socket.permissions().mode() & 0o777,
        0o600,
        "the socket is the owner's alone"
    );
    let holders = std::fs::metadata(serving.socket.parent().expect("the folder of the sockets")).expect("the folder");
    assert_eq!(
        holders.permissions().mode() & 0o777,
        0o700,
        "the folder is the owner's alone"
    );
    close(&mut client, serving);
}

#[test]
fn a_stale_socket_is_replaced_and_a_live_one_is_not() {
    let (_folder, home) = temp_home();
    let dead = launch("stale", "read line; printf 'read %s\\n' \"$line\"");
    let paths = Paths::of(&home, &dead.local_id).expect("the socket");
    std::fs::create_dir_all(paths.socket().parent().expect("the folder")).expect("the folder");
    std::fs::set_permissions(
        paths.socket().parent().expect("the folder"),
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("a private folder");
    drop(UnixListener::bind(paths.socket()).expect("a socket that a killed holder left"));

    let serving = Serving::start(&home, &dead, 1024);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    let live = super::serve::serve(&home, &dead, 1024);
    assert!(
        matches!(live, Err(HolderError::AlreadyServing(_))),
        "a socket that answers is not replaced"
    );
    client.send(&Frame::Input(b"alive\n".to_vec()));
    client.text_until("read alive");
    close(&mut client, serving);
}

#[test]
fn a_client_that_breaks_the_protocol_leaves_the_program_running() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            "broken",
            "while IFS= read -r line; do printf 'read %s\\n' \"$line\"; done",
        ),
        1024,
    );
    let (mut client, hello) = attach_when_free(&serving.socket);
    client.send(&Frame::Input(b"kept\n".to_vec()));
    client.text_until("read kept");
    drop(client);

    let (mut broken, again) = attach_when_free(&serving.socket);
    assert_eq!(again.pid, hello.pid, "the detached client finds the same program");
    broken.stream.write_all(&[0xEE, 0, 0, 0, 0]).expect("bytes of no frame");
    drop(broken);

    let (mut broken, again) = attach_when_free(&serving.socket);
    assert_eq!(
        again.pid, hello.pid,
        "the program outlives a client that broke the protocol"
    );
    broken
        .stream
        .write_all(&[1, 0x00, 0x10, 0x00, 0x01])
        .expect("a frame above the bound");
    drop(broken);

    let (mut client, again) = attach_when_free(&serving.socket);
    assert_eq!(again.pid, hello.pid, "the program outlives a frame above the bound");
    assert!(crate::process::is_alive(hello.pid), "the program still runs");
    client.send(&Frame::Input(b"ends\n".to_vec()));
    client.text_until("read ends");
    close(&mut client, serving);
}

#[test]
fn the_launch_document_is_no_argument_of_the_holder() {
    let command = holder_command(Path::new("/bin/true"));
    let arguments: Vec<_> = command.get_args().collect();
    assert_eq!(
        arguments,
        [PANEL_HOLDER_ARG],
        "the command names the mode and nothing else"
    );
    assert_eq!(command.get_program(), "/bin/true");
}

#[test]
fn a_start_writes_the_launch_document_to_the_standard_input() {
    let (folder, home) = temp_home();
    let copy = folder.path().join("document.json");
    let stand_in = crate::util::stand_in_program_in(
        folder.path(),
        "holder-stand-in",
        &format!("cat > '{}'\n", copy.display()),
    )
    .expect("a stand-in program");
    let asked = launch("document", "printf 'never'");

    let started = start_with(&stand_in, &home, &asked);
    assert!(
        matches!(started, Err(HolderError::StartExited(Some(0)))),
        "the stand-in ends without answering: {started:?}"
    );
    let document: Launch = serde_json::from_slice(&std::fs::read(&copy).expect("the document of the start"))
        .expect("the launch document of the holder");
    assert_eq!(
        document, asked,
        "the holder reads its whole launch on its standard input"
    );
    assert_eq!(document.env["TEST_MARK"], "the environment of the holder");
}

#[test]
fn a_probe_finds_no_holder_where_no_socket_answers() {
    let (_folder, home) = temp_home();
    assert_eq!(
        super::probe(&home, "nothing").expect("no socket"),
        None,
        "no socket is no holder"
    );

    let paths = Paths::of(&home, "stale").expect("the socket");
    std::fs::create_dir_all(paths.socket().parent().expect("the folder")).expect("the folder");
    drop(UnixListener::bind(paths.socket()).expect("a socket that nobody listens on"));
    assert_eq!(
        super::probe(&home, "stale").expect("a stale socket"),
        None,
        "a socket that nobody listens on is no holder"
    );

    let mut long = launch("x", "exec cat");
    long.local_id = "x".repeat(200);
    assert!(
        matches!(Paths::of(&home, &long.local_id), Err(HolderError::PathTooLong(_))),
        "a socket path above the bound is refused, not cut"
    );
}

#[test]
fn the_folder_of_the_sockets_is_made_private() {
    let (_folder, home) = temp_home();
    let paths = Paths::of(&home, "private-folder").expect("the socket");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(paths.socket().parent().expect("the folder"))
        .expect("a folder that is open to other users");
    paths.prepare().expect("a private folder");
    let mode = std::fs::metadata(paths.socket().parent().expect("the folder"))
        .expect("the folder")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "a folder that existed is made private");
}
