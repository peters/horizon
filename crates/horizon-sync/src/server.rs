//! One link of one computer: the frames of a connection after its greeting.
//!
//! The server reads a frame, answers it, and never blocks on a slow answer: every
//! frame that goes out passes through one writing thread of the connection, so a
//! [`Wait`] that is still open cannot hold a push. The device name of the greeting
//! is the name of the link, and an intent that names another computer is refused,
//! so one computer cannot spend the intent counters of another.
//!
//! Two transports greet a link:
//!
//! - A link of the Unix socket of the server sends a `hello` and nothing else,
//!   because `ssh` is its login.
//! - A link of the TCP listener sends either a `join` with the code of an
//!   invitation, which answers one token and ends the link, or a `hello` with the
//!   token that a join gave it.
//!
//! Limits of one link: a frame of at most 256 KiB, 256 intents in one push, and 32
//! open waits. The first two close the link with a typed error, because the frame
//! breaks the contract of the protocol; the third answers that one wait and keeps
//! the link.
//!
//! A link whose first request is the `open` of [`crate::live`] is a live link
//! from then on, and the loop of that module serves it. No live frame travels on
//! a link that carries the store.

use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use horizon_core::server_link::normalize_code;

use crate::clock::{Clock, SystemClock};
use crate::error::StoreError;
use crate::frame::{FrameError, FrameWriter, read_frame, write_frame};
use crate::intent::MAX_INTENTS_PER_PUSH;
use crate::listener::ListenerInfo;
use crate::login;
use crate::protocol::{
    DeviceList, DeviceRemoved, Frame, Hello, HelloOk, Invited, Join, Joined, LinkError, LinkErrorCode,
    SERVER_PROTOCOL_VERSION, check_device_name,
};
use crate::store::{JoinAnswer, Store, TokenLogin};

/// The most waits that one link holds open at once.
pub const MAX_OPEN_WAITS: usize = 32;

/// How long a wait stays open without a change.
///
/// The answer of a wait that no change reached is the proof that the link is alive,
/// so this bound is also how long an idle computer may believe in a server program
/// that stopped answering. Fifteen seconds let a computer that reads the version of
/// this server (`SERVER_PROTOCOL_VERSION`) prove its link within about twenty
/// seconds, and cost two small frames for every fifteen seconds of an idle link.
pub const WAIT_BOUND: Duration = Duration::from_secs(15);

/// How long a wrong code waits before the server answers it, so a computer that
/// guesses codes spends a second on each one.
pub const WRONG_CODE_DELAY: Duration = Duration::from_secs(1);

/// How often an open wait looks at the link before it gives up, so a link that
/// ends does not keep a thread for its whole bound.
const WAIT_SLICE: Duration = Duration::from_millis(200);

/// How long the server waits for the error frame of an ended link to reach the
/// computer before it closes the transport.
const END_FLUSH: Duration = Duration::from_secs(1);

/// The one sentence of a computer that a person removed: the end of its open link
/// and the answer to its later greeting read the same.
const REMOVED_TEXT: &str = "This computer was removed from the server.";

/// What a server process knows about itself: the listener that it opened, the
/// links that are open, and its clock.
///
/// It is one value of the process, so both transports answer from the same store,
/// the same registry and the same clock.
pub struct ServerInfo {
    /// The listener of this server, when `--listen` gave one. A link is only
    /// returned when it is there, and `invite` answers `no_listener` without it.
    pub listener: Option<ListenerInfo>,
    /// The links that are open, so a removal ends them.
    pub registry: Arc<LinkRegistry>,
    /// The time and the sleep of the server. A test replaces them.
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ServerInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerInfo")
            .field("listener", &self.listener)
            .field("links", &self.registry.len())
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            listener: None,
            registry: Arc::new(LinkRegistry::default()),
            clock: Arc::new(SystemClock),
        }
    }
}

impl ServerInfo {
    /// A server with the listener of `--listen`, one registry and the system clock.
    #[must_use]
    pub fn with_listener(listener: Option<ListenerInfo>) -> Self {
        Self {
            listener,
            ..Self::default()
        }
    }

    /// A server with no listener and the clock of a test.
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            ..Self::default()
        }
    }

    /// The time of this server.
    #[must_use]
    pub fn now_millis(&self) -> i64 {
        self.clock.now_millis()
    }
}

/// The transport of one link, as the server can end it from another thread.
pub trait LinkTransport: Send + Sync {
    /// Closes both directions, so a read that waits returns at once.
    fn shutdown(&self);
}

impl LinkTransport for std::os::unix::net::UnixStream {
    fn shutdown(&self) {
        let _ = std::os::unix::net::UnixStream::shutdown(self, std::net::Shutdown::Both);
    }
}

impl LinkTransport for std::net::TcpStream {
    fn shutdown(&self) {
        let _ = std::net::TcpStream::shutdown(self, std::net::Shutdown::Both);
    }
}

/// A transport that has no side to close, for a test that gives two streams.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTransport;

impl LinkTransport for NoTransport {
    fn shutdown(&self) {}
}

/// One frame that the writing thread of a link must write.
pub(crate) struct Outgoing {
    pub(crate) frame: Frame,
    /// Told when the frame reached the transport, so a removal can wait for its
    /// error frame before it closes the link.
    pub(crate) written: Option<mpsc::SyncSender<()>>,
}

/// One open link of one device, so the server can end it.
pub struct LinkTicket {
    pub(crate) id: u64,
    pub(crate) device: String,
    frames: mpsc::Sender<Outgoing>,
    closed: Arc<AtomicBool>,
    transport: Arc<dyn LinkTransport>,
}

impl std::fmt::Debug for LinkTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinkTicket")
            .field("id", &self.id)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl LinkTicket {
    /// Ends this link with one typed error, and closes its transport.
    ///
    /// The error frame is written first, so the computer reads the reason before
    /// its side of the link ends.
    pub fn end(&self, code: LinkErrorCode, message: &str) {
        let (written, waiting) = mpsc::sync_channel(1);
        let frame = error_frame(code, message);
        if self
            .frames
            .send(Outgoing {
                frame,
                written: Some(written),
            })
            .is_ok()
        {
            let _ = waiting.recv_timeout(END_FLUSH);
        }
        self.shut();
    }

    /// Closes this link at once and without an error frame, for a link whose
    /// writing side failed.
    pub(crate) fn shut(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.transport.shutdown();
    }
}

/// The links of one server that are open now, so a removal ends the links of the
/// computer that it removes, and the hub of its live links, which every link of
/// the server shares.
#[derive(Default)]
pub struct LinkRegistry {
    next: AtomicU64,
    open: Mutex<BTreeMap<u64, Arc<LinkTicket>>>,
    pub(crate) live: crate::live::LiveHub,
}

impl std::fmt::Debug for LinkRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinkRegistry")
            .field("links", &self.len())
            .finish()
    }
}

impl LinkRegistry {
    /// The number of links that are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether no link is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Registers one open link, and answers a ticket that ends it.
    pub(crate) fn add(
        &self,
        device: &str,
        frames: mpsc::Sender<Outgoing>,
        closed: Arc<AtomicBool>,
        transport: Arc<dyn LinkTransport>,
    ) -> Arc<LinkTicket> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let ticket = Arc::new(LinkTicket {
            id,
            device: device.to_owned(),
            frames,
            closed,
            transport,
        });
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::clone(&ticket));
        ticket
    }

    /// Forgets one link that ended.
    pub fn close(&self, id: u64) {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }

    /// The names of the computers that have an open link now, so the list of the
    /// computers can say that the server sees them.
    #[must_use]
    pub fn devices(&self) -> std::collections::BTreeSet<String> {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|ticket| ticket.device.clone())
            .collect()
    }

    /// Ends every open link of `device`, except the link `except`, with one typed
    /// error.
    pub fn end_device(&self, device: &str, except: Option<u64>, code: LinkErrorCode, message: &str) {
        let ending: Vec<Arc<LinkTicket>> = {
            let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
            open.values()
                .filter(|ticket| ticket.device == device && Some(ticket.id) != except)
                .map(Arc::clone)
                .collect()
        };
        for ticket in ending {
            ticket.end(code, message);
        }
    }
}

/// Answers one connection of the Unix socket until its computer ends it.
///
/// # Errors
/// Returns why the link failed, for the caller to log. A computer that ends its
/// link answers `Ok(())`.
pub fn serve<R, W>(reader: R, writer: W, store: &Store) -> Result<(), FrameError>
where
    R: Read,
    W: FrameWriter + Send + 'static,
{
    run(
        reader,
        writer,
        store,
        &ServerInfo::default(),
        Login::Ssh,
        Arc::new(NoTransport),
    )
}

/// Answers one connection of the Unix socket of the server of `info`.
///
/// The link of a socket has no token: `ssh` let its computer in, so its greeting
/// is a `hello` and nothing else.
///
/// # Errors
/// Returns why the link failed.
pub fn serve_ssh<R, W>(
    reader: R,
    writer: W,
    store: &Store,
    info: &ServerInfo,
    transport: Arc<dyn LinkTransport>,
) -> Result<(), FrameError>
where
    R: Read,
    W: FrameWriter + Send + 'static,
{
    run(reader, writer, store, info, Login::Ssh, transport)
}

/// Answers one connection of the TCP listener of `info`.
///
/// The first frame of the link is a `join` with the code of an invitation, or a
/// `hello` with the token of one device.
///
/// # Errors
/// Returns why the link failed.
pub fn serve_tcp<R, W>(
    reader: R,
    writer: W,
    store: &Store,
    info: &ServerInfo,
    transport: Arc<dyn LinkTransport>,
) -> Result<(), FrameError>
where
    R: Read,
    W: FrameWriter + Send + 'static,
{
    run(reader, writer, store, info, Login::Token, transport)
}

/// How a link proves the name of its computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Login {
    /// The socket of the server, where `ssh` is the login and a token is absent.
    Ssh,
    /// The TCP listener, where the token of a join is the login.
    Token,
}

/// The name of a computer that a greeting proved, and the ticket that ends its
/// link.
struct Identity {
    device: String,
    ticket: Arc<LinkTicket>,
}

fn run<R, W>(
    mut reader: R,
    writer: W,
    store: &Store,
    info: &ServerInfo,
    login: Login,
    transport: Arc<dyn LinkTransport>,
) -> Result<(), FrameError>
where
    R: Read,
    W: FrameWriter + Send + 'static,
{
    let (frames, outgoing) = mpsc::channel::<Outgoing>();
    let closed = Arc::new(AtomicBool::new(false));
    let writing = {
        let closed = Arc::clone(&closed);
        thread::Builder::new()
            .name("horizon-sync-link".to_owned())
            .spawn(move || {
                let mut writer = writer;
                for out in outgoing {
                    if write_frame(&mut writer, &out.frame).is_err() {
                        closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    if let Some(written) = out.written {
                        let _ = written.send(());
                    }
                }
                // The writing side ends first: the computer reads the last frame,
                // and then the end of the link.
                let _ = writer.finish();
                closed.store(true, Ordering::SeqCst);
            })
            .map_err(|error| FrameError::Io(std::io::Error::other(format!("the link thread: {error}"))))?
    };
    let result = exchange(&mut reader, &frames, store, info, &closed, login, transport);
    closed.store(true, Ordering::SeqCst);
    drop(frames);
    let _ = writing.join();
    result
}

/// The frames of one link, until the computer ends it or breaks the protocol.
fn exchange<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Outgoing>,
    store: &Store,
    info: &ServerInfo,
    closed: &Arc<AtomicBool>,
    login: Login,
    transport: Arc<dyn LinkTransport>,
) -> Result<(), FrameError> {
    let Some(identity) = greet(reader, frames, store, info, closed, login, transport)? else {
        return Ok(());
    };
    let device = identity.device.clone();
    let ticket = identity.ticket;
    let result = converse(reader, frames, store, info, closed, &device, &ticket);
    info.registry.close(ticket.id);
    result
}

/// The frame loop of one greeted link.
fn converse<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Outgoing>,
    store: &Store,
    info: &ServerInfo,
    closed: &Arc<AtomicBool>,
    device: &str,
    ticket: &Arc<LinkTicket>,
) -> Result<(), FrameError> {
    let waits = Arc::new(AtomicUsize::new(0));
    let mut fresh = true;
    loop {
        let Some(frame) = next_frame(reader, frames)? else {
            return Ok(());
        };
        let first = std::mem::replace(&mut fresh, false);
        match frame {
            Frame::Live(request) => {
                return crate::live::serve(reader, frames, &info.registry, ticket, request, first);
            }
            Frame::Push(push) => {
                if push.intents.len() > MAX_INTENTS_PER_PUSH {
                    post(
                        frames,
                        error_frame(
                            LinkErrorCode::TooManyIntents,
                            &format!("one push holds at most {MAX_INTENTS_PER_PUSH} intents"),
                        ),
                    );
                    return Ok(());
                }
                match store.push(device, &push.intents) {
                    Ok(reply) => post(frames, Frame::Pushed(reply)),
                    Err(error) => {
                        tracing::warn!(%error, "the server could not apply a push");
                        post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                    }
                }
            }
            Frame::Pull(pull) => match store.pull(pull.since, &pull.tip) {
                Ok(changes) => post(frames, Frame::Changes(changes)),
                Err(error) => {
                    tracing::warn!(%error, "the server could not read a pull");
                    post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                }
            },
            Frame::Wait(wait) => open_wait(store, frames, closed, &waits, wait.since),
            Frame::Invite(_) => invite(frames, store, info),
            Frame::Devices(_) => device_list(frames, store, info, device),
            Frame::DeviceRemove(remove) => {
                if !remove_device(frames, store, info, device, ticket, &remove.device) {
                    return Ok(());
                }
            }
            Frame::Hello(_)
            | Frame::HelloOk(_)
            | Frame::Join(_)
            | Frame::Joined(_)
            | Frame::Invited(_)
            | Frame::DeviceList(_)
            | Frame::DeviceRemoved(_)
            | Frame::Pushed(_)
            | Frame::Changes(_)
            | Frame::LiveEvent(_)
            | Frame::Error(_) => {
                post(frames, error_frame(LinkErrorCode::BadFrame, not_a_request()));
                return Ok(());
            }
        }
    }
}

/// The sentence of a frame that a link does not send to a server.
fn not_a_request() -> &'static str {
    "a link carries one greeting, then pushes, pulls, waits, an invitation and the list of its computers"
}

/// Makes one invitation and answers it with the link that holds its code.
fn invite(frames: &mpsc::Sender<Outgoing>, store: &Store, info: &ServerInfo) {
    let Some(listener) = info.listener.as_ref() else {
        post(
            frames,
            error_frame(
                LinkErrorCode::NoListener,
                "This server has no listener, so it cannot hand out a link. Start it with --listen.",
            ),
        );
        return;
    };
    let now = info.now_millis();
    let made = login::new_code()
        .and_then(|code| login::hash_code(&code).map(|hash| (code, hash)))
        .and_then(|(code, hash)| store.invite(&hash, now).map(|expires| (code, expires)));
    match made {
        Ok((code, expires_millis)) => {
            let link = match listener.join_link(&code) {
                Ok(link) => link,
                Err(error) => {
                    tracing::warn!(%error, "the server cannot build the link of an invitation");
                    post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                    return;
                }
            };
            let expires_millis = u64::try_from(expires_millis.saturating_sub(now)).unwrap_or(0);
            post(
                frames,
                Frame::Invited(Invited {
                    code,
                    link,
                    expires_millis,
                    now_millis: u64::try_from(now).unwrap_or(0),
                }),
            );
        }
        Err(error) => {
            tracing::warn!(%error, "the server could not make an invitation");
            post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
        }
    }
}

/// Answers the list of the computers of this server.
///
/// A computer that has an open link is a computer that the server sees now, so the
/// list stamps the time of the answer on it instead of the time of its last hello.
fn device_list(frames: &mpsc::Sender<Outgoing>, store: &Store, info: &ServerInfo, caller: &str) {
    let now = info.now_millis();
    match store.prune_unjoined(now) {
        Ok(0) => {}
        Ok(pruned) => tracing::info!(pruned, "a join that no hello followed left the server"),
        Err(error) => tracing::warn!(%error, "the server could not drop a join that no hello followed"),
    }
    let now_millis = u64::try_from(now).unwrap_or(0);
    let open = info.registry.devices();
    match store.devices(caller) {
        Ok(mut rows) => {
            for row in &mut rows {
                if open.contains(&row.name) {
                    row.last_seen_millis = row.last_seen_millis.max(now_millis);
                }
            }
            post(frames, Frame::DeviceList(DeviceList { rows, now_millis }));
        }
        Err(error) => {
            tracing::warn!(%error, "the server could not read its computers");
            post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
        }
    }
}

/// Removes one computer and ends its links. Answers whether the loop of this link
/// continues: a computer that removes itself reads its answer and its link ends.
fn remove_device(
    frames: &mpsc::Sender<Outgoing>,
    store: &Store,
    info: &ServerInfo,
    device: &str,
    ticket: &Arc<LinkTicket>,
    target: &str,
) -> bool {
    let removed = match store.remove_device(target, info.now_millis()) {
        Ok(removed) => removed,
        Err(error) => {
            tracing::warn!(%error, "the server could not remove a computer");
            post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
            return true;
        }
    };
    tracing::info!(device = %target, removed, "a computer was removed from the server");
    post(
        frames,
        Frame::DeviceRemoved(DeviceRemoved {
            device: target.to_owned(),
            removed,
        }),
    );
    if !removed {
        return true;
    }
    info.registry
        .end_device(target, Some(ticket.id), LinkErrorCode::Removed, REMOVED_TEXT);
    target != device
}

/// The greeting of a link: the name of its computer, and the ticket that ends it.
fn greet<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Outgoing>,
    store: &Store,
    info: &ServerInfo,
    closed: &Arc<AtomicBool>,
    login: Login,
    transport: Arc<dyn LinkTransport>,
) -> Result<Option<Identity>, FrameError> {
    let Some(first) = next_frame(reader, frames)? else {
        return Ok(None);
    };
    let device = match (first, login) {
        (Frame::Join(join), Login::Token) => {
            accept_join(frames, store, info, &join);
            // A join ends its link: the computer reads the token and comes back
            // with a hello that presents it.
            return Ok(None);
        }
        (Frame::Hello(hello), login) => match accept_hello(frames, store, info, &hello, login) {
            Some(device) => device,
            None => return Ok(None),
        },
        (_first, Login::Ssh) => {
            post(
                frames,
                error_frame(LinkErrorCode::NotHello, "the first frame of a link is a hello"),
            );
            return Ok(None);
        }
        (_first, Login::Token) => {
            post(
                frames,
                error_frame(
                    LinkErrorCode::NotHello,
                    "the first frame of a link of the listener is a join or a hello",
                ),
            );
            return Ok(None);
        }
    };
    let ticket = info
        .registry
        .add(&device, frames.clone(), Arc::clone(closed), transport);
    Ok(Some(Identity { device, ticket }))
}

/// Answers the greet of a computer that presents a token.
fn accept_hello(
    frames: &mpsc::Sender<Outgoing>,
    store: &Store,
    info: &ServerInfo,
    hello: &Hello,
    login: Login,
) -> Option<String> {
    if hello.protocol > SERVER_PROTOCOL_VERSION {
        post(
            frames,
            error_frame(
                LinkErrorCode::ProtocolVersion,
                &format!(
                    "the computer speaks protocol {}, and this server speaks protocol {SERVER_PROTOCOL_VERSION}",
                    hello.protocol
                ),
            ),
        );
        return None;
    }
    if !check_device_name(&hello.device) {
        post(
            frames,
            error_frame(
                LinkErrorCode::BadDevice,
                "the device name is 1 to 40 characters of a-z, 0-9 and -",
            ),
        );
        return None;
    }
    if login == Login::Token {
        let Some(token) = hello.token.as_deref() else {
            post(frames, error_frame(LinkErrorCode::NotLoggedIn, not_logged_in()));
            return None;
        };
        let hash = login_hash(token);
        match store.present_token(&hash, &hello.device, info.now_millis()) {
            Ok(TokenLogin::Present) => {}
            Ok(TokenLogin::Removed) => {
                post(frames, error_frame(LinkErrorCode::Removed, REMOVED_TEXT));
                return None;
            }
            Ok(TokenLogin::Unknown) => {
                post(frames, error_frame(LinkErrorCode::NotLoggedIn, not_logged_in()));
                return None;
            }
            Err(error) => {
                tracing::warn!(%error, "the server could not check the token of a computer");
                post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                return None;
            }
        }
    }
    let login = match login {
        Login::Ssh => "ssh",
        Login::Token => "token",
    };
    match store.touch_device(&hello.device, login, info.now_millis()) {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(%error, "the server could not write the row of a computer");
            post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
            return None;
        }
    }
    let answer = HelloOk {
        protocol: SERVER_PROTOCOL_VERSION,
        store: store.store_id().to_owned(),
        now_millis: u64::try_from(info.now_millis()).unwrap_or(0),
        secrets_lost: store.secrets_lost(),
    };
    post(frames, Frame::HelloOk(answer));
    Some(hello.device.clone())
}

/// The hash of a token, which the server compares with the hash of one row.
fn login_hash(token: &str) -> String {
    login::hash_token(token)
}

/// The one sentence of a link that presents no token of this server. A missing
/// token, an unknown one and the token of another device read the same, so the
/// answer tells nothing about the computers of this server.
fn not_logged_in() -> &'static str {
    "This computer is not logged in to this server. Join it with the code of an invitation from a computer that is logged in."
}

/// Answers the join of a computer with the code of an invitation.
fn accept_join(frames: &mpsc::Sender<Outgoing>, store: &Store, info: &ServerInfo, join: &Join) {
    if !check_device_name(&join.device) {
        post(
            frames,
            error_frame(
                LinkErrorCode::BadDevice,
                "the device name is 1 to 40 characters of a-z, 0-9 and -",
            ),
        );
        return;
    }
    let Ok(code) = normalize_code(&join.code) else {
        wrong_code(frames, store, info);
        return;
    };
    let Ok(hash) = login::hash_code(&code) else {
        post(
            frames,
            error_frame(LinkErrorCode::StoreFailed, "the server cannot hash a code"),
        );
        return;
    };
    let Ok(token) = login::new_token() else {
        post(
            frames,
            error_frame(LinkErrorCode::StoreFailed, "the server cannot make a token"),
        );
        return;
    };
    match store.join(&hash, &join.device, &login::hash_token(&token), info.now_millis()) {
        Ok(JoinAnswer::Joined) => post(
            frames,
            Frame::Joined(Joined {
                device: join.device.clone(),
                token,
            }),
        ),
        Ok(JoinAnswer::WrongCode) => {
            // The store counted this wrong code and closed the door when it was
            // the twentieth, so this answer is the delay of one wrong code.
            info.clock.sleep(WRONG_CODE_DELAY);
            post(frames, error_frame(LinkErrorCode::WrongCode, wrong_code_text()));
        }
        Ok(JoinAnswer::Locked) => post(frames, error_frame(LinkErrorCode::JoinLocked, join_locked_text())),
        Ok(JoinAnswer::NameTaken) => post(
            frames,
            error_frame(
                LinkErrorCode::NameTaken,
                "This server already has a computer with that name. Remove the old one first.",
            ),
        ),
        Err(error) => {
            tracing::warn!(%error, "the server could not use an invitation");
            post(frames, error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
        }
    }
}

/// Counts one code that is not 12 characters, and answers it at the same delay as
/// a code that is not one invitation, so the two read the same.
fn wrong_code(frames: &mpsc::Sender<Outgoing>, store: &Store, info: &ServerInfo) {
    let locked = match store.record_wrong_code(info.now_millis()) {
        Ok(locked) => locked,
        Err(error) => {
            tracing::warn!(%error, "the server could not count a wrong code");
            false
        }
    };
    info.clock.sleep(WRONG_CODE_DELAY);
    if locked {
        post(frames, error_frame(LinkErrorCode::JoinLocked, join_locked_text()));
    } else {
        post(frames, error_frame(LinkErrorCode::WrongCode, wrong_code_text()));
    }
}

fn wrong_code_text() -> &'static str {
    "The code is not one open invitation of this server."
}

fn join_locked_text() -> &'static str {
    "Too many wrong codes. Every invitation is gone until a computer that is logged in makes a new one."
}

/// Reads one frame, and answers a frame that the server cannot read with the typed
/// error of the protocol before the link ends.
pub(crate) fn next_frame<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Outgoing>,
) -> Result<Option<Frame>, FrameError> {
    match read_frame(reader) {
        Ok(Some(frame)) => Ok(Some(frame)),
        Ok(None) => Ok(None),
        Err(error) => {
            post(frames, error_frame(error.code(), &error.to_string()));
            Err(error)
        }
    }
}

/// Opens one wait on its own thread, unless the link holds the maximum.
fn open_wait(
    store: &Store,
    frames: &mpsc::Sender<Outgoing>,
    closed: &Arc<AtomicBool>,
    waits: &Arc<AtomicUsize>,
    since: u64,
) {
    if waits.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN_WAITS {
        waits.fetch_sub(1, Ordering::SeqCst);
        post(
            frames,
            error_frame(
                LinkErrorCode::TooManyWaits,
                &format!("one link holds at most {MAX_OPEN_WAITS} open waits"),
            ),
        );
        return;
    }
    let store = store.clone();
    let replying = frames.clone();
    let stopping = Arc::clone(closed);
    let counting = Arc::clone(waits);
    let spawned = thread::Builder::new()
        .name("horizon-sync-wait".to_owned())
        .spawn(move || {
            let answer = wait_for_change(&store, since, &stopping);
            counting.fetch_sub(1, Ordering::SeqCst);
            let frame = match answer {
                Ok(Some(changes)) => Frame::Changes(changes),
                Ok(None) => return,
                Err(error) => error_frame(LinkErrorCode::StoreFailed, &error.to_string()),
            };
            post(&replying, frame);
        });
    if spawned.is_err() {
        waits.fetch_sub(1, Ordering::SeqCst);
        post(
            frames,
            error_frame(LinkErrorCode::StoreFailed, "the server could not open a wait"),
        );
    }
}

/// Waits for a change after `since` within the bound of the link, and answers
/// `None` when the link ended first.
fn wait_for_change(
    store: &Store,
    since: u64,
    closed: &AtomicBool,
) -> Result<Option<crate::store::Changes>, StoreError> {
    let deadline = Instant::now() + WAIT_BOUND;
    loop {
        if closed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let changes = if remaining.is_zero() {
            store.changes(since)?
        } else {
            store.wait(since, remaining.min(WAIT_SLICE))?
        };
        if changes.index > since || remaining.is_zero() {
            return Ok(Some(changes));
        }
    }
}

/// Sends one frame to the writing thread of this link.
pub(crate) fn post(frames: &mpsc::Sender<Outgoing>, frame: Frame) {
    let _ = frames.send(Outgoing { frame, written: None });
}

pub(crate) fn error_frame(code: LinkErrorCode, message: &str) -> Frame {
    Frame::Error(LinkError::new(code, message))
}
