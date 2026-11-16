//! The life of one holder: the terminal, its program, and one client at a time.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};

use super::frames::{self, Frame, Frames, Hello};
use super::{HolderError, Launch, Paths};
use crate::HorizonHome;

/// The newest output that a holder keeps for the next client.
pub(crate) const RING_LIMIT: usize = 2 * 1024 * 1024;
/// How long a holder keeps the status of a program that ended with no client.
const KEEP_STATUS: Duration = Duration::from_hours(1);
/// How long a holder waits for a client to close after the status of its program.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// The pause of the loop while a client is attached.
const POLL: Duration = Duration::from_millis(5);
/// The pause of the loop while no client is attached.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// The bytes of one read of the output of the program.
const BUFFER: usize = 64 * 1024;

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
    let paths = Paths::of(home, &launch.local_id)?;
    paths.prepare()?;
    let listener = paths.bind()?;
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
    let pump = start_pump(pty.reader().try_clone()?)?;
    let mut ring: Vec<u8> = Vec::new();
    let mut client: Option<Client> = None;
    let mut ended: Option<Option<i32>> = None;
    let mut ended_at: Option<Instant> = None;
    let mut exit_sent_at: Option<Instant> = None;
    tracing::info!(pid, local_id = %launch.local_id, socket = %paths.socket().display(), "a holder serves a panel");

    loop {
        while let Some(stream) = accept(&listener)? {
            if client.is_some() {
                let mut refused = stream;
                let _ = frames::write(&mut refused, &Frame::Refusal);
                continue;
            }
            let hello = Hello {
                pid,
                start_time: start_time.get().cloned().flatten(),
                ring_len: u32::try_from(ring.len()).unwrap_or(u32::MAX),
            };
            client = Some(Client::attach(stream, &hello, &ring)?);
        }

        let mut close = false;
        if let Some(attached) = client.as_mut() {
            match answer(attached, &mut pty) {
                Some(asked) => close = asked,
                None => client = None,
            }
        }
        if close {
            end_program(pty, pid, start_time);
            paths.remove_socket()?;
            return Ok(());
        }

        let mut output = Vec::new();
        while let Ok(mut chunk) = pump.try_recv() {
            ring.extend_from_slice(&chunk);
            output.append(&mut chunk);
        }
        if ring.len() > ring_limit {
            let excess = ring.len() - ring_limit;
            ring.drain(..excess);
        }
        if !output.is_empty()
            && let Some(attached) = client.as_mut()
            && frames::write(&mut attached.socket, &Frame::Output(output)).is_err()
        {
            client = None;
        }

        if ended.is_none()
            && let Some(ChildEvent::Exited(status)) = pty.next_child_event()
        {
            ended = Some(status.and_then(|status| status.code()));
            ended_at = Some(Instant::now());
            tracing::info!(pid, status = ?ended, "the program of a held panel ended");
        }
        if let Some(status) = ended {
            if exit_sent_at.is_none()
                && let Some(attached) = client.as_mut()
            {
                let _ = frames::write(&mut attached.socket, &Frame::Exit(status));
                exit_sent_at = Some(Instant::now());
            }
            let unanswered = exit_sent_at.is_some_and(|sent| sent.elapsed() >= CLOSE_WAIT);
            let kept = exit_sent_at.is_none() && ended_at.is_some_and(|end| end.elapsed() >= KEEP_STATUS);
            if (client.is_none() && exit_sent_at.is_some()) || unanswered || kept {
                paths.remove_socket()?;
                return Ok(());
            }
        }

        thread::sleep(if client.is_some() { POLL } else { IDLE_POLL });
    }
}

/// The one client of a holder.
struct Client {
    socket: UnixStream,
    frames: Frames,
}

impl Client {
    /// Answers the hello and the newest output, so the client reads the last
    /// screens before the live output.
    fn attach(mut socket: UnixStream, hello: &Hello, ring: &[u8]) -> Result<Self, HolderError> {
        frames::write(&mut socket, &Frame::Hello(hello.clone()))?;
        if !ring.is_empty() {
            frames::write(&mut socket, &Frame::Replay(ring.to_vec()))?;
        }
        Ok(Self {
            socket,
            frames: Frames::default(),
        })
    }

    /// Reads what the client sent, and answers the frames that arrived whole.
    ///
    /// `None` says that the client went away or broke the protocol: the program
    /// keeps running and the ring keeps filling.
    fn read(&mut self) -> Option<Vec<Frame>> {
        let mut chunk = [0u8; 8192];
        let mut answered = Vec::new();
        loop {
            match self.socket.read(&mut chunk) {
                Ok(0) => return None,
                Ok(got) => match self.frames.push(&chunk[..got]) {
                    Ok(frames) => answered.extend(frames),
                    Err(problem) => {
                        tracing::warn!(%problem, "the client of a holder broke the protocol");
                        return None;
                    }
                },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Some(answered),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    tracing::debug!(%error, "the client of a holder went away");
                    return None;
                }
            }
        }
    }
}

/// Answers the frames of the client: its input reaches the program and its size
/// reaches the terminal.
///
/// `None` says that the client went away, and `Some(true)` that it asked for the
/// end of the program.
fn answer(client: &mut Client, pty: &mut Pty) -> Option<bool> {
    let mut close = false;
    for frame in client.read()? {
        match frame {
            Frame::Input(bytes) => {
                let _ = pty.writer().write_all(&bytes);
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
            Frame::Close => close = true,
            other => tracing::warn!(
                kind = other.kind(),
                "the client of a holder sent a frame that only a holder sends"
            ),
        }
    }
    Some(close)
}

/// Accepts the next client of `listener`, or answers `None` when no client waits.
fn accept(listener: &UnixListener) -> Result<Option<UnixStream>, HolderError> {
    match listener.accept() {
        Ok((stream, _)) => {
            stream.set_nonblocking(true)?;
            Ok(Some(stream))
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Reads the output of the program on its own thread, so the holder serves a
/// client while the program writes nothing.
///
/// The master of a terminal is non-blocking, so this reader is made blocking: the
/// thread waits in the read instead of asking the kernel for new bytes.
fn start_pump(mut reader: fs::File) -> Result<Receiver<Vec<u8>>, HolderError> {
    rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::empty()).map_err(io::Error::from)?;
    let (send, receive) = mpsc::channel();
    thread::Builder::new()
        .name("panel-holder-reader".to_owned())
        .spawn(move || pump(&mut reader, &send))?;
    Ok(receive)
}

/// Hands every part of the output of the program to the holder, until it ends.
fn pump(reader: &mut fs::File, send: &Sender<Vec<u8>>) {
    let mut buffer = vec![0u8; BUFFER];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(got) => {
                if send.send(buffer[..got].to_vec()).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => return,
        }
    }
}

/// Ends the program as Horizon ends one: the master of its terminal closes, and a
/// program that outlives the hangup gets `SIGTERM`, then `SIGKILL`.
fn end_program(pty: Pty, pid: u32, start_time: Arc<OnceLock<Option<String>>>) {
    crate::panel::watch_a_program_at_the_end(crate::process::HangingChild::new(pid, start_time), "panel holder", None);
    drop(pty);
}
