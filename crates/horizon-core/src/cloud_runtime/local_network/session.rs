//! The one SSH process per bridge: it forwards the worker's bridge socket to the local proxy
//! and keeps the worker helper alive with a heartbeat. It is restarted with backoff while on.
//! The same session carries the helper's discovery calls and their answers.
use super::{Answers, Shared, State};
use crate::cloud_runtime::{Cancellation, command::Runner, ssh::Connection};
use horizon_cloud_protocol::local_network::{
    HEARTBEAT_INTERVAL, Nonce, PREPARE_COMMAND, PREPARED, Ready, Subnet,
    discovery::{Answer, Call, MAX_CALL, MAX_LINE, Message, Request, text},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError, TryLockError,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PREPARE_TIMEOUT: Duration = Duration::from_secs(40);
const READY_TIMEOUT: Duration = Duration::from_secs(40);
const POLL: Duration = Duration::from_millis(100);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(30);
/// A session that stayed up this long starts the next retry from [`FIRST_RETRY`] again.
const STABLE: Duration = Duration::from_secs(60);
/// Calls waiting for or being answered; the helper never has more in flight.
const MAX_QUEUED: usize = 8;
const BUSY: &str = "The owner's Horizon is answering too many requests; try again";
pub(super) const UNSUPPORTED: &str =
    "This cloud's worker image does not support Local Network Bridge yet; rebuild the image";
const CLOSED: &str = "The bridge connection to the worker closed";
const UNCONFIRMED: &str = "The worker did not confirm the bridge";

pub(super) trait Transport: Send + 'static {
    /// Must print [`PREPARED`] on a worker that supports the bridge.
    fn prepare(&self) -> Command;
    /// Holds one session: the worker's bridge socket forwarded to `127.0.0.1:proxy_port`
    /// here, and the worker helper reading heartbeats from standard input.
    fn hold(&self, nonce: &Nonce, subnet: Subnet, proxy_port: u16) -> Command;
    fn heartbeat(&self) -> Duration {
        HEARTBEAT_INTERVAL
    }
}

/// The cloud's own pinned SSH connection: known host key only, no agent or X11 forwarding.
pub(super) struct Ssh(pub(super) Connection);

impl Transport for Ssh {
    fn prepare(&self) -> Command {
        self.0.pinned_command(PREPARE_COMMAND)
    }

    fn hold(&self, nonce: &Nonce, subnet: Subnet, proxy_port: u16) -> Command {
        let mut command = Command::new("ssh");
        // OpenSSH keeps the first value of an option, so these precede the connection's own.
        command
            .args(["-T", "-a", "-x", "-o", "ExitOnForwardFailure=yes", "-R"])
            .arg(format!("{}:127.0.0.1:{proxy_port}", nonce.bridge_socket()))
            .args(self.0.pinned_args())
            .arg(nonce.hold_command(subnet));
        command
    }
}

enum Ended {
    Cancelled,
    Unsupported,
    Lost(String),
}

pub(super) fn supervise(
    transport: &dyn Transport,
    subnet: Subnet,
    proxy_port: u16,
    answers: &Arc<dyn Answers>,
    shared: &Shared,
    cancel: &Cancellation,
) {
    let mut retry = FIRST_RETRY;
    while !cancel.is_cancelled() {
        let mut active_since = None;
        let error = match attempt(
            transport,
            subnet,
            proxy_port,
            answers,
            shared,
            cancel,
            &mut active_since,
        ) {
            Ended::Cancelled => return,
            Ended::Unsupported => {
                shared.set(State::Failed {
                    error: UNSUPPORTED.into(),
                });
                return;
            }
            Ended::Lost(error) => error,
        };
        tracing::info!(%error, "local network bridge session ended");
        shared.set(State::Reconnecting { error });
        if active_since.is_some_and(|since: Instant| since.elapsed() >= STABLE) {
            retry = FIRST_RETRY;
        }
        let resume = Instant::now() + retry;
        while Instant::now() < resume {
            if cancel.is_cancelled() {
                return;
            }
            thread::sleep(POLL);
        }
        retry = (retry * 2).min(LAST_RETRY);
    }
}

fn attempt(
    transport: &dyn Transport,
    subnet: Subnet,
    proxy_port: u16,
    answers: &Arc<dyn Answers>,
    shared: &Shared,
    cancel: &Cancellation,
    active_since: &mut Option<Instant>,
) -> Ended {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let prepared = runner.run("Local network preparation", &mut transport.prepare(), PREPARE_TIMEOUT);
    if cancel.is_cancelled() {
        return Ended::Cancelled;
    }
    match prepared.map(|output| preparation(&output)) {
        Ok(None) => {}
        Ok(Some(ended)) => return ended,
        Err(_) => return Ended::Lost("Cannot reach the worker over SSH".into()),
    }
    let nonce = Nonce::random();
    let mut command = transport.hold(&nonce, subnet, proxy_port);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut hold = match Hold::spawn(command, answers) {
        Ok(hold) => hold,
        Err(error) => return Ended::Lost(format!("Cannot start SSH: {error}")),
    };
    let deadline = Instant::now() + READY_TIMEOUT;
    let ready = loop {
        if cancel.is_cancelled() {
            return Ended::Cancelled;
        }
        if let Some(ended) = hold.exited() {
            return ended;
        }
        match hold.ready.recv_timeout(POLL) {
            Ok(ready) => break ready,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Ended::Lost(UNCONFIRMED.into()),
            // Output ended without the ready line; the exit status explains why.
            Err(mpsc::RecvTimeoutError::Disconnected) => return hold.finish(),
        }
    };
    shared.set(State::Active { proxy: ready.proxy });
    *active_since = Some(Instant::now());
    let mut beat = Instant::now() + transport.heartbeat();
    loop {
        if cancel.is_cancelled() {
            return Ended::Cancelled;
        }
        if let Some(ended) = hold.exited() {
            return ended;
        }
        if Instant::now() >= beat {
            if hold.beat().is_err() {
                return hold.exited().unwrap_or_else(|| Ended::Lost(CLOSED.into()));
            }
            beat = Instant::now() + transport.heartbeat();
        }
        thread::sleep(POLL);
    }
}

/// What the worker's preparation output means: `None` when the bridge can start.
fn preparation(output: &str) -> Option<Ended> {
    if output.lines().any(|line| line.trim() == PREPARED) {
        return None;
    }
    // An image built before the bridge lacks the subcommand, or the whole helper.
    let missing_helper = output
        .lines()
        .any(|line| line.contains("horizon-cloud-worker") && line.contains("not found"));
    if output.contains("Usage: horizon-cloud-worker") || missing_helper {
        return Some(Ended::Unsupported);
    }
    let reason = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(printable);
    Some(Ended::Lost(format!(
        "The worker could not prepare the bridge: {}",
        reason.unwrap_or_else(|| "no output".into())
    )))
}

/// Remote output reaches the card, so it is bounded and stripped of control characters.
fn printable(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_control())
        .take(300)
        .collect()
}

/// The running SSH process, killed and reaped on drop.
struct Hold {
    child: Child,
    input: Input,
    ready: mpsc::Receiver<Ready>,
    last_error: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
}

/// The helper's standard input, shared by the heartbeat and the answers, and closed on drop.
type Input = Arc<Mutex<Option<ChildStdin>>>;

/// Writes one line to the helper; `wait` false skips it while another line is being written.
fn send(input: &Input, line: &[u8], wait: bool) -> std::io::Result<()> {
    let mut input = if wait {
        input.lock().unwrap_or_else(PoisonError::into_inner)
    } else {
        match input.try_lock() {
            Ok(input) => input,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            // An answer being written reaches the helper as a heartbeat too.
            Err(TryLockError::WouldBlock) => return Ok(()),
        }
    };
    let input = input
        .as_mut()
        .ok_or_else(|| std::io::Error::other("SSH input unavailable"))?;
    input.write_all(line)?;
    input.write_all(b"\n")?;
    input.flush()
}

fn message(message: &Message) -> Vec<u8> {
    let line = serde_json::to_vec(message).unwrap_or_default();
    if line.len() < MAX_LINE {
        return line;
    }
    // Answers are bounded to fit; anything else is refused rather than cut.
    match message {
        Message::Answer { id, .. } => serde_json::to_vec(&Message::Answer {
            id: *id,
            answer: Answer::Refused("The answer was too large".into()),
        })
        .unwrap_or_default(),
        Message::Hello(_) => Vec::new(),
    }
}

/// A call from the helper's output line, or `None` for any other line. A call whose request
/// this build cannot read, unknown or malformed, is refused by its number.
fn call(line: &str) -> Option<(u64, Result<Request, String>)> {
    let call: Call<serde_json::Value> = serde_json::from_str(line.trim_end()).ok()?;
    Some((
        call.id,
        serde_json::from_value(call.request).map_err(|error| {
            text(&format!(
                "The owner's Horizon could not read this request ({error}); it may need an update"
            ))
        }),
    ))
}

/// Answers each call on its own thread, so a slow answer never delays the next call past the
/// helper's deadline; at most [`MAX_QUEUED`] are answered at once, and more are refused.
fn answer_calls(queue: &mpsc::Receiver<(u64, Result<Request, String>)>, answers: &Arc<dyn Answers>, input: &Input) {
    let running = Arc::new(AtomicUsize::new(0));
    for (id, request) in queue {
        let admitted = running
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_QUEUED).then_some(count + 1)
            })
            .is_ok();
        let (answers, input, slot) = (Arc::clone(answers), Arc::clone(input), Arc::clone(&running));
        let answer = move || {
            let answer = match request {
                Ok(_) if !admitted => Answer::Refused(BUSY.into()),
                Ok(request) => answers.answer(request),
                Err(refusal) => Answer::Refused(refusal),
            };
            let _ = send(&input, &message(&Message::Answer { id, answer }), true);
            if admitted {
                slot.fetch_sub(1, Ordering::AcqRel);
            }
        };
        if !admitted {
            answer();
            continue;
        }
        if let Err(error) = thread::Builder::new().name("local-network-answer".into()).spawn(answer) {
            tracing::debug!(%error, "local network answer thread did not start");
            running.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Hold {
    fn spawn(mut command: Command, answers: &Arc<dyn Answers>) -> std::io::Result<Self> {
        let mut child = command.spawn()?;
        // Only the first ready line is read; later output keeps draining but is not kept.
        let (sender, ready) = mpsc::sync_channel(1);
        let mut hold = Self {
            input: Arc::new(Mutex::new(child.stdin.take())),
            ready,
            last_error: Arc::new(Mutex::new(String::new())),
            readers: Vec::with_capacity(2),
            child,
        };
        // From here on, a failure drops `hold`, which stops the process.
        let _ = send(&hold.input, &message(&Message::Hello(answers.hello())), true);
        let (calls, queue) = mpsc::sync_channel(MAX_QUEUED);
        if let Some(output) = hold.child.stdout.take() {
            hold.readers
                .push(spawn_reader("local-network-ready", output, move |line| {
                    if let Ok(ready) = serde_json::from_str::<Ready>(line)
                        && ready.usable()
                    {
                        let _ = sender.try_send(ready);
                    } else if let Some(call) = call(line)
                        && calls.try_send(call).is_err()
                    {
                        tracing::debug!("local network helper sent more calls than it may have waiting");
                    }
                })?);
        }
        {
            let (answers, input) = (Arc::clone(answers), Arc::clone(&hold.input));
            thread::Builder::new()
                .name("local-network-calls".into())
                .spawn(move || answer_calls(&queue, &answers, &input))?;
        }
        if let Some(errors) = hold.child.stderr.take() {
            let last = Arc::clone(&hold.last_error);
            hold.readers
                .push(spawn_reader("local-network-errors", errors, move |line| {
                    let line = printable(line.trim());
                    if !line.is_empty() {
                        *last.lock().unwrap_or_else(PoisonError::into_inner) = line;
                    }
                })?);
        }
        Ok(hold)
    }

    fn exited(&mut self) -> Option<Ended> {
        self.child.try_wait().ok()??;
        // Let the readers take the process's final words, but never wait on a descendant
        // that keeps the pipes open.
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let error = self.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Some(Ended::Lost(if error.is_empty() { CLOSED.into() } else { error }))
    }

    /// Reports how the process ended, stopping it if it has not ended within a moment.
    fn finish(&mut self) -> Ended {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(ended) = self.exited() {
                return ended;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ended::Lost(UNCONFIRMED.into())
    }

    /// Never waits on an answer being written, so a helper that stops reading cannot stall
    /// the supervisor.
    fn beat(&self) -> std::io::Result<()> {
        send(&self.input, b"", false)
    }
}

impl Drop for Hold {
    /// Stopping the process first ends any write blocked on its input. The readers and the
    /// call thread end on their own when the pipes close; they are not joined here, so a
    /// descendant holding a pipe open cannot block the caller.
    fn drop(&mut self) {
        let _ = self.child.kill();
        drop(self.input.lock().unwrap_or_else(PoisonError::into_inner).take());
        let _ = self.child.wait();
    }
}

/// Calls `each` for every line of `stream`. A line longer than [`MAX_CALL`] is skipped whole.
fn spawn_reader(
    name: &str,
    stream: impl Read + Send + 'static,
    mut each: impl FnMut(&str) + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        let mut oversized = false;
        // Undecodable bytes never stop the draining, or a full pipe would stall the process.
        loop {
            line.clear();
            match (&mut reader).take(MAX_CALL as u64).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    // Only the last line before the end of output may lack its newline.
                    let complete = line.ends_with(b"\n") || line.len() < MAX_CALL;
                    if !oversized && complete {
                        each(&String::from_utf8_lossy(&line));
                    }
                    oversized = !complete;
                }
            }
        }
    })
}
