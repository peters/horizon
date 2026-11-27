//! One bridge: one child program, its three threads and the events that it
//! sends.
//!
//! [`Bridge::start`] starts the program of a [`ProgramSpec`] and gives it three
//! threads. `agent-bridge-out` reads the standard output by lines with the bound
//! of [`MAX_LINE_BYTES`], hands each line to the codec, sends the events of that
//! line and calls the wake after each line that gave an event. `agent-bridge-in`
//! writes the lines of [`Bridge::send`] and flushes each one. A reader of the
//! standard error keeps its last [`STDERR_TAIL_BYTES`] bytes for the notice of
//! the end.
//!
//! The end of the standard output is the end of the conversation: the reader
//! waits for the child, joins the reader of the standard error, sends
//! [`BridgeEvent::Ended`] with the exit code and the tail of the words, and
//! speaks the wake. A log line of this module never holds a line of the program.
//!
//! [`Bridge::stop`] closes the standard input, waits, then ends the child by its
//! own process id when the child ignores both steps, and joins the threads. The
//! whole stop is bounded by [`STOP_TIMEOUT`].

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use rustix::process::Signal;

use crate::process::{HangingChild, start_identity};

use super::program::{ProgramSpec, apply_environment};
use super::{BridgeEvent, DropReason, LineCodec, MAX_LINE_BYTES, MAX_TEXT_BYTES, STDERR_TAIL_BYTES, STOP_TIMEOUT};

/// How long a stop waits for the reader after it closed the standard input,
/// before it ends the child.
const STOP_GRACE: Duration = Duration::from_millis(250);

/// The time between two reads of the state of a thread during a stop.
const STOP_POLL: Duration = Duration::from_millis(5);

/// How long a stop waits for the program after the termination signal, before
/// the kill signal.
const TERM_GRACE: Duration = Duration::from_millis(1500);

/// How long a stop waits for the program after the kill signal.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// How long the reader waits for the last words of the standard error after the
/// child ended.
const STDERR_GRACE: Duration = Duration::from_millis(250);

/// The failure of one bridge.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The program did not start.
    #[error("the program of the bridge does not start")]
    Start(#[source] std::io::Error),
    /// The program started, and its pipes are not there.
    #[error("the program of the bridge has no pipes")]
    Pipes,
    /// A thread of the bridge did not start.
    #[error("a thread of the bridge does not start")]
    Thread(#[source] std::io::Error),
    /// The program ended, or its standard input is closed.
    #[error("the bridge has no program to send to")]
    Closed,
    /// The text of a turn is empty.
    #[error("the text of a turn is empty")]
    EmptyText,
    /// The text of a turn is above [`MAX_TEXT_BYTES`] bytes.
    #[error("the text of a turn is above {MAX_TEXT_BYTES} bytes")]
    TextTooLong,
}

/// The threads of one bridge, which its stop joins.
struct Threads {
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

/// One program of this slice, and its conversation, on a pipe.
pub struct Bridge {
    events: Receiver<BridgeEvent>,
    turns: Option<Sender<String>>,
    ended: Arc<AtomicBool>,
    /// The process id of the child, which the stop uses when the pipes do not
    /// end it.
    pid: u32,
    threads: Option<Threads>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Bridge")
            .field("pid", &self.pid)
            .field("ended", &self.ended.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// Starts the program of `spec` and the threads that speak to it.
    ///
    /// # Errors
    ///
    /// [`BridgeError::Start`] when the program does not start,
    /// [`BridgeError::Pipes`] when its pipes are not there, and
    /// [`BridgeError::Thread`] when a thread of the bridge does not start. A
    /// failure ends the program that did start.
    pub fn start(
        spec: ProgramSpec,
        codec: Box<dyn LineCodec>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, BridgeError> {
        let ProgramSpec {
            program,
            args,
            env,
            env_remove,
            cwd,
        } = spec;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_environment(&env, &env_remove, &mut command);
        let shown_program = command.get_program().to_string_lossy().into_owned();
        let mut child = command.spawn().map_err(BridgeError::Start)?;
        let pid = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(BridgeError::Pipes);
        };
        tracing::info!(pid, program = %shown_program, "the agent bridge started a program");

        let (event_sender, events) = mpsc::channel();
        let (turn_sender, turns) = mpsc::channel::<String>();
        let child = Arc::new(Mutex::new(Some(child)));
        let tail = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let codec: Arc<Mutex<Box<dyn LineCodec>>> = Arc::new(Mutex::new(codec));

        let writer = spawn("agent-bridge-in", {
            let codec = Arc::clone(&codec);
            move || write_lines(stdin, turns, &codec)
        })
        .map_err(|error| fail_start(pid, error))?;
        let error_tail = Arc::clone(&tail);
        let stderr = spawn("agent-bridge-err", move || read_tail(stderr, &error_tail))
            .map_err(|error| fail_start(pid, error))?;
        let reader = Reader {
            stdout,
            codec: Arc::clone(&codec),
            events: event_sender,
            wake,
            child: Arc::clone(&child),
            stderr,
            tail: Arc::clone(&tail),
            ended: Arc::clone(&ended),
        };
        let reader = spawn("agent-bridge-out", move || reader.run()).map_err(|error| fail_start(pid, error))?;
        Ok(Self {
            events,
            turns: Some(turn_sender),
            ended,
            pid,
            threads: Some(Threads { reader, writer }),
        })
    }

    /// The process id of the child of this bridge.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Queues one text for the program. It never waits for the program: the
    /// writer thread builds the line, writes it and flushes it.
    ///
    /// # Errors
    ///
    /// [`BridgeError::EmptyText`] for an empty text, [`BridgeError::TextTooLong`]
    /// for a text above [`MAX_TEXT_BYTES`] bytes, and [`BridgeError::Closed`]
    /// when the program ended or its standard input is closed.
    pub fn send(&self, text: String) -> Result<(), BridgeError> {
        if text.is_empty() {
            return Err(BridgeError::EmptyText);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(BridgeError::TextTooLong);
        }
        let Some(turns) = self.turns.as_ref() else {
            return Err(BridgeError::Closed);
        };
        if self.ended.load(Ordering::Acquire) {
            return Err(BridgeError::Closed);
        }
        turns.send(text).map_err(|_| BridgeError::Closed)
    }

    /// The events that arrived, and no wait for one that did not.
    #[must_use]
    pub fn poll(&self) -> Vec<BridgeEvent> {
        self.events.try_iter().collect()
    }

    /// Ends this bridge: closes the standard input, waits for the program, and
    /// ends it by its own process id when it outlives both steps. The whole
    /// stop is bounded by [`STOP_TIMEOUT`].
    pub fn stop(mut self) {
        self.close_input();
        let Some(threads) = self.threads.take() else {
            return;
        };
        let deadline = Instant::now() + STOP_TIMEOUT;
        let mut done = join_within(&threads.reader, STOP_GRACE.min(remaining(deadline)));
        if !done {
            end_child(self.pid);
            done = join_within(&threads.reader, remaining(deadline));
        }
        if done {
            let _ = threads.reader.join();
        } else {
            tracing::warn!(pid = self.pid, "the reader of the bridge outlives its stop");
        }
        if join_within(&threads.writer, remaining(deadline)) {
            let _ = threads.writer.join();
        }
    }

    /// Closes the standard input of the program by dropping the queue of the
    /// writer. The writer thread ends and drops the pipe.
    fn close_input(&mut self) {
        self.turns = None;
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.close_input();
        self.threads = None;
    }
}

/// Ends the child of a start that failed, and answers the error of that start.
fn fail_start(pid: u32, error: BridgeError) -> BridgeError {
    end_child(pid);
    error
}

/// Starts one thread of the bridge with its name.
fn spawn<F>(name: &str, body: F) -> Result<JoinHandle<()>, BridgeError>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map_err(BridgeError::Thread)
}

/// The values that the reader of one bridge holds, so that its thread starts
/// with one value.
struct Reader {
    stdout: ChildStdout,
    codec: Arc<Mutex<Box<dyn LineCodec>>>,
    events: Sender<BridgeEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    child: Arc<Mutex<Option<Child>>>,
    stderr: JoinHandle<()>,
    tail: Arc<Mutex<Vec<u8>>>,
    ended: Arc<AtomicBool>,
}

impl Reader {
    /// Reads the lines of the program until its standard output ends, and sends
    /// the events and the one event of the end.
    fn run(self) {
        let Self {
            stdout,
            codec,
            events,
            wake,
            child,
            stderr,
            tail,
            ended,
        } = self;
        let mut lines = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            let batch = match read_line(&mut lines, &mut line) {
                Ok(ReadLine::End) => break,
                Ok(ReadLine::TooLong) => vec![BridgeEvent::Dropped {
                    reason: DropReason::TooLong,
                }],
                Ok(ReadLine::Line) => match std::str::from_utf8(&line) {
                    Ok(text) => codec.lock().unwrap_or_else(PoisonError::into_inner).parse(text),
                    Err(_) => vec![BridgeEvent::Dropped {
                        reason: DropReason::NotJson,
                    }],
                },
                Err(error) => {
                    tracing::warn!(%error, "the agent bridge cannot read its program");
                    break;
                }
            };
            if batch.is_empty() {
                continue;
            }
            for event in &batch {
                if let BridgeEvent::Dropped { reason } = event {
                    tracing::debug!(?reason, "the agent bridge dropped a line");
                }
            }
            for event in batch {
                let _ = events.send(event);
            }
            wake();
        }
        let exit_code = wait_child(&child);
        if !join_within(&stderr, STDERR_GRACE) {
            tracing::debug!("the last words of the program outlive its end");
        }
        let stderr_tail = String::from_utf8_lossy(&tail.lock().unwrap_or_else(PoisonError::into_inner)).into_owned();
        ended.store(true, Ordering::Release);
        let _ = events.send(BridgeEvent::Ended { exit_code, stderr_tail });
        wake();
        tracing::info!(?exit_code, "the agent bridge ended");
    }
}

/// Writes each queued text as one line of the program and flushes it. The end of
/// the queue closes the pipe, and so does an end of the program.
fn write_lines(mut stdin: impl Write, texts: Receiver<String>, codec: &Mutex<Box<dyn LineCodec>>) {
    for text in texts {
        let line = codec.lock().unwrap_or_else(PoisonError::into_inner).user_line(&text);
        if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() || stdin.flush().is_err() {
            return;
        }
    }
}

/// Reads the standard error of the program and keeps its last bytes.
fn read_tail(mut stderr: ChildStderr, tail: &Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 1024];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => return,
            Ok(count) => {
                let mut kept = tail.lock().unwrap_or_else(PoisonError::into_inner);
                kept.extend_from_slice(&chunk[..count]);
                let length = kept.len();
                if length > STDERR_TAIL_BYTES {
                    kept.drain(..length - STDERR_TAIL_BYTES);
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// The answer of one bounded read of one line of the program.
enum ReadLine {
    /// One whole line, without its newline, within the bound.
    Line,
    /// A line above the bound, read to its newline and held nowhere.
    TooLong,
    /// The end of the standard output.
    End,
}

/// Reads one line of at most [`MAX_LINE_BYTES`] bytes. A longer line is read to
/// its newline and answers [`ReadLine::TooLong`] without a byte of it.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> std::io::Result<ReadLine> {
    line.clear();
    let mut too_long = false;
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if chunk.is_empty() {
            return Ok(if too_long {
                ReadLine::TooLong
            } else if line.is_empty() {
                ReadLine::End
            } else {
                ReadLine::Line
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(chunk.len());
        if !too_long {
            if line.len() + take <= MAX_LINE_BYTES {
                line.extend_from_slice(&chunk[..take]);
            } else {
                too_long = true;
                line.clear();
            }
        }
        let consumed = if newline.is_some() { take + 1 } else { take };
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(if too_long { ReadLine::TooLong } else { ReadLine::Line });
        }
    }
}

/// Waits for the child to end and answers its exit code.
fn wait_child(child: &Arc<Mutex<Option<Child>>>) -> Option<i32> {
    let mut slot = child.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.as_mut()?.wait() {
        Ok(status) => status.code(),
        Err(_) => None,
    }
}

/// Ends the child of `pid`: the termination signal, then the kill signal, each
/// step guarded by the start time of the process so that no signal reaches a
/// later process that the system gave the same id.
fn end_child(pid: u32) {
    let child = HangingChild::new(pid, start_identity(pid));
    if child.has_ended() || !signal(&child, Signal::TERM, "SIGTERM") {
        return;
    }
    if ended_within(&child, TERM_GRACE) || !signal(&child, Signal::KILL, "SIGKILL") {
        return;
    }
    let _ = ended_within(&child, KILL_GRACE);
}

/// Sends one signal to the exact process id of the child.
fn signal(child: &HangingChild, signal: Signal, step: &'static str) -> bool {
    if !child.still_names_its_process() {
        return false;
    }
    let Ok(raw) = i32::try_from(child.pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    let reached = rustix::process::kill_process(pid, signal).is_ok();
    if reached {
        tracing::warn!(pid = child.pid, step, "the program of the bridge outlived its stop");
    }
    reached
}

/// Whether the program of `child` ended within `within`.
fn ended_within(child: &HangingChild, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if child.has_ended() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        sleep(STOP_POLL.min(deadline.saturating_duration_since(now)));
    }
}

/// The time that is left of `deadline`.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Waits up to `limit` for `thread`, and answers whether it ended. A thread that
/// did not end is left to its own end.
fn join_within(thread: &JoinHandle<()>, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if thread.is_finished() {
            return true;
        }
        sleep(STOP_POLL);
    }
    thread.is_finished()
}
