//! The link over a Unix socket of this computer, for a server and for the child
//! that carries one computer.
//!
//! The next slice puts `ssh` on top of these two functions: the computer runs one
//! child, the child relays the frames of its stdin and stdout to the socket of the
//! server, and the server answers. Nothing here names a host, a key or a command,
//! so the bytes of a link are the whole boundary of this function.

use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::server::serve;
use crate::store::Store;

/// The mode of the socket file: the server user alone reaches it.
const SOCKET_MODE: u32 = 0o600;

/// How long the server waits before it looks for a stop again, when no computer
/// connects.
const ACCEPT_PAUSE: Duration = Duration::from_millis(10);

/// The bytes of one relay read.
const RELAY_BUFFER: usize = 64 * 1024;

/// Why a socket of the server could not be made or reached.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// The socket cannot be bound, connected, read or written.
    #[error("the server socket at {path}: {error}")]
    Io { path: PathBuf, error: io::Error },
}

/// A server that answers the links of a Unix socket.
#[derive(Debug)]
pub struct SocketServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl SocketServer {
    /// The socket path of this server.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stops accepting links and removes the socket file. A link that is open
    /// ends when its computer closes it; the store stays with the caller.
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accepting) = self.accepting {
            let _ = accepting.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// Binds `path` and answers every link of it with `store`.
///
/// # Errors
/// Returns why the socket cannot be bound or made private.
pub fn run_server_on_unix_socket(path: &Path, store: Store) -> Result<SocketServer, SocketError> {
    remove_socket(path)?;
    let listener = UnixListener::bind(path).map_err(|error| io_error(path, error))?;
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE)).map_err(|error| io_error(path, error))?;
    listener.set_nonblocking(true).map_err(|error| io_error(path, error))?;
    let stop = Arc::new(AtomicBool::new(false));
    let accepting_stop = Arc::clone(&stop);
    let accepting = thread::Builder::new()
        .name("horizon-sync-socket".to_owned())
        .spawn(move || accept_links(&listener, &store, &accepting_stop))
        .map_err(|error| io_error(path, error))?;
    Ok(SocketServer {
        path: path.to_path_buf(),
        stop,
        accepting: Some(accepting),
    })
}

/// Answers one link of `listener` per connection, one thread each.
fn accept_links(listener: &UnixListener, store: &Store, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _address)) => {
                // A link reads and writes with blocking calls. macOS hands an
                // accepted socket the non-blocking mode of its listener; Linux
                // does not.
                if let Err(error) = stream.set_nonblocking(false) {
                    tracing::warn!(%error, "a link of the server socket could not be made blocking");
                    continue;
                }
                let Ok(reading) = stream.try_clone() else {
                    tracing::warn!("a link of the server socket could not be read");
                    continue;
                };
                let store = store.clone();
                let spawned = thread::Builder::new()
                    .name("horizon-sync-connection".to_owned())
                    .spawn(move || {
                        if let Err(error) = serve(reading, stream, &store) {
                            tracing::debug!(%error, "a link ended");
                        }
                    });
                if let Err(error) = spawned {
                    tracing::warn!(%error, "a link of the server socket could not start");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_PAUSE),
            Err(error) => {
                tracing::warn!(%error, "the server socket could not accept a link");
                thread::sleep(ACCEPT_PAUSE);
            }
        }
    }
}

/// Relays the frames of one child of this computer: its stdin and stdout carry
/// the bytes of the link, and the socket of the server is the other side.
///
/// # Errors
/// Returns why the socket cannot be reached. A link that ends answers `Ok(())`.
pub fn run_link_stdio_to_unix_socket(path: &Path) -> Result<(), SocketError> {
    relay(io::stdin(), io::stdout(), path)
}

/// Relays one pair of streams to `path` until either side ends.
///
/// # Errors
/// Returns why the socket cannot be reached.
pub fn relay<R, W>(reader: R, writer: W, path: &Path) -> Result<(), SocketError>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let socket = UnixStream::connect(path).map_err(|error| io_error(path, error))?;
    let outward = socket.try_clone().map_err(|error| io_error(path, error))?;
    let carrying = thread::Builder::new()
        .name("horizon-sync-relay".to_owned())
        .spawn(move || {
            let mut outward = outward;
            copy(reader, &mut outward);
            let _ = outward.shutdown(Shutdown::Write);
        })
        .map_err(|error| io_error(path, error))?;
    let mut inward = socket;
    let mut writer = writer;
    copy(&mut inward, &mut writer);
    let _ = carrying.join();
    Ok(())
}

/// Copies `from` to `to` until `from` ends, then ends the write half of `to` when
/// it is a socket, so the other side reads the end too.
fn copy<R: Read, W: Write>(mut from: R, to: &mut W) {
    let mut buffer = vec![0u8; RELAY_BUFFER];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                if to.write_all(&buffer[..read]).is_err() {
                    return;
                }
                let _ = to.flush();
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Removes a socket file that a stopped server left. A file that is not a socket
/// is left alone, so no path of the person is deleted by mistake.
fn remove_socket(path: &Path) -> Result<(), SocketError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|error| io_error(path, error))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn io_error(path: &Path, error: io::Error) -> SocketError {
    SocketError::Io {
        path: path.to_path_buf(),
        error,
    }
}
