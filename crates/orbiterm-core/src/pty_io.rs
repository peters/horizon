use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use filedescriptor::{FileDescriptor, POLLERR, POLLHUP, POLLIN, poll, pollfd, socketpair};
use portable_pty::unix::RawFd;

use crate::{Error, Result};

static PTY_IO_LOOP: OnceLock<Arc<PtyIoLoop>> = OnceLock::new();

pub fn register_reader(
    panel_id: u64,
    fd: RawFd,
    reader: Box<dyn Read + Send>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    PtyIoLoop::shared()?.register(ReaderRegistration {
        panel_id,
        fd,
        reader,
        output_tx,
    })
}

pub fn unregister_reader(panel_id: u64) {
    if let Some(io_loop) = PTY_IO_LOOP.get() {
        io_loop.unregister(panel_id);
    }
}

struct PtyIoLoop {
    control_tx: mpsc::Sender<ControlMessage>,
    wake_writer: Mutex<FileDescriptor>,
}

struct ReaderRegistration {
    panel_id: u64,
    fd: RawFd,
    reader: Box<dyn Read + Send>,
    output_tx: mpsc::Sender<Vec<u8>>,
}

struct ReaderEntry {
    fd: RawFd,
    reader: Box<dyn Read + Send>,
    output_tx: mpsc::Sender<Vec<u8>>,
}

enum ControlMessage {
    Register(ReaderRegistration),
    Unregister(u64),
}

impl PtyIoLoop {
    fn shared() -> Result<&'static Arc<Self>> {
        if let Some(io_loop) = PTY_IO_LOOP.get() {
            return Ok(io_loop);
        }

        let io_loop = Arc::new(Self::spawn()?);
        let _ = PTY_IO_LOOP.set(io_loop);
        PTY_IO_LOOP
            .get()
            .ok_or_else(|| Error::Pty("failed to initialize PTY I/O loop".to_string()))
    }

    fn spawn() -> Result<Self> {
        let (control_tx, control_rx) = mpsc::channel();
        let (wake_reader, wake_writer) =
            socketpair().map_err(|error| Error::Pty(format!("failed to create PTY wake socket: {error}")))?;

        std::thread::Builder::new()
            .name("orbiterm-pty-io".to_string())
            .spawn(move || run_io_loop(&control_rx, wake_reader))
            .map_err(|error| Error::Pty(format!("failed to spawn PTY I/O loop: {error}")))?;

        Ok(Self {
            control_tx,
            wake_writer: Mutex::new(wake_writer),
        })
    }

    fn register(&self, registration: ReaderRegistration) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Register(registration))
            .map_err(|error| Error::Pty(format!("failed to queue PTY reader registration: {error}")))?;
        self.wake()
    }

    fn unregister(&self, panel_id: u64) {
        if self.control_tx.send(ControlMessage::Unregister(panel_id)).is_ok() {
            let _ = self.wake();
        }
    }

    fn wake(&self) -> Result<()> {
        let mut wake_writer = self
            .wake_writer
            .lock()
            .map_err(|_| Error::Pty("PTY wake socket lock poisoned".to_string()))?;
        wake_writer
            .write_all(&[1])
            .map_err(|error| Error::Pty(format!("failed to wake PTY I/O loop: {error}")))
    }
}

fn run_io_loop(control_rx: &mpsc::Receiver<ControlMessage>, mut wake_reader: FileDescriptor) {
    let mut readers = HashMap::new();

    loop {
        drain_control_messages(control_rx, &mut readers);

        let (mut poll_fds, ordered_panel_ids) = build_poll_fds(&wake_reader, &readers);

        match poll(&mut poll_fds, None) {
            Ok(0) => {}
            Ok(_) => {
                if poll_fds
                    .first()
                    .is_some_and(|wake_fd| wake_fd.revents & (POLLIN | POLLHUP | POLLERR) != 0)
                {
                    drain_wake_reader(&mut wake_reader);
                    drain_control_messages(control_rx, &mut readers);
                }

                drain_ready_readers(&ordered_panel_ids, &poll_fds[1..], &mut readers);
            }
            Err(error) => {
                tracing::error!("PTY I/O loop poll failed: {error}");
            }
        }
    }
}

fn drain_control_messages(control_rx: &mpsc::Receiver<ControlMessage>, readers: &mut HashMap<u64, ReaderEntry>) {
    while let Ok(message) = control_rx.try_recv() {
        apply_control_message(message, readers);
    }
}

fn apply_control_message(message: ControlMessage, readers: &mut HashMap<u64, ReaderEntry>) {
    match message {
        ControlMessage::Register(registration) => {
            readers.insert(
                registration.panel_id,
                ReaderEntry {
                    fd: registration.fd,
                    reader: registration.reader,
                    output_tx: registration.output_tx,
                },
            );
        }
        ControlMessage::Unregister(panel_id) => {
            readers.remove(&panel_id);
        }
    }
}

fn build_poll_fds(wake_reader: &FileDescriptor, readers: &HashMap<u64, ReaderEntry>) -> (Vec<pollfd>, Vec<u64>) {
    let mut poll_fds = Vec::with_capacity(readers.len() + 1);
    let mut panel_ids = Vec::with_capacity(readers.len());
    poll_fds.push(pollfd {
        fd: wake_reader.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    });

    for (&panel_id, entry) in readers {
        panel_ids.push(panel_id);
        poll_fds.push(pollfd {
            fd: entry.fd,
            events: POLLIN | POLLHUP | POLLERR,
            revents: 0,
        });
    }

    (poll_fds, panel_ids)
}

fn drain_wake_reader(wake_reader: &mut FileDescriptor) {
    let mut wake_buf = [0_u8; 64];
    match wake_reader.read(&mut wake_buf) {
        Ok(bytes_read) => {
            if bytes_read == wake_buf.len() {
                tracing::trace!("PTY wake socket still has queued notifications");
            }
        }
        Err(error) => tracing::debug!("failed to drain PTY wake socket: {error}"),
    }
}

fn drain_ready_readers(ordered_panel_ids: &[u64], poll_fds: &[pollfd], readers: &mut HashMap<u64, ReaderEntry>) {
    let ready_panel_ids: Vec<u64> = ordered_panel_ids
        .iter()
        .copied()
        .zip(poll_fds.iter())
        .filter_map(|(panel_id, poll_fd)| (poll_fd.revents & (POLLIN | POLLHUP | POLLERR) != 0).then_some(panel_id))
        .collect();

    let mut removed_panel_ids = Vec::new();
    for panel_id in ready_panel_ids {
        let should_remove = readers.get_mut(&panel_id).is_some_and(process_reader_event);

        if should_remove {
            removed_panel_ids.push(panel_id);
        }
    }

    for panel_id in removed_panel_ids {
        readers.remove(&panel_id);
    }
}

fn process_reader_event(entry: &mut ReaderEntry) -> bool {
    let mut buf = [0_u8; 4096];
    match entry.reader.read(&mut buf) {
        Ok(0) => true,
        Ok(bytes_read) => entry.output_tx.send(buf[..bytes_read].to_vec()).is_err(),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => false,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(error) => {
            tracing::debug!("PTY reader loop dropping fd {} after read error: {error}", entry.fd);
            true
        }
    }
}
