//! One receiver connection: an I/O thread that owns the TLS stream, answers
//! heartbeats, correlates replies by `requestId` and queues unsolicited events.
use crate::{
    Error, Result, channel,
    proto::{self, CastMessage, Payload},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::{ErrorKind, Read, Write},
    net::SocketAddr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(crate) const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub(crate) const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub(crate) const SENDER_ID: &str = "sender-0";
pub(crate) const PLATFORM_RECEIVER: &str = "receiver-0";

const POLL: Duration = Duration::from_millis(25);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Receivers answer every PING, so this much silence means the link is gone.
const IDLE_LIMIT: Duration = Duration::from_secs(20);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_BACKLOG: usize = 256;
const EVENT_BACKLOG: usize = 64;
type MediaClock = Arc<dyn Fn() -> Option<f64> + Send + Sync>;

/// A message the receiver sent without a matching request, such as a media
/// status change or an application closing its virtual connection.
#[derive(Clone, Debug)]
pub struct Event {
    pub namespace: String,
    pub source: String,
    pub payload: Value,
    /// Progressive media time, in seconds, when this event was queued.
    /// `None` when the session has no media clock. A buffer that is handled
    /// later still opens at this time: the source has moved, the receiver
    /// clock has not.
    pub media_time: Option<f64>,
}

impl Event {
    #[must_use]
    pub fn kind(&self) -> Option<&str> {
        self.payload.get("type").and_then(Value::as_str)
    }
}

enum Command {
    Send(Vec<u8>),
    Close,
}

struct Shared {
    pending: Mutex<HashMap<u64, SyncSender<Value>>>,
    connected: Mutex<HashSet<String>>,
    next_request: AtomicU64,
    open: AtomicBool,
    /// Sampled when an unsolicited event is queued, not when the host handles it.
    media_clock: Mutex<Option<MediaClock>>,
}

pub struct CastClient {
    address: SocketAddr,
    commands: SyncSender<Command>,
    shared: Arc<Shared>,
    events: Mutex<Receiver<Event>>,
    worker: Option<JoinHandle<()>>,
}

impl CastClient {
    /// Connects to a receiver and opens the platform virtual connection.
    /// # Errors
    /// Returns an error if the TCP or TLS connection fails.
    pub fn connect(address: SocketAddr) -> Result<Self> {
        let stream = channel::connect(address, POLL)?;
        let (commands, command_queue) = mpsc::sync_channel(COMMAND_BACKLOG);
        let (event_sink, events) = mpsc::sync_channel(EVENT_BACKLOG);
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            connected: Mutex::new(HashSet::new()),
            next_request: AtomicU64::new(1),
            open: AtomicBool::new(true),
            media_clock: Mutex::new(None),
        });
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("chromecast-io".to_owned())
            .spawn(move || run(stream, command_queue, &worker_shared, &event_sink))?;
        let client = Self {
            address,
            commands,
            shared,
            events: Mutex::new(events),
            worker: Some(worker),
        };
        client.connect_virtual(PLATFORM_RECEIVER)?;
        Ok(client)
    }

    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::Acquire)
    }

    /// Publishes the progressive media time so a queued event can remember it.
    pub(crate) fn watch_media_time<F>(&self, clock: F)
    where
        F: Fn() -> Option<f64> + Send + Sync + 'static,
    {
        *lock(&self.shared.media_clock) = Some(Arc::new(clock));
    }

    /// Opens a virtual connection to `destination` once per client.
    pub(crate) fn connect_virtual(&self, destination: &str) -> Result<()> {
        let fresh = lock(&self.shared.connected).insert(destination.to_owned());
        if fresh {
            self.send(
                destination,
                NS_CONNECTION,
                &json!({"type": "CONNECT", "userAgent": "horizon-chromecast"}),
            )?;
        }
        Ok(())
    }

    /// Sends a JSON message without waiting for a reply.
    /// # Errors
    /// Returns [`Error::Closed`] once the connection has ended.
    pub fn send(&self, destination: &str, namespace: &str, payload: &Value) -> Result<()> {
        let frame = CastMessage::text(SENDER_ID, destination, namespace, payload.to_string()).encode_frame()?;
        self.commands.send(Command::Send(frame)).map_err(|_| Error::Closed)
    }

    /// Sends a JSON message with a fresh `requestId` and waits for the reply
    /// carrying the same id.
    /// # Errors
    /// Returns [`Error::Timeout`] when no reply arrives, or [`Error::Closed`].
    pub fn request(&self, destination: &str, namespace: &str, payload: Value) -> Result<Value> {
        self.request_within(destination, namespace, payload, REQUEST_TIMEOUT)
    }

    /// [`CastClient::request`] with a caller-chosen reply timeout.
    /// # Errors
    /// Returns [`Error::Timeout`] when no reply arrives, or [`Error::Closed`].
    pub fn request_within(
        &self,
        destination: &str,
        namespace: &str,
        mut payload: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.shared.next_request.fetch_add(1, Ordering::Relaxed);
        let Some(fields) = payload.as_object_mut() else {
            return Err(Error::Protocol("request payload must be an object"));
        };
        fields.insert("requestId".to_owned(), id.into());
        let (reply, response) = mpsc::sync_channel(1);
        lock(&self.shared.pending).insert(id, reply);
        let sent = self.send(destination, namespace, &payload);
        let result = sent.and_then(|()| match response.recv_timeout(timeout) {
            Ok(value) => Ok(value),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout(request_kind(&payload))),
            Err(RecvTimeoutError::Disconnected) => Err(Error::Closed),
        });
        lock(&self.shared.pending).remove(&id);
        result
    }

    /// Waits up to `timeout` for the next unsolicited receiver message.
    /// # Errors
    /// Returns [`Error::Closed`] once the connection has ended and every
    /// queued event has been read.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<Event>> {
        match lock(&self.events).recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }
}

impl Drop for CastClient {
    fn drop(&mut self) {
        let connected: Vec<String> = lock(&self.shared.connected).drain().collect();
        for destination in connected.iter().filter(|d| *d != PLATFORM_RECEIVER) {
            let _ = self.send(destination, NS_CONNECTION, &json!({"type": "CLOSE"}));
        }
        let _ = self.send(PLATFORM_RECEIVER, NS_CONNECTION, &json!({"type": "CLOSE"}));
        let _ = self.commands.send(Command::Close);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Maps a reply that is not the expected status into [`Error::Rejected`].
pub(crate) fn rejected(reply: &Value) -> Error {
    let field = |name: &str| reply.get(name).and_then(Value::as_str).map(str::to_owned);
    Error::Rejected {
        kind: field("type").unwrap_or_else(|| "UNKNOWN".to_owned()),
        reason: field("reason").or_else(|| reply.get("detailedErrorCode").map(Value::to_string)),
    }
}

fn request_kind(payload: &Value) -> String {
    payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("request")
        .to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run(mut stream: channel::TlsStream, commands: Receiver<Command>, shared: &Shared, events: &SyncSender<Event>) {
    if let Err(error) = io_loop(&mut stream, &commands, shared, events) {
        tracing::debug!(%error, "chromecast connection ended");
    }
    // Close the queue before failing waiters: a request registered after the
    // clear below can then no longer be sent, so it cannot wait for a reply.
    drop(commands);
    shared.open.store(false, Ordering::Release);
    lock(&shared.pending).clear();
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

fn io_loop(
    stream: &mut channel::TlsStream,
    commands: &Receiver<Command>,
    shared: &Shared,
    events: &SyncSender<Event>,
) -> Result<()> {
    let ping = CastMessage::text(
        SENDER_ID,
        PLATFORM_RECEIVER,
        NS_HEARTBEAT,
        r#"{"type":"PING"}"#.to_owned(),
    )
    .encode_frame()?;
    let mut inbound = Vec::new();
    let mut chunk = vec![0; 16 * 1024];
    let mut last_ping = Instant::now();
    let mut last_inbound = Instant::now();
    loop {
        // At most one backlog per pass, so busy senders cannot starve reads and heartbeats.
        for _ in 0..COMMAND_BACKLOG {
            match commands.try_recv() {
                Ok(Command::Send(frame)) => stream.write_all(&frame)?,
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
            }
        }
        if last_ping.elapsed() >= HEARTBEAT_INTERVAL {
            stream.write_all(&ping)?;
            last_ping = Instant::now();
        }
        stream.flush()?;
        if last_inbound.elapsed() >= IDLE_LIMIT {
            return Err(Error::Timeout("heartbeat".to_owned()));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(Error::Closed),
            Ok(read) => {
                last_inbound = Instant::now();
                inbound.extend_from_slice(&chunk[..read]);
                for message in proto::drain_frames(&mut inbound)? {
                    dispatch(stream, message, shared, events)?;
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn dispatch(
    stream: &mut channel::TlsStream,
    message: CastMessage,
    shared: &Shared,
    events: &SyncSender<Event>,
) -> Result<()> {
    let Payload::Text(text) = message.payload else {
        return Ok(());
    };
    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
        tracing::debug!(namespace = %message.namespace, "ignoring non-JSON receiver message");
        return Ok(());
    };
    let kind = payload.get("type").and_then(Value::as_str);
    if message.namespace == NS_HEARTBEAT {
        if kind == Some("PING") {
            let pong = CastMessage::text(
                SENDER_ID,
                &message.source,
                NS_HEARTBEAT,
                r#"{"type":"PONG"}"#.to_owned(),
            );
            stream.write_all(&pong.encode_frame()?)?;
        }
        return Ok(());
    }
    if message.namespace == NS_CONNECTION && kind == Some("CLOSE") {
        lock(&shared.connected).remove(&message.source);
    }
    let waiter = payload
        .get("requestId")
        .and_then(Value::as_u64)
        .filter(|id| *id != 0)
        .and_then(|id| lock(&shared.pending).remove(&id));
    if let Some(waiter) = waiter {
        let _ = waiter.try_send(payload);
        return Ok(());
    }
    let clock = lock(&shared.media_clock).clone();
    let media_time = clock.as_ref().and_then(|clock| clock());
    let event = Event {
        namespace: message.namespace,
        source: message.source,
        payload,
        media_time,
    };
    if let Err(TrySendError::Full(event)) = events.try_send(event) {
        tracing::warn!(namespace = %event.namespace, "dropping receiver event; host is not draining events");
    }
    Ok(())
}
