//! The life of one holder: the terminal, its program, and one client at a time.
//!
//! The loop blocks until something happened: output of the program, a frame or the
//! end of the client, a new client, or the end of the output of the program. Each
//! of those is read on its own thread and the loop reads one channel, so an idle
//! holder makes no turn.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};

use super::{HolderError, Launch, Paths};
use crate::HorizonHome;
use horizon_pty_tap::frames::{self, Frame, Frames, Hello};

/// The newest output that a holder keeps for the next client.
pub(crate) const RING_LIMIT: usize = 2 * 1024 * 1024;
/// How long a holder keeps the status of a program that ended with no client.
const KEEP_STATUS: Duration = Duration::from_hours(1);
/// How long a holder waits for a client to close after the status of its program.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// How long a client may refuse the output of the program before the holder
/// detaches it. The program keeps running and the ring keeps the output.
const CLIENT_WRITE_BOUND: Duration = Duration::from_secs(5);
/// The happenings that wait for the loop, which bounds the memory that a slow
/// reader can hold.
const QUEUE: usize = 8;
/// The parts of input that wait for a program that reads nothing.
const INPUT_QUEUE: usize = 64;
/// The bytes of one read of the output of the program.
const BUFFER: usize = 64 * 1024;
/// How long the loop waits for the event of a program whose output ended.
const PROGRAM_END_BOUND: Duration = Duration::from_secs(2);
/// How long a holder waits for the start time of its program before it serves it.
const IDENTITY_BOUND: Duration = crate::process::START_TIME_BOUND;
/// The pause between two reads of the event of a program whose output ended.
const END_PAUSE: Duration = Duration::from_millis(5);

/// Something that happened to a holder, and the reason that its loop runs.
enum Happening {
    /// Bytes of the output of the program.
    Output(Vec<u8>),
    /// The output of the program ended, so the event of its child is near.
    OutputEnded,
    /// A client connected.
    NewClient(UnixStream),
    /// Frames of a client, named by the generation of its link.
    FromClient(u64, Vec<Frame>),
    /// A client went away, named by the generation of its link.
    ClientGone(u64),
}

/// Serves the socket of `launch` until the program or a client ends the holder.
///
/// The holder is the only owner of the terminal. It answers one client at a time:
/// a second client gets one refusal frame and is closed. A client that goes away
/// detaches, and the program keeps running with its ring filling.
///
/// # Errors
///
/// Returns why the socket, the terminal or a frame of the link cannot continue.
pub(crate) fn serve(home: &HorizonHome, launch: &Launch, ring_limit: usize) -> Result<(), HolderError> {
    serve_watched(home, launch, ring_limit, None)
}

/// Serves the socket of `launch`, and counts the turns of its loop in `turns`,
/// which a test of an idle holder reads.
pub(crate) fn serve_watched(
    home: &HorizonHome,
    launch: &Launch,
    ring_limit: usize,
    turns: Option<&AtomicUsize>,
) -> Result<(), HolderError> {
    let paths = Paths::of(home, &launch.local_id)?;
    paths.prepare()?;
    let listener = paths.bind()?;
    let _plugins = plugin_folder_lease(home, launch);
    let mut pty = crate::terminal::open_pty(
        launch.program.clone(),
        launch.args.clone(),
        launch.cwd.clone(),
        launch.env.clone(),
        launch.window_size(),
        0,
    )
    .map_err(|error| HolderError::Terminal(error.to_string()))?;
    let pid = pty.child().id();
    let start_time = crate::process::start_identity(pid);
    // A system that reads the start time of a program with a program fills it after
    // the start, and the hello of a holder names it.
    let identity_until = Instant::now() + IDENTITY_BOUND;
    while start_time.get().is_none() && Instant::now() < identity_until {
        thread::sleep(END_PAUSE);
    }
    let (send, events) = mpsc::sync_channel(QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    start_accepting(listener, send.clone(), Arc::clone(&stop))?;
    start_pump(pty.reader().try_clone()?, send.clone())?;
    let input = start_input(pty.writer().try_clone()?)?;
    let mut ring: VecDeque<u8> = VecDeque::new();
    let mut client: Option<Client> = None;
    let mut generation = 0u64;
    let mut output_ended = false;
    let mut close = false;
    let mut ended: Option<ChildEnd> = None;
    let mut ended_at: Option<Instant> = None;
    let mut exit_sent_at: Option<Instant> = None;
    tracing::info!(pid, local_id = %launch.local_id, socket = %paths.socket().display(), "a holder serves a panel");

    loop {
        let waited = match turn_deadline(exit_sent_at, ended_at) {
            Some(at) => events.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => events.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        if let Some(turns) = turns {
            turns.fetch_add(1, Ordering::Relaxed);
        }
        match waited {
            Ok(Happening::NewClient(stream)) if client.is_some() => refuse_client(stream),
            Ok(Happening::NewClient(stream)) => {
                generation = generation.saturating_add(1);
                client = attach_client(stream, generation, (pid, &start_time), (&ring, &send));
            }
            Ok(Happening::Output(chunk)) => {
                ring.extend(chunk.iter().copied());
                if ring.len() > ring_limit {
                    ring.drain(..ring.len() - ring_limit);
                }
                if let Some(held) = client.as_mut()
                    && frames::write(&mut held.socket, &Frame::Output(chunk)).is_err()
                {
                    client = None;
                }
            }
            Ok(Happening::OutputEnded) => {
                output_ended = true;
                if let Some(status) = child_event(&mut pty, true) {
                    (ended, ended_at) = (Some(status), Some(ended_state(pid, status.0)));
                }
            }
            Ok(Happening::FromClient(who, whole)) => {
                // A `Close` is the one frame that must not be lost: it ends the
                // program also when the holder detached its client before.
                close |= whole.contains(&Frame::Close);
                if client.as_ref().is_some_and(|held| held.generation == who) && !answer(whole, &mut pty, &input) {
                    client = None;
                }
            }
            Ok(Happening::ClientGone(who)) if client.as_ref().is_some_and(|held| held.generation == who) => {
                client = None;
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if ended.is_none()
            && let Some(status) = child_event(&mut pty, false)
        {
            (ended, ended_at) = (Some(status), Some(ended_state(pid, status.0)));
        }
        if output_ended
            && let Some(status) = ended
            && exit_sent_at.is_none()
            && let Some(held) = client.as_mut()
        {
            let _ = frames::write(&mut held.socket, &Frame::Exit(status.0));
            exit_sent_at = Some(Instant::now());
        }
        let unanswered = exit_sent_at.is_some_and(|sent| sent.elapsed() >= CLOSE_WAIT);
        let kept = exit_sent_at.is_none() && ended_at.is_some_and(|end| end.elapsed() >= KEEP_STATUS);
        if close || (client.is_none() && exit_sent_at.is_some()) || unanswered || kept {
            break;
        }
    }

    if close {
        end_the_program(pty, pid, start_time);
    }
    stop.store(true, Ordering::Release);
    let _ = UnixStream::connect(paths.socket());
    paths.remove_socket()
}

/// Locks the file that says that a program of the Horizon of this launch runs on.
/// The arguments and the tools of that program name the plugin folder and the
/// identity of that Horizon, so both stay while this value lives
/// (`crates/horizon-ui/src/plugin_install/`).
fn plugin_folder_lease(home: &HorizonHome, launch: &Launch) -> Option<fs::File> {
    let host = launch.env.get(crate::browser::manifest::HOST_INSTANCE_ENV)?;
    let held = fs::File::open(home.agent_plugin_host_held_path(host)).ok()?;
    held.try_lock_shared().ok()?;
    Some(held)
}

/// Ends the program of a holder as Horizon ends a panel: the master of its terminal
/// closes, and a program that outlives the hangup gets `SIGTERM`, then `SIGKILL`.
fn end_the_program(pty: Pty, pid: u32, start_time: Arc<OnceLock<Option<String>>>) {
    crate::panel::watch_a_program_at_the_end(crate::process::HangingChild::new(pid, start_time), "panel holder", None);
    drop(pty);
}

/// Reads the event of the child of `pty`, and waits for it for
/// [`PROGRAM_END_BOUND`] when `wait` says that its output ended as well.
fn child_event(pty: &mut Pty, wait: bool) -> Option<ChildEnd> {
    let deadline = Instant::now() + if wait { PROGRAM_END_BOUND } else { Duration::ZERO };
    loop {
        if let Some(ChildEvent::Exited(status)) = pty.next_child_event() {
            return Some(ChildEnd(status.and_then(|status| status.code())));
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(END_PAUSE);
    }
}

/// Records the status of a program that ended.
fn ended_state(pid: u32, status: Option<i32>) -> Instant {
    tracing::info!(pid, status = ?status, "the program of a held panel ended");
    Instant::now()
}

/// The end of the child of a holder, with the status that the system reported, or
/// with none when the system reported none.
#[derive(Clone, Copy)]
struct ChildEnd(Option<i32>);

/// The deadline of the next turn of the loop, or none when the loop waits for the
/// next happening.
fn turn_deadline(exit_sent_at: Option<Instant>, ended_at: Option<Instant>) -> Option<Instant> {
    exit_sent_at
        .map(|at| at + CLOSE_WAIT)
        .or_else(|| ended_at.map(|at| at + KEEP_STATUS))
}

/// Ends the link of a client that arrives while the holder serves another one.
fn refuse_client(stream: UnixStream) {
    let mut refused = stream;
    let _ = frames::write(&mut refused, &Frame::Refusal);
}

/// Answers a new client with the hello and the newest output, and answers the link
/// that the holder writes to, or nothing when that client cannot attach.
fn attach_client(
    stream: UnixStream,
    generation: u64,
    program: (u32, &Arc<OnceLock<Option<String>>>),
    link: (&VecDeque<u8>, &SyncSender<Happening>),
) -> Option<Client> {
    let (ring, send) = link;
    let hello = Hello {
        pid: program.0,
        start_time: program.1.get().cloned().flatten(),
        ring_len: u32::try_from(ring.len()).unwrap_or(u32::MAX),
    };
    match Client::attach(stream, generation, &hello, ring, send) {
        Ok(attached) => Some(attached),
        Err(error) => {
            tracing::warn!(%error, "a client of a holder could not attach");
            None
        }
    }
}

/// Answers the frames of the client: its input reaches the thread that writes it,
/// and its size reaches the terminal. The loop reads its `Close` itself.
///
/// `false` says that the client waits for a program that reads nothing.
fn answer(whole: Vec<Frame>, pty: &mut Pty, input: &SyncSender<Vec<u8>>) -> bool {
    for frame in whole {
        match frame {
            Frame::Input(bytes) => {
                if input.try_send(bytes).is_err() {
                    tracing::warn!("the program of a held panel reads no input");
                    return false;
                }
            }
            Frame::Resize {
                rows,
                cols,
                cell_width,
                cell_height,
            } => pty.on_resize(WindowSize {
                num_lines: rows,
                num_cols: cols,
                cell_width,
                cell_height,
            }),
            Frame::Close => {}
            other => tracing::warn!(
                kind = other.kind(),
                "the client of a holder sent a frame that only a holder sends"
            ),
        }
    }
    true
}

/// The one client of a holder: the link that the loop writes to.
struct Client {
    socket: UnixStream,
    generation: u64,
}

impl Client {
    /// Answers the hello and the newest output, so the client reads the last
    /// screens before the live output, and reads this client on its own thread.
    /// That thread starts first, so the `Close` of a client that is slow to take
    /// its replay reaches the loop.
    fn attach(
        socket: UnixStream,
        generation: u64,
        hello: &Hello,
        ring: &VecDeque<u8>,
        send: &SyncSender<Happening>,
    ) -> Result<Self, HolderError> {
        socket.set_nonblocking(false)?;
        socket.set_write_timeout(Some(CLIENT_WRITE_BOUND))?;
        let mut read = socket.try_clone()?;
        let send = send.clone();
        thread::Builder::new()
            .name("panel-holder-client".to_owned())
            .spawn(move || {
                let mut decoded = Frames::default();
                let mut chunk = [0u8; 8192];
                loop {
                    match read.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(got) => match decoded.push(&chunk[..got]) {
                            Ok(whole) if !whole.is_empty() => {
                                if send.send(Happening::FromClient(generation, whole)).is_err() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(problem) => {
                                tracing::warn!(%problem, "the client of a holder broke the protocol");
                                break;
                            }
                        },
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            tracing::debug!(%error, "the client of a holder went away");
                            break;
                        }
                    }
                }
                let _ = send.send(Happening::ClientGone(generation));
            })?;
        let mut client = Self { socket, generation };
        frames::write(&mut client.socket, &Frame::Hello(hello.clone()))?;
        let (older, newer) = ring.as_slices();
        let most = frames::MAX_PAYLOAD as usize;
        for part in older.chunks(most).chain(newer.chunks(most)) {
            frames::write(&mut client.socket, &Frame::Replay(part.to_vec()))?;
        }
        Ok(client)
    }
}

impl Drop for Client {
    /// Ends the thread that reads this client, so a holder keeps no thread for a
    /// client that left.
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

/// Answers every connection of `listener` to the loop until `stop` says that the
/// holder ends.
fn start_accepting(
    listener: UnixListener,
    send: SyncSender<Happening>,
    stop: Arc<AtomicBool>,
) -> Result<(), HolderError> {
    thread::Builder::new()
        .name("panel-holder-accept".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((_stream, _)) if stop.load(Ordering::Acquire) => return,
                    Ok((stream, _)) => {
                        // A client that arrives while the loop is behind waits in no
                        // queue of its own: the holder drops it, and it connects again.
                        let _ = send.try_send(Happening::NewClient(stream));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        tracing::warn!(%error, "the holder of a panel accepts no client");
                        return;
                    }
                }
            }
        })?;
    Ok(())
}

/// Reads the output of the program on its own thread, so the holder serves a
/// client while the program writes nothing.
///
/// The master of a terminal is non-blocking, so this reader and the writer of the
/// input are made blocking: both wait in the system call instead of asking the
/// kernel for new bytes.
fn start_pump(mut reader: fs::File, send: SyncSender<Happening>) -> Result<(), HolderError> {
    rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::empty()).map_err(io::Error::from)?;
    thread::Builder::new()
        .name("panel-holder-reader".to_owned())
        .spawn(move || {
            let mut buffer = vec![0u8; BUFFER];
            while let Ok(got) = reader.read(&mut buffer) {
                if got == 0 || send.send(Happening::Output(buffer[..got].to_vec())).is_err() {
                    break;
                }
            }
            let _ = send.send(Happening::OutputEnded);
        })?;
    Ok(())
}

/// Writes the input of the client to the program on its own thread, so a program
/// that reads nothing cannot stop the loop that its output needs.
fn start_input(mut writer: fs::File) -> Result<SyncSender<Vec<u8>>, HolderError> {
    let (send, input) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    thread::Builder::new()
        .name("panel-holder-input".to_owned())
        .spawn(move || {
            for bytes in input {
                if writer.write_all(&bytes).is_err() {
                    return;
                }
            }
        })?;
    Ok(send)
}
