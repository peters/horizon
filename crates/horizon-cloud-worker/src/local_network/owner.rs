//! The owner's Horizon as one bridge session's helper hears it: its heartbeat on standard
//! input, what it answers, and the calls waiting for those answers. Calls go out one per line
//! on standard output.
use horizon_cloud_protocol::local_network::{
    HEARTBEAT_TIMEOUT,
    discovery::{ANSWER_TIMEOUT, Answer, Call, Hello, MAX_LINE, Message, Request},
};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(250);
pub(super) const NO_DISCOVERY: &str =
    "The owner's Horizon does not support discovery yet; ask the owner to update Horizon, or ask for device addresses";
const NO_ANSWER: &str = "The owner's Horizon did not answer in time; try again";
const ENDED: &str = "The bridge stopped before the owner's Horizon answered";

type Waiting = Arc<Mutex<HashMap<u64, mpsc::SyncSender<Answer>>>>;

pub(super) struct Owner {
    last: Arc<Mutex<Instant>>,
    ended: Arc<AtomicBool>,
    hello: Arc<Mutex<Option<Hello>>>,
    waiting: Waiting,
    next: AtomicU64,
    output: Mutex<Box<dyn Write + Send>>,
}

/// Removes one call from the waiting list however its wait ends.
struct Pending<'a>(&'a Waiting, u64);

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.1);
    }
}

impl Owner {
    /// Reads the owner's input on its own thread: every byte is a heartbeat, and complete
    /// lines may also carry the hello or an answer. A line longer than [`MAX_LINE`] is
    /// skipped whole; nothing the input says ends the session but its end.
    pub(super) fn watch(input: impl Read + Send + 'static, output: impl Write + Send + 'static) -> io::Result<Self> {
        let owner = Self {
            last: Arc::new(Mutex::new(Instant::now())),
            ended: Arc::new(AtomicBool::new(false)),
            hello: Arc::new(Mutex::new(None)),
            waiting: Arc::default(),
            next: AtomicU64::new(1),
            output: Mutex::new(Box::new(output)),
        };
        let (last, ended, hello, waiting) = (
            Arc::clone(&owner.last),
            Arc::clone(&owner.ended),
            Arc::clone(&owner.hello),
            Arc::clone(&owner.waiting),
        );
        thread::Builder::new()
            .name("local-network-owner".into())
            .spawn(move || {
                let mut reader = BufReader::new(input);
                let mut line = Vec::new();
                let mut oversized = false;
                loop {
                    line.clear();
                    match (&mut reader).take(MAX_LINE as u64).read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            *last.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
                            let complete = line.ends_with(b"\n");
                            if complete && !oversized {
                                receive(&line, &hello, &waiting);
                            }
                            oversized = !complete;
                        }
                    }
                }
                ended.store(true, Ordering::Release);
            })?;
        Ok(owner)
    }

    pub(super) fn silent_for(&self) -> Duration {
        self.last.lock().unwrap_or_else(PoisonError::into_inner).elapsed()
    }

    pub(super) fn alive(&self) -> bool {
        !self.ended.load(Ordering::Acquire) && self.silent_for() < HEARTBEAT_TIMEOUT
    }

    /// What the owner's Horizon said it answers, if it said anything.
    pub(super) fn hello(&self) -> Option<Hello> {
        self.hello.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Writes one line to the owner's Horizon.
    pub(super) fn write(&self, value: &impl serde::Serialize) -> io::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut output = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        output.write_all(&line)?;
        output.flush()
    }

    /// Asks the owner's Horizon and waits for its answer.
    ///
    /// # Errors
    /// Reports an owner that does not answer this kind of request, answers too late, or
    /// whose session ends first.
    pub(super) fn ask(&self, request: Request) -> io::Result<Answer> {
        if self.hello().is_none_or(|hello| hello.discovery == 0) {
            return Err(io::Error::other(NO_DISCOVERY));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, answer) = mpsc::sync_channel(1);
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
        let _pending = Pending(&self.waiting, id);
        self.write(&Call { id, request })?;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            match answer.recv_timeout(POLL) {
                Ok(answer) => return Ok(answer),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::Error::other(ENDED)),
                Err(mpsc::RecvTimeoutError::Timeout) if !self.alive() => return Err(io::Error::other(ENDED)),
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    return Err(io::Error::other(NO_ANSWER));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// An owner last heard from `silent` ago, whose input is never read.
    #[cfg(test)]
    pub(super) fn silent(silent: Duration) -> Self {
        Self {
            last: Arc::new(Mutex::new(
                Instant::now().checked_sub(silent).unwrap_or_else(Instant::now),
            )),
            ended: Arc::new(AtomicBool::new(false)),
            hello: Arc::new(Mutex::new(None)),
            waiting: Arc::default(),
            next: AtomicU64::new(1),
            output: Mutex::new(Box::new(io::sink())),
        }
    }
}

/// Takes in one complete line; an empty line is only a heartbeat, and anything unreadable is
/// ignored.
fn receive(line: &[u8], hello: &Mutex<Option<Hello>>, waiting: &Waiting) {
    if line.trim_ascii().is_empty() {
        return;
    }
    match serde_json::from_slice(line) {
        Ok(Message::Hello(said)) => *hello.lock().unwrap_or_else(PoisonError::into_inner) = Some(said),
        Ok(Message::Answer { id, answer }) => {
            if let Some(sender) = waiting.lock().unwrap_or_else(PoisonError::into_inner).remove(&id) {
                let _ = sender.try_send(answer);
            }
        }
        Err(_) => {}
    }
}
