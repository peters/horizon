//! The server home, and the two modes that a person starts: the server itself,
//! and the link that relays the standard input and output of one `ssh` child to
//! the socket of that server.
//!
//! Both modes are the same `horizon` binary with one flag. They touch one
//! directory only, the server home: [`ServerHome::resolve`] names it from
//! `HORIZON_SERVER_HOME` or `~/.horizon-server`, and every file of the server
//! lives there. They read no setting, open no window, and install no agent
//! integration, so a server never reads or writes `~/.horizon` of the person.
//!
//! The socket is `0600` in a `0700` directory of the server user. One lock file
//! makes a second server of the same home refuse instead of removing the socket
//! of the first one, and a stop flag ends the first one cleanly: it stops
//! accepting, removes the socket, and closes the store.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io,
    io::Read,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::error::StoreError;
use crate::link::{SocketError, relay, run_link_stdio_to_unix_socket, run_server_on_unix_socket};
use crate::listener::{ListenError, ListenerInfo, TcpServer, run_server_on_tcp};
use crate::server::ServerInfo;
use crate::store::Store;

/// The mode of the server home and of its socket: the server user alone reaches
/// it.
const HOME_MODE: u32 = 0o700;
/// The mode of the lock file of the server home.
const FILE_MODE: u32 = 0o600;

/// How long the server waits before it looks at its stop flag again.
const STOP_PAUSE: Duration = Duration::from_millis(50);

/// The exit code of a link that found no server. It is fixed and not zero, so a
/// script can wait for a server and try again without reading the text.
pub const EXIT_NO_SERVER: i32 = 3;

/// Why a mode could not do its work.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// This computer has no home directory, so it has no default server home.
    #[error("this computer has no home directory, so it has no server home; set HORIZON_SERVER_HOME")]
    NoHome,
    /// A directory or a file of the server home could not be read or written.
    #[error("{path}: {error}")]
    Io {
        /// The path of the file or directory.
        path: PathBuf,
        /// The reason the system gave.
        error: io::Error,
    },
    /// Another server already answers for this home.
    #[error("a Horizon server already runs for {home}; stop it before you start another one")]
    AlreadyRunning {
        /// The server home that the running server holds.
        home: PathBuf,
    },
    /// No server answers the socket of this home.
    #[error("no Horizon server answers at {path}; start `horizon --server` on that computer")]
    NoServer {
        /// The socket path that the link tried.
        path: PathBuf,
    },
    /// The link of the socket failed.
    #[error(transparent)]
    Socket(#[from] SocketError),
    /// The listener of `--listen` could not be opened.
    #[error(transparent)]
    Listener(#[from] ListenError),
    /// The store of the server could not be opened.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl HostError {
    /// The exit code of the mode that failed. A link that found no server has its
    /// own code, so a caller can wait for the server and try again.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NoServer { .. } => EXIT_NO_SERVER,
            Self::NoHome
            | Self::Io { .. }
            | Self::AlreadyRunning { .. }
            | Self::Socket(_)
            | Self::Listener(_)
            | Self::Store(_) => 1,
        }
    }
}

/// The directory that holds everything a server has: its store, its socket and
/// the lock that says that it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerHome {
    root: PathBuf,
}

impl ServerHome {
    /// The home at `root`.
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The server home that this start uses: the path it named, then
    /// `HORIZON_SERVER_HOME`, then `~/.horizon-server`.
    ///
    /// # Errors
    /// Returns why this start cannot name a home.
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self, HostError> {
        Self::resolve_with(
            explicit,
            std::env::var_os("HORIZON_SERVER_HOME").map(PathBuf::from),
            horizon_core::user_home_dir(),
        )
    }

    /// The rule of [`Self::resolve`], with its three inputs named, so a test
    /// reads the rule without the environment of the process.
    fn resolve_with(
        explicit: Option<PathBuf>,
        from_env: Option<PathBuf>,
        home: Option<PathBuf>,
    ) -> Result<Self, HostError> {
        explicit
            .filter(|path| !path.as_os_str().is_empty())
            .or(from_env.filter(|path| !path.as_os_str().is_empty()))
            .or_else(|| home.map(|home| home.join(".horizon-server")))
            .map(Self::at)
            .ok_or(HostError::NoHome)
    }

    /// The directory of this server.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The socket that the links of the computers reach.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.root.join("server.sock")
    }

    /// The store of the server.
    #[must_use]
    pub fn store_path(&self) -> PathBuf {
        self.root.join("server.sqlite")
    }

    /// The file that says that a server runs, so a second one refuses.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("server.lock")
    }

    /// Makes the directory private, so every file of the server is the server
    /// user's alone.
    ///
    /// # Errors
    /// Returns why the directory cannot be made or be made private.
    pub fn prepare(&self) -> Result<(), HostError> {
        fs::create_dir_all(&self.root).map_err(|error| io_error(&self.root, error))?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(HOME_MODE))
            .map_err(|error| io_error(&self.root, error))
    }

    /// Takes the lock of this home, so one server runs for it.
    ///
    /// # Errors
    /// Returns [`HostError::AlreadyRunning`] when another server holds the lock,
    /// and why the lock file cannot be made.
    pub fn lock(&self) -> Result<ServerLock, HostError> {
        let path = self.lock_path();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(FILE_MODE)
            .open(&path)
            .map_err(|error| io_error(&path, error))?;
        match file.try_lock() {
            Ok(()) => Ok(ServerLock { file }),
            Err(TryLockError::WouldBlock) => Err(HostError::AlreadyRunning {
                home: self.root.clone(),
            }),
            Err(TryLockError::Error(error)) => Err(io_error(&path, error)),
        }
    }
}

/// The lock of one server home. The lock ends when this value drops.
#[derive(Debug)]
pub struct ServerLock {
    file: File,
}

impl Drop for ServerLock {
    fn drop(&mut self) {
        // The descriptor is the lock: closing it releases the lock for the next
        // server, and an explicit unlock only makes a second release possible.
        let _ = self.file.unlock();
    }
}

/// The listener that a server opens for the computers that join it with a code,
/// and the name that its links hold.
///
/// Both are absent without `--listen`, and then the only transport is the socket
/// of the server home, which `ssh` reaches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOptions {
    /// `--listen <ip>:<port>`: the address of the TCP listener.
    pub listen: Option<String>,
    /// `--name <dns-name>`: the name that a link of this server holds instead of
    /// the address of the listener.
    pub name: Option<String>,
}

impl ServerOptions {
    /// The listener of these options, or `None` when they name none.
    ///
    /// # Errors
    /// Refuses an address outside the three ranges of a link, an address that is
    /// not one IP address and one port, and a name that is not one DNS name.
    pub fn listener(&self) -> Result<Option<ListenerInfo>, ListenError> {
        let Some(listen) = self.listen.as_deref().map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        ListenerInfo::new(listen, self.name.as_deref()).map(Some)
    }
}

/// Answers the links of `home` until `stop` is set, and then ends cleanly.
///
/// The store is opened by this call and closed before it returns, so a stop
/// leaves a checkpointed file and no socket behind. A second server of the same
/// home refuses before it touches the socket of the first one.
///
/// # Errors
/// Returns why the home, its lock, the store or the socket failed.
pub fn serve_until_stopped(home: &ServerHome, stop: &AtomicBool) -> Result<(), HostError> {
    serve_with_options(home, stop, &ServerOptions::default())
}

/// Answers the links of `home` until `stop` is set, with the listener of
/// `options` when it names one.
///
/// # Errors
/// Returns why the home, its lock, the store, the socket or the listener failed.
pub fn serve_with_options(home: &ServerHome, stop: &AtomicBool, options: &ServerOptions) -> Result<(), HostError> {
    home.prepare()?;
    let _lock = home.lock()?;
    let listener = options.listener()?;
    let store = Store::open(&home.store_path())?;
    let socket = home.socket_path();
    tracing::info!(
        home = %home.root().display(),
        socket = %socket.display(),
        listener = ?listener.as_ref().map(ListenerInfo::address),
        "the Horizon server answers"
    );
    let info = ServerInfo::with_listener(listener.clone());
    let server = run_server_on_unix_socket(&socket, store.clone(), &info)?;
    let tcp: Option<TcpServer> = match listener {
        Some(listener) => match run_server_on_tcp(&listener, store.clone(), &info) {
            Ok(tcp) => Some(tcp),
            Err(error) => {
                server.stop();
                store.close();
                return Err(HostError::Listener(error));
            }
        },
        None => None,
    };
    while !stop.load(Ordering::SeqCst) {
        thread::sleep(STOP_PAUSE);
    }
    if let Some(tcp) = tcp {
        tcp.stop();
    }
    server.stop();
    store.close();
    tracing::info!("the Horizon server stopped");
    Ok(())
}

/// Relays the standard input and output of one child to the socket of `home`.
/// The child is the `ssh` of one computer, and it starts no server: a home
/// without a server answers [`HostError::NoServer`].
///
/// # Errors
/// Returns why the socket cannot be reached or the bytes cannot be moved.
pub fn link_stdio(home: &ServerHome) -> Result<(), HostError> {
    run_link_stdio_to_unix_socket(&home.socket_path()).map_err(host_socket_error)
}

/// Relays one pair of streams to the socket of `home`, for a caller that is not
/// the standard input and output of the process.
///
/// # Errors
/// Returns why the socket cannot be reached or the bytes cannot be moved.
pub fn link_streams<R, W>(home: &ServerHome, reader: R, writer: W) -> Result<(), HostError>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    relay(reader, writer, &home.socket_path()).map_err(host_socket_error)
}

/// The stop flag of the server mode: `SIGTERM` and `SIGINT` set it, so a stop
/// of the service ends the server the same way a person does.
///
/// # Errors
/// Returns why the signal handlers cannot be installed.
pub fn terminate_flag() -> io::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&stop))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop))?;
    Ok(stop)
}

/// A socket that answers nothing means that the server is not there, which a
/// caller reports as its own reason instead of a raw system error.
fn host_socket_error(error: SocketError) -> HostError {
    match &error {
        SocketError::Io { path, error }
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
            ) =>
        {
            HostError::NoServer { path: path.clone() }
        }
        SocketError::Io { .. } => HostError::Socket(error),
    }
}

fn io_error(path: &Path, error: io::Error) -> HostError {
    HostError::Io {
        path: path.to_path_buf(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::{EXIT_NO_SERVER, HostError, ServerHome, link_streams, serve_until_stopped};

    fn home(directory: &tempfile::TempDir) -> ServerHome {
        ServerHome::at(directory.path().join("server"))
    }

    #[test]
    fn the_home_is_the_named_path_then_the_variable_then_the_user_home() {
        let named = ServerHome::resolve_with(
            Some("/named/server".into()),
            Some("/variable/server".into()),
            Some("/home/person".into()),
        )
        .expect("a named home");
        assert_eq!(named.root(), std::path::Path::new("/named/server"));

        let variable = ServerHome::resolve_with(None, Some("/variable/server".into()), Some("/home/person".into()))
            .expect("a home from the variable");
        assert_eq!(variable.root(), std::path::Path::new("/variable/server"));

        let default = ServerHome::resolve_with(None, None, Some("/home/person".into())).expect("the default");
        assert_eq!(default.root(), std::path::Path::new("/home/person/.horizon-server"));

        let empty = ServerHome::resolve_with(Some("".into()), Some("".into()), None).expect_err("no home");
        assert!(matches!(empty, HostError::NoHome));
        assert_eq!(empty.exit_code(), 1);
    }

    #[test]
    fn the_home_is_private_and_holds_the_socket_the_store_and_the_lock() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = home(&directory);
        home.prepare().expect("the home");
        assert_eq!(
            std::fs::metadata(home.root()).expect("the home").permissions().mode() & 0o777,
            0o700,
            "the server home is the server user's alone"
        );
        assert_eq!(home.socket_path(), home.root().join("server.sock"));
        assert_eq!(home.store_path(), home.root().join("server.sqlite"));
        assert_eq!(home.lock_path(), home.root().join("server.lock"));
    }

    #[test]
    fn a_second_server_of_one_home_refuses_before_it_touches_the_socket() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = home(&directory);
        let stop = Arc::new(AtomicBool::new(false));
        let running = {
            let (home, stop) = (home.clone(), Arc::clone(&stop));
            thread::spawn(move || serve_until_stopped(&home, &stop))
        };
        wait_for(|| home.socket_path().exists());
        let refused = serve_until_stopped(&home, &AtomicBool::new(false)).expect_err("a second server");
        assert!(matches!(refused, HostError::AlreadyRunning { .. }), "{refused}");
        assert_eq!(refused.exit_code(), 1);
        assert!(
            home.socket_path().exists(),
            "the refused server leaves the socket of the running one"
        );
        stop.store(true, Ordering::SeqCst);
        running.join().expect("the server thread").expect("a clean stop");
        assert!(!home.socket_path().exists(), "a stopped server removes its socket");
    }

    #[test]
    fn a_link_that_finds_no_server_answers_its_own_reason_and_exit_code() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = home(&directory);
        home.prepare().expect("the home");
        let refused = link_streams(&home, &b""[..], Vec::new()).expect_err("no server");
        assert!(matches!(refused, HostError::NoServer { .. }), "{refused}");
        assert_eq!(refused.exit_code(), EXIT_NO_SERVER);
        assert_ne!(EXIT_NO_SERVER, 0);
    }

    /// The hard rule of the two modes: a home of their own, and nothing else
    /// touched. The person's home stands beside the server home and must stay
    /// exactly as it was.
    #[test]
    fn a_server_and_a_link_touch_the_server_home_only() {
        let directory = tempfile::tempdir().expect("a directory");
        let person = directory.path().join("person");
        std::fs::create_dir(&person).expect("the person's home");
        let home = home(&directory);
        let stop = Arc::new(AtomicBool::new(false));
        let running = {
            let (home, stop) = (home.clone(), Arc::clone(&stop));
            thread::spawn(move || serve_until_stopped(&home, &stop))
        };
        wait_for(|| home.socket_path().exists());

        // One link, over streams that stand in for the standard input and
        // output of an `ssh` child.
        let (requests, mut request) = std::io::pipe().expect("a pipe");
        let (mut answer, answers) = std::io::pipe().expect("a pipe");
        let relaying = {
            let home = home.clone();
            thread::spawn(move || link_streams(&home, requests, answers))
        };
        let hello = crate::Frame::Hello(crate::Hello {
            protocol: crate::PROTOCOL_VERSION,
            device: "test-device".to_owned(),
            store: None,
            token: None,
        });
        crate::write_frame(&mut request, &hello).expect("the hello");
        let answered = crate::read_frame(&mut answer).expect("the answer").expect("one frame");
        assert!(matches!(answered, crate::Frame::HelloOk(_)), "{answered:?}");
        drop(request);
        drop(answer);
        relaying.join().expect("the relay").expect("a clean relay");

        stop.store(true, Ordering::SeqCst);
        running.join().expect("the server thread").expect("a clean stop");

        let names: Vec<String> = std::fs::read_dir(directory.path())
            .expect("the directory")
            .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            sorted(&names),
            vec!["person".to_string(), "server".to_string()],
            "the modes create nothing outside the server home"
        );
        assert_eq!(
            std::fs::read_dir(&person).expect("the person's home").count(),
            0,
            "no mode writes into the home of the person"
        );
        let inside = sorted(
            &std::fs::read_dir(home.root())
                .expect("the server home")
                .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        assert!(inside.contains(&"server.sqlite".to_string()), "{inside:?}");
    }

    fn sorted(names: &[String]) -> Vec<String> {
        let mut names = names.to_vec();
        names.sort();
        names
    }

    /// Waits for a state of the server, so a test never reads a socket that the
    /// server did not bind yet.
    fn wait_for(ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ready() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the server did not reach its state in time");
    }

    #[test]
    fn a_stop_flag_ends_the_server_and_releases_its_lock() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = home(&directory);
        let stop = Arc::new(AtomicBool::new(false));
        let (asked, asking) = mpsc::channel();
        let running = {
            let (home, stop) = (home.clone(), Arc::clone(&stop));
            thread::spawn(move || {
                asked.send(()).expect("the test waits");
                serve_until_stopped(&home, &stop)
            })
        };
        asking.recv().expect("the server started");
        wait_for(|| home.socket_path().exists());
        stop.store(true, Ordering::SeqCst);
        running.join().expect("the server thread").expect("a clean stop");
        let lock = home.lock().expect("a stopped server releases its lock");
        drop(lock);
        assert!(
            home.lock_path().exists(),
            "the lock file stays after a stop; the lock itself is the descriptor of the running server"
        );
    }

    /// A server that was killed leaves its lock file, its socket and the
    /// write-ahead files of SQLite. The next start must serve a home in that state
    /// instead of refusing it: the lock is the open file of the running process,
    /// and a socket that no process listens on is stale.
    #[test]
    fn a_home_that_a_killed_server_left_starts_again() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = home(&directory);
        home.prepare().expect("the home");
        std::fs::write(home.lock_path(), b"").expect("a lock file of a killed server");
        let stale = std::os::unix::net::UnixListener::bind(home.socket_path()).expect("a socket of a killed server");
        drop(stale);
        // A killed server leaves its store, its write-ahead log and its shared
        // index behind. The three files of a store that was writing are copied
        // here, because a store that closed cleanly checkpoints its log away.
        let writing = directory.path().join("killed");
        std::fs::create_dir(&writing).expect("the directory of the killed server");
        let store = crate::Store::open(&writing.join("server.sqlite")).expect("the store of the killed server");
        store
            .push(
                "test-device",
                &[crate::Intent {
                    device: "test-device".to_owned(),
                    id: 1,
                    seen: None,
                    op: crate::Op::MachineRemove {
                        name: "build-1".to_owned(),
                    },
                }],
            )
            .expect("one change of the killed server");
        for name in ["server.sqlite", "server.sqlite-wal", "server.sqlite-shm"] {
            let from = writing.join(name);
            assert!(from.exists(), "the killed server left {name}");
            std::fs::copy(&from, home.root().join(name)).expect("a file of the killed server");
        }
        store.close();

        let stop = Arc::new(AtomicBool::new(false));
        let running = {
            let (home, stop) = (home.clone(), Arc::clone(&stop));
            thread::spawn(move || serve_until_stopped(&home, &stop))
        };
        // The socket file of a killed server stays after its process is gone, so
        // the test waits for a link that the new server answers and not for a name.
        wait_for(|| answers_a_link(&home));

        stop.store(true, Ordering::SeqCst);
        running.join().expect("the server thread").expect("a clean stop");
        assert!(!home.socket_path().exists(), "a stopped server removes its socket");
        assert!(
            home.lock_path().exists(),
            "the lock file of the home stays, as after every stop"
        );
    }

    /// Whether the server of `home` answers the hello of one link now.
    fn answers_a_link(home: &ServerHome) -> bool {
        let (requests, mut request) = std::io::pipe().expect("a pipe");
        let (mut answer, answers) = std::io::pipe().expect("a pipe");
        let relaying = {
            let home = home.clone();
            thread::spawn(move || link_streams(&home, requests, answers))
        };
        let hello = crate::Frame::Hello(crate::Hello {
            protocol: crate::PROTOCOL_VERSION,
            device: "test-device".to_owned(),
            store: None,
            token: None,
        });
        let _ = crate::write_frame(&mut request, &hello);
        let answered = crate::read_frame(&mut answer).ok().flatten();
        drop(request);
        drop(answer);
        let _ = relaying.join();
        matches!(answered, Some(crate::Frame::HelloOk(_)))
    }
}
