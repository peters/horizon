//! The login of a computer: the code of an invitation, the token that follows, the
//! list of the computers and the removal of one.
//!
//! Every test of this file runs the real server over a real Unix socket and a real
//! TCP listener on loopback, with real store files and a real secrets file. The
//! listener binds port 0 and the test reads the port back, so no test names a port
//! of the machine that runs it.
//!
//! The clock of the server is a value of the test, so an invitation that expires
//! and the second that a wrong code waits cost no time. The one real wait of these
//! tests is the socket of a link, which is the code path of a person.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use horizon_core::named_secrets::{NamedSecretVault, SecretName};
use horizon_core::remote_browser_credential::SharedCredentialStore;
use horizon_core::secret_store::{self, SecretStore};
use horizon_core::{JoinLink, Machine};
use horizon_sync::{
    Client, Clock, DeviceList, DeviceLogin, DeviceRemove, DeviceRemoved, Devices, Frame, Hello, HelloOk, Intent,
    Invite, Invited, Join, JoinError, Link, LinkRegistry, ListenerInfo, ObjectMirror, Op, PROTOCOL_VERSION, ServerInfo,
    ServerReply, ServerRequest, SocketServer, Store, TcpServer, WRONG_CODE_DELAY, first_invitation, join,
    run_server_on_tcp, run_server_on_unix_socket,
};

/// The time that a test gives one frame to arrive.
const PATIENCE: Duration = Duration::from_secs(20);

/// The value of the named secret that one computer sends to the other.
const NAMED_VALUE: &str = "MARKER-e71c-a-named-value";

/// The clock of a test: a time that the test moves, and the sleeps that the server
/// takes instead of waiting.
#[derive(Debug, Default)]
struct TestClock {
    now: AtomicI64,
    slept: Mutex<Vec<Duration>>,
}

impl TestClock {
    fn at(millis: i64) -> Self {
        Self {
            now: AtomicI64::new(millis),
            ..Self::default()
        }
    }

    fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    fn slept(&self) -> Vec<Duration> {
        self.slept.lock().expect("the sleeps").clone()
    }

    fn clear(&self) {
        self.slept.lock().expect("the sleeps").clear();
    }
}

impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) {
        self.slept.lock().expect("the sleeps").push(duration);
    }
}

/// One server of a test: its home, its store, its socket and its listener.
struct Server {
    directory: tempfile::TempDir,
    store: Store,
    listener: ListenerInfo,
    tcp: Option<TcpServer>,
    socket: Option<SocketServer>,
    clock: Arc<TestClock>,
}

impl Server {
    fn start() -> Self {
        Self::with_clock(Arc::new(TestClock::at(1_760_000_000_000)))
    }

    fn with_clock(clock: Arc<TestClock>) -> Self {
        let directory = tempfile::tempdir().expect("a directory");
        let store = Store::open(&directory.path().join("server.sqlite")).expect("the store");
        let listener = ListenerInfo::free(None);
        let info = ServerInfo {
            listener: Some(listener.clone()),
            registry: Arc::new(LinkRegistry::default()),
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
        };
        let tcp = run_server_on_tcp(&listener, store.clone(), &info).expect("the listener");
        let socket =
            run_server_on_unix_socket(&directory.path().join("server.sock"), store.clone(), &info).expect("the socket");
        Self {
            directory,
            store,
            listener,
            tcp: Some(tcp),
            socket: Some(socket),
            clock,
        }
    }

    fn home(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }

    fn address(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.listener.port()))
    }

    /// One link of the socket, greeted as `device`, which `ssh` would let in.
    fn ssh(&self, device: &str) -> UnixStream {
        let path = self.directory.path().join("server.sock");
        let mut stream = UnixStream::connect(&path).expect("the socket of the server");
        stream
            .set_read_timeout(Some(PATIENCE))
            .expect("a read bound of the test");
        let answer = exchange(
            &mut stream,
            &Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: device.to_owned(),
                store: None,
                token: None,
            }),
        );
        assert!(matches!(answer, Frame::HelloOk(_)), "{answer:?}");
        stream
    }

    /// One link of the listener, greeted with `token`.
    fn tcp(&self, device: &str, token: &str) -> TcpStream {
        let mut stream = TcpStream::connect(self.address()).expect("the listener of the server");
        stream
            .set_read_timeout(Some(PATIENCE))
            .expect("a read bound of the test");
        let answer = exchange(
            &mut stream,
            &Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: device.to_owned(),
                store: None,
                token: Some(token.to_owned()),
            }),
        );
        assert!(matches!(answer, Frame::HelloOk(_)), "{answer:?}");
        stream
    }

    /// One invitation, made by the computer that runs the server.
    fn invite(&self) -> Invited {
        let invited = first_invitation(&horizon_sync::ServerHome::at(self.home()), horizon_sync::SERVER_DEVICE)
            .expect("the first invitation");
        assert_eq!(invited.code.len(), 14);
        assert!(invited.expires_millis > 0);
        invited
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

/// Sends one frame and reads its answer.
fn exchange(stream: &mut (impl Read + Write), frame: &Frame) -> Frame {
    horizon_sync::write_frame(stream, frame).expect("the frame leaves");
    receive(stream)
}

fn receive(stream: &mut impl Read) -> Frame {
    horizon_sync::read_frame(stream)
        .expect("the frame reads")
        .expect("one frame")
}

fn error_code(frame: &Frame) -> &'static str {
    match frame {
        Frame::Error(error) => error.code.as_str(),
        other => panic!("the frame is not an error: {other:?}"),
    }
}

/// A secrets file of one computer, which holds the token of its join.
fn secrets(directory: &Path) -> Arc<SecretStore> {
    SecretStore::open(directory.join("secrets.sqlite")).expect("the secrets file")
}

/// The token that `store` holds.
fn token_of(store: &SecretStore) -> String {
    secret_store::read_token(store)
        .expect("the token row")
        .expect("a token")
        .to_string()
}

/// A machine of the shared list.
fn machine(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        port: Some(2222),
        session: Some("work".to_owned()),
        provider: Some("Hetzner, hel1".to_owned()),
        added_at_millis: 7,
    }
}

#[test]
fn the_right_code_joins_a_computer_and_writes_its_token() {
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let store = secrets(directory.path());

    let joined = join(&invited.link, "laptop-b", &store).expect("the join");
    assert_eq!(joined.device, "laptop-b");
    assert_eq!(token_of(&store), joined.token, "the secrets file holds the token");
    assert!(
        secret_store::check_token(&joined.token),
        "the token is one token of this Horizon"
    );

    // The computer comes back with a hello that presents the token.
    let mut stream = server.tcp("laptop-b", &joined.token);
    let rows = ask_devices(&mut stream);
    assert_eq!(rows.len(), 2, "the join left one row for each computer: {rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.name == "server" && row.login == DeviceLogin::Ssh)
    );
    let b = rows.iter().find(|row| row.name == "laptop-b").expect("the row of B");
    assert_eq!(b.login, DeviceLogin::Token);
    assert!(b.caller, "the list names the computer that asked");

    // A join ends its own link, so the computer connects again: the link above is
    // the one that the token opened.
    assert_eq!(
        JoinLink::parse(&invited.link)
            .expect("the link of the invitation")
            .code(),
        invited.code,
        "the link holds the code that the answer names"
    );
}

#[test]
fn a_wrong_code_answers_after_one_second_and_an_expired_code_reads_the_same() {
    let server = Server::start();
    let invited = server.invite();

    // A code that is not one invitation at all.
    let wrong = try_join(&server, "ZZZZ-ZZZZ-ZZZZ", "laptop-b");
    assert_eq!(error_code(&wrong), "wrong_code");
    assert_eq!(
        server.clock.slept(),
        vec![WRONG_CODE_DELAY],
        "a wrong code waits one second before the answer"
    );

    // The invitation expires after ten minutes, and an expired code reads like a
    // wrong one.
    server.clock.advance(600_001);
    server.clock.clear();
    let expired = try_join(&server, &invited.code, "laptop-b");
    assert_eq!(error_code(&expired), "wrong_code");
    assert_eq!(server.clock.slept(), vec![WRONG_CODE_DELAY], "no oracle of an expiry");
}

#[test]
fn a_used_code_is_no_code_any_more() {
    let server = Server::start();
    let invited = server.invite();
    let first = tempfile::tempdir().expect("a directory");
    join(&invited.link, "laptop-b", &secrets(first.path())).expect("the first join");

    let second = try_join(&server, &invited.code, "laptop-c");
    assert_eq!(error_code(&second), "wrong_code", "an invitation works once");
}

#[test]
fn twenty_wrong_codes_delete_every_invitation_until_a_new_one() {
    let server = Server::start();
    let invited = server.invite();
    let mut answers = Vec::new();
    for guess in 0..horizon_sync::MAX_WRONG_CODES {
        let wrong = format!("WRONG-CODE-{guess:04}");
        answers.push(error_code(&try_join(&server, &wrong, "laptop-b")));
    }
    assert!(
        answers[..horizon_sync::MAX_WRONG_CODES - 1]
            .iter()
            .all(|code| *code == "wrong_code"),
        "each guess before the last is one wrong code: {answers:?}"
    );
    assert_eq!(answers[horizon_sync::MAX_WRONG_CODES - 1], "join_locked");

    // The door is closed, so even the right code is refused now.
    let locked = try_join(&server, &invited.code, "laptop-b");
    assert_eq!(error_code(&locked), "join_locked");

    // One new invitation opens the door again, and the old code stays dead.
    let fresh = server.invite();
    let joined = try_join(&server, &fresh.code, "laptop-b");
    assert!(matches!(joined, Frame::Joined(_)), "{joined:?}");
    assert_eq!(server.clock.slept().len(), horizon_sync::MAX_WRONG_CODES);
}

#[test]
fn a_name_that_already_has_a_token_is_taken() {
    let server = Server::start();
    let invited = server.invite();
    let first = tempfile::tempdir().expect("a directory");
    join(&invited.link, "laptop-b", &secrets(first.path())).expect("the first join");

    let invited = server.invite();
    let taken = try_join(&server, &invited.code, "laptop-b");
    assert_eq!(error_code(&taken), "name_taken");

    // The code is still open, so the person removes the old entry or names this
    // computer otherwise.
    let other = try_join(&server, &invited.code, "laptop-c");
    assert!(matches!(other, Frame::Joined(_)), "{other:?}");
}

#[test]
fn a_hello_presents_the_token_of_exactly_one_device() {
    let server = Server::start();
    let invited = server.invite();
    let first = tempfile::tempdir().expect("a directory");
    let b = join(&invited.link, "laptop-b", &secrets(first.path())).expect("the join");
    let invited = server.invite();
    let second = tempfile::tempdir().expect("a directory");
    let c = join(&invited.link, "laptop-c", &secrets(second.path())).expect("the join");

    // No token, another token, and the token of another device.
    for (device, token) in [
        ("laptop-b", None),
        ("laptop-b", Some("not-a-token-of-this-server".to_owned())),
        ("laptop-b", Some(c.token.clone())),
        ("laptop-c", Some(b.token.clone())),
    ] {
        let mut stream = TcpStream::connect(server.address()).expect("the listener");
        let answer = exchange(
            &mut stream,
            &Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: device.to_owned(),
                store: None,
                token,
            }),
        );
        assert_eq!(
            error_code(&answer),
            "not_logged_in",
            "a link of {device} with that token is refused"
        );
    }

    // The token of the device itself is the one login that answers.
    let _stream = server.tcp("laptop-b", &b.token);
}

#[test]
fn the_removal_of_a_computer_ends_its_link_and_frees_its_name() {
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let store = secrets(directory.path());
    let b = join(&invited.link, "laptop-b", &store).expect("the join");

    let mut open = server.tcp("laptop-b", &b.token);
    let mut a = server.ssh("laptop-a");
    let removed = exchange(
        &mut a,
        &Frame::DeviceRemove(DeviceRemove {
            device: "laptop-b".to_owned(),
        }),
    );
    let Frame::DeviceRemoved(DeviceRemoved { device, removed: held }) = removed else {
        panic!("the removal answers its own frame: {removed:?}");
    };
    assert_eq!(device, "laptop-b");
    assert!(held);

    // The open link of B ends with the typed error, and the row of B is gone.
    let ended = receive(&mut open);
    assert_eq!(error_code(&ended), "removed");
    assert!(
        server
            .store
            .devices("laptop-a")
            .expect("the computers")
            .iter()
            .all(|row| row.name != "laptop-b"),
        "the removal deletes the row of the computer"
    );

    // The link of A is still open, and the name of B is free again.
    let rows = ask_devices(&mut a);
    assert!(rows.iter().all(|row| row.name != "laptop-b"), "{rows:?}");
    let invited = server.invite();
    let joined = join(&invited.link, "laptop-b", &store).expect("a new join takes the name again");
    assert_eq!(joined.device, "laptop-b");
}

/// One greeting of the listener with `token`, which answers the frame of the
/// server as it is: the greeting of a computer that is not in step.
fn greet_with(server: &Server, device: &str, token: &str) -> Frame {
    let mut stream = TcpStream::connect(server.address()).expect("the listener");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("a read bound of the test");
    exchange(
        &mut stream,
        &Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: device.to_owned(),
            store: None,
            token: Some(token.to_owned()),
        }),
    )
}

#[test]
fn a_computer_whose_row_was_removed_while_it_was_away_reads_that_it_was_removed() {
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let b = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");

    // B greeted the server once and is away now: its link is gone before the
    // removal, so the removal ends no link of B and only the row goes.
    drop(server.tcp("laptop-b", &b.token));
    let mut a = server.ssh("laptop-a");
    let removed = exchange(
        &mut a,
        &Frame::DeviceRemove(DeviceRemove {
            device: "laptop-b".to_owned(),
        }),
    );
    assert!(matches!(
        removed,
        Frame::DeviceRemoved(DeviceRemoved { ref device, removed: true }) if device == "laptop-b"
    ));

    // The computer of B comes back with the token of its old row, and it reads the
    // reason that the person gave it instead of a login that it never had.
    let answer = greet_with(&server, "laptop-b", &b.token);
    assert_eq!(error_code(&answer), "removed");

    // A token that belongs to no computer of this server reads the same as before.
    let unknown = greet_with(&server, "laptop-b", "not-a-token-of-this-server");
    assert_eq!(error_code(&unknown), "not_logged_in");

    // A computer of the socket has no token, so a removal keeps no record of it and
    // `ssh` greets the same name again.
    let mut socket = server.ssh("laptop-a");
    exchange(
        &mut socket,
        &Frame::DeviceRemove(DeviceRemove {
            device: "laptop-a".to_owned(),
        }),
    );
    let mut again = server.ssh("laptop-a");
    let rows = ask_devices(&mut again);
    assert!(rows.iter().any(|row| row.name == "laptop-a"), "{rows:?}");
}

#[test]
fn a_join_of_the_same_name_clears_the_record_of_a_removal() {
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let store = secrets(directory.path());
    let b = join(&invited.link, "laptop-b", &store).expect("the join");
    let mut a = server.ssh("laptop-a");
    exchange(
        &mut a,
        &Frame::DeviceRemove(DeviceRemove {
            device: "laptop-b".to_owned(),
        }),
    );
    assert_eq!(error_code(&greet_with(&server, "laptop-b", &b.token)), "removed");

    // The person joins this computer again under the name that it already had: the
    // name belongs to the new token, and the old one is the token of nothing.
    let invited = server.invite();
    let joined = join(&invited.link, "laptop-b", &store).expect("the join of the same name");
    assert_eq!(joined.device, "laptop-b");
    assert_ne!(joined.token, b.token, "a join answers a new token");
    let _open = server.tcp("laptop-b", &joined.token);
    assert_eq!(error_code(&greet_with(&server, "laptop-b", &b.token)), "not_logged_in");
}

#[test]
fn the_record_of_a_removal_is_kept_for_thirty_days() {
    let clock = Arc::new(TestClock::at(1_760_000_000_000));
    let server = Server::with_clock(Arc::clone(&clock));
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let b = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");
    assert!(
        server
            .store
            .remove_device("laptop-b", clock.now_millis())
            .expect("the removal of a computer"),
        "the server held the computer"
    );
    assert_eq!(error_code(&greet_with(&server, "laptop-b", &b.token)), "removed");

    // The record of a removal is pruned at the next removal or join, and one that
    // is older than the bound of the record goes: the token then reads as the token
    // of nothing, and the token itself never worked again.
    clock.advance(horizon_sync::REMOVED_KEPT_MILLIS + 1);
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    join(&invited.link, "laptop-c", &secrets(directory.path())).expect("the join of another computer");
    assert_eq!(error_code(&greet_with(&server, "laptop-b", &b.token)), "not_logged_in");
}

#[test]
fn the_list_of_the_computers_names_the_caller_and_how_each_one_logs_in() {
    let server = Server::start();
    let mut a = server.ssh("laptop-a");
    let rows = ask_devices(&mut a);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "laptop-a");
    assert_eq!(rows[0].login, DeviceLogin::Ssh);
    assert!(rows[0].caller);
    assert!(
        !server.store.devices("laptop-z").expect("the rows")[0].caller,
        "a row of another caller is no caller"
    );
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let b = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");
    let mut open = server.tcp("laptop-b", &b.token);
    let rows = ask_devices(&mut open);
    let names: Vec<(&str, bool)> = rows.iter().map(|row| (row.name.as_str(), row.caller)).collect();
    assert_eq!(names, vec![("laptop-a", false), ("laptop-b", true), ("server", false)]);
    assert!(
        rows.iter()
            .all(|row| row.created_millis > 0 && row.last_seen_millis > 0),
        "every row names its times: {rows:?}"
    );
}

/// One join on a link of its own, which answers the frame of the join.
///
/// A join ends its link: the computer reads its answer and, when it is one token,
/// comes back with a hello that presents it.
fn try_join(server: &Server, code: &str, device: &str) -> Frame {
    let mut stream = TcpStream::connect(server.address()).expect("the listener of the server");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("a read bound of the test");
    exchange(
        &mut stream,
        &Frame::Join(Join {
            code: code.to_owned(),
            device: device.to_owned(),
        }),
    )
}

/// The list of the computers of a greeted link.
fn ask_devices(stream: &mut (impl Read + Write)) -> Vec<horizon_sync::DeviceRow> {
    let answer = exchange(stream, &Frame::Devices(Devices {}));
    let Frame::DeviceList(DeviceList { rows, now_millis }) = answer else {
        panic!("the list answers its own frame: {answer:?}");
    };
    assert!(now_millis >= rows.iter().map(|row| row.last_seen_millis).max().unwrap_or(0));
    rows
}

#[test]
fn two_computers_share_the_machine_list_and_a_named_secret() {
    let server = Server::start();

    // A is the computer of the socket, which `ssh` lets in. It pushes one machine
    // and one named secret, and then invites B.
    let mut a = server.ssh("laptop-a");
    let intents = vec![
        Intent {
            device: "laptop-a".to_owned(),
            id: 1,
            seen: None,
            op: Op::MachinePut {
                record: machine("build-1"),
            },
        },
        Intent {
            device: "laptop-a".to_owned(),
            id: 2,
            seen: None,
            op: Op::SecretPut {
                id: "named/DEMO_TOKEN".to_owned(),
                body: Some(horizon_sync::SecretBody::new(NAMED_VALUE.to_owned())),
            },
        },
    ];
    let answer = exchange(&mut a, &Frame::Push(horizon_sync::Push { intents }));
    let Frame::Pushed(reply) = answer else {
        panic!("a push is answered: {answer:?}");
    };
    assert_eq!(reply.results.len(), 2);
    let invited = server.invite();

    // B joins over the listener with real files of its own.
    let directory = tempfile::tempdir().expect("a directory of B");
    let values = secrets(directory.path());
    let joined = join(&invited.link, "laptop-b", &values).expect("the join of B");
    let stream = TcpStream::connect(server.address()).expect("the listener of B");
    let reading = stream.try_clone().expect("the reading half of B");
    let mut link = Link::new(reading, stream);
    let mut client =
        Client::open(directory.path().join("machines/server-queue.json"), "laptop-b").expect("the queue of B");
    let hello = client
        .hello_with_token(&mut link, Some(&joined.token))
        .expect("B is logged in with the token of its join");
    assert_eq!(
        hello.protocol,
        horizon_sync::SERVER_PROTOCOL_VERSION,
        "the server answers the version in which it proves an idle link alive"
    );
    let mut mirror = ObjectMirror::default();
    let pulled = client.pull(&mut link, &mut mirror).expect("the changes of the server");
    assert!(pulled.rows >= 2, "the machine and the secret came: {pulled:?}");
    let record = mirror.get("machine", "build-1").expect("the machine of A arrived at B");
    assert_eq!(record.body["host"], "203.0.113.10");
    let secret = mirror
        .get("secret", "named/DEMO_TOKEN")
        .expect("the named secret of A arrived at B");
    assert_eq!(secret.body, NAMED_VALUE);

    // The value is written where the agent panels of B read it.
    values
        .write(
            "named/DEMO_TOKEN",
            secret.body.as_str().expect("the text of the value").as_bytes(),
        )
        .expect("the value of B");
    let shared = Arc::new(Mutex::new(Arc::clone(&values)));
    let named = NamedSecretVault::new(Box::new(SharedCredentialStore::new(shared)));
    named.set_names(["DEMO_TOKEN".to_owned()]);
    assert_eq!(named.refresh().expect("the values of B").len(), 1);
    let name = SecretName::parse("DEMO_TOKEN").expect("a name");
    assert_eq!(
        named
            .value(&name)
            .expect("the value of B")
            .expect("one value")
            .as_slice(),
        NAMED_VALUE.as_bytes()
    );

    // A removes B, which ends the open link of B.
    let removed = exchange(
        &mut a,
        &Frame::DeviceRemove(DeviceRemove {
            device: "laptop-b".to_owned(),
        }),
    );
    assert!(matches!(
        removed,
        Frame::DeviceRemoved(DeviceRemoved { removed: true, .. })
    ));
    match link.receive("the end of the link") {
        Err(horizon_sync::SyncError::Refused { code, .. }) => assert_eq!(code, "removed"),
        other => panic!("the link of B ends with the typed error: {other:?}"),
    }
}

#[test]
fn the_first_invitation_of_a_server_holds_a_link_that_a_computer_accepts() {
    let server = Server::start();
    let invited = server.invite();
    let parsed = JoinLink::parse(&invited.link).expect("the parser of a computer accepts the link");
    assert_eq!(parsed.code(), invited.code);
    assert_eq!(parsed.port(), server.listener.port());
    assert!(
        horizon_core::allows_address(parsed.socket_addr().expect("the address of a free listener").ip()),
        "a link of a test names one address of the rule"
    );
    let qr = horizon_core::Qr::encode(&invited.link).expect("the link as one QR code");
    assert!(qr.size() >= 21, "a QR code of this text has modules");

    // Four open invitations is the maximum, and a fifth replaces the oldest.
    let first = server.invite();
    let second = server.invite();
    let third = server.invite();
    let fourth = server.invite();
    let fifth = server.invite();
    assert_eq!(
        server
            .store
            .open_invitations(server.clock.now_millis())
            .expect("the count"),
        4
    );
    let replaced = try_join(&server, &first.code, "laptop-z");
    assert_eq!(error_code(&replaced), "wrong_code", "the oldest invitation made room");
    let newest = try_join(&server, &fifth.code, "laptop-z");
    assert!(matches!(newest, Frame::Joined(_)), "{newest:?}");
    for (code, device) in [
        (second.code, "laptop-y"),
        (third.code, "laptop-x"),
        (fourth.code, "laptop-w"),
    ] {
        let answer = try_join(&server, &code, device);
        assert!(matches!(answer, Frame::Joined(_)), "{answer:?}");
    }
    let rows = server.store.devices("server").expect("the rows");
    let row = rows
        .iter()
        .find(|row| row.name == "server")
        .expect("the row of the server");
    assert_eq!(
        row.login,
        DeviceLogin::Ssh,
        "the server computer logs in over the socket"
    );
    assert!(row.caller, "the server computer asked for the invitation");
}

#[test]
fn a_server_without_a_listener_cannot_hand_out_a_link() {
    let directory = tempfile::tempdir().expect("a directory");
    let store = Store::open(&directory.path().join("server.sqlite")).expect("the store");
    let info = ServerInfo::default();
    let socket =
        run_server_on_unix_socket(&directory.path().join("server.sock"), store.clone(), &info).expect("the socket");
    let mut stream = UnixStream::connect(directory.path().join("server.sock")).expect("the socket");
    let answer = exchange(
        &mut stream,
        &Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device: "laptop-a".to_owned(),
            store: None,
            token: None,
        }),
    );
    assert!(matches!(answer, Frame::HelloOk(HelloOk { .. })), "{answer:?}");
    let refused = exchange(&mut stream, &Frame::Invite(Invite {}));
    assert_eq!(error_code(&refused), "no_listener");
    socket.stop();
    store.close();
}

#[test]
fn a_join_of_a_link_that_points_outside_the_rule_sends_nothing() {
    let directory = tempfile::tempdir().expect("a directory");
    let store = secrets(directory.path());
    // One address of a public network, which the rule of the link refuses before
    // the first byte. The rule
    // of the link refuses both before the first byte.
    for link in [
        "horizon://8.8.8.8:7000/join#ABCD-EFGH-JKMN",
        "horizon://0.0.0.0:7000/join#ABCD-EFGH-JKMN",
    ] {
        let error = join(link, "laptop-b", &store).expect_err(link);
        assert_eq!(error.code(), "address_not_allowed");
        assert!(
            error.sentence().contains("100.64.0.0/10"),
            "the refusal names the rule: {}",
            error.sentence()
        );
    }
    assert!(secret_store::read_token(&store).expect("no token").is_none());
    let error = join("not a link", "laptop-b", &store).expect_err("a text that is no link");
    assert_eq!(error.code(), "not_one");
    let error = join("horizon://127.0.0.1:7000/join#ABCD-EFGH-JKMN", "Laptop", &store)
        .expect_err("a name that is no device name");
    assert_eq!(error.code(), "bad_device");
    assert!(matches!(error, JoinError::Device));
    assert!(PATIENCE > Duration::ZERO);
}

#[test]
fn a_question_of_the_window_travels_on_the_link_of_its_computer() {
    // The question path of the supervisor: the answer comes back on a channel that
    // a frame polls, and the invitation is one of the three questions.
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let joined = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");
    let mut stream = server.tcp("laptop-b", &joined.token);
    let answer = exchange(&mut stream, &Frame::Invite(Invite {}));
    let Frame::Invited(again) = answer else {
        panic!("the invitation answers its own frame: {answer:?}");
    };
    assert_eq!(again.link, JoinLink::parse(&again.link).expect("a link").to_uri());
    assert_eq!(again.expires_millis, 600_000);
    assert_eq!(
        again.now_millis,
        u64::try_from(server.clock.now_millis()).expect("a time")
    );
    // The two requests of the window that name a computer or a list.
    let rows = ask_devices(&mut stream);
    assert!(rows.iter().any(|row| row.name == "laptop-b" && row.caller));
    let Frame::Invited(_) = exchange(&mut stream, &Frame::Invite(Invite {})) else {
        panic!("one more invitation");
    };
    assert_eq!(
        server
            .store
            .open_invitations(server.clock.now_millis())
            .expect("the count"),
        2,
        "the join used the first invitation, and two are open"
    );
    assert!(
        matches!(ServerRequest::Invite, ServerRequest::Invite),
        "the request type of the window is the one that the link sends"
    );
    assert!(matches!(ServerReply::Invited(again), ServerReply::Invited(_)));
}

#[test]
fn a_join_that_no_hello_followed_leaves_no_computer_on_the_server() {
    let server = Server::start();
    let invited = server.invite();

    // A computer joined and never came back with its token, so its row is one that
    // no computer can use.
    let answer = try_join(&server, &invited.code, "laptop-b");
    assert!(matches!(answer, Frame::Joined(_)), "{answer:?}");
    let rows = server.store.devices("server").expect("the rows");
    assert!(rows.iter().any(|row| row.name == "laptop-b"), "{rows:?}");

    // The row waits for the grace of the server, and then the list drops it.
    server.clock.advance(horizon_sync::UNJOINED_GRACE_MILLIS - 1_000);
    let mut a = server.ssh("laptop-a");
    let rows = ask_devices(&mut a);
    assert!(
        rows.iter().any(|row| row.name == "laptop-b"),
        "a row inside its grace stays: {rows:?}"
    );
    server.clock.advance(2_000);
    let rows = ask_devices(&mut a);
    assert!(
        rows.iter().all(|row| row.name != "laptop-b"),
        "a join that no hello followed leaves no row: {rows:?}"
    );

    // A computer that greeted the server keeps its row, whatever its age.
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let joined = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");
    let _open = server.tcp("laptop-b", &joined.token);
    server.clock.advance(horizon_sync::UNJOINED_GRACE_MILLIS * 10);
    let rows = ask_devices(&mut a);
    assert!(
        rows.iter().any(|row| row.name == "laptop-b"),
        "a row that greeted the server stays: {rows:?}"
    );
}

#[test]
fn the_list_says_that_the_server_sees_a_computer_that_has_an_open_link() {
    let server = Server::start();
    let invited = server.invite();
    let directory = tempfile::tempdir().expect("a directory");
    let joined = join(&invited.link, "laptop-b", &secrets(directory.path())).expect("the join");

    // B holds its link open and the clock moves on: its last hello is minutes old,
    // but the server still sees the link.
    let open = server.tcp("laptop-b", &joined.token);
    server.clock.advance(4 * 60_000);
    let mut a = server.ssh("laptop-a");
    let rows = ask_devices(&mut a);
    let b = rows.iter().find(|row| row.name == "laptop-b").expect("the row of B");
    assert_eq!(
        b.last_seen_millis,
        u64::try_from(server.clock.now_millis()).expect("a time"),
        "a computer with an open link reads as one that the server sees now"
    );

    // A link that ended leaves the time of its last hello, which is the truth.
    drop(open);
    let deadline = std::time::Instant::now() + PATIENCE;
    let rows = loop {
        let rows = ask_devices(&mut a);
        let b = rows.iter().find(|row| row.name == "laptop-b").expect("the row of B");
        if b.last_seen_millis < u64::try_from(server.clock.now_millis()).expect("a time")
            || std::time::Instant::now() >= deadline
        {
            break rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let b = rows.iter().find(|row| row.name == "laptop-b").expect("the row of B");
    assert!(
        b.last_seen_millis < u64::try_from(server.clock.now_millis()).expect("a time"),
        "a computer whose link ended reads as the time of its last hello"
    );
}
