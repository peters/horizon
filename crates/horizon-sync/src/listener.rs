//! The TCP listener of a server: the second transport of the same frames.
//!
//! A server opens it only when a person asks for it with
//! `horizon --server --listen <ip>:<port>`, and only on an address that a link may
//! cross: loopback, `100.64.0.0/10` or `fd7a:115c:a1e0::/48`. Horizon encrypts
//! nothing on this link, so the network must, and those three ranges are the
//! addresses of a private overlay network that does.
//!
//! The address of the listener is also the address of every link that it hands
//! out, unless `--name` gives a name: a person who reaches the server by name
//! gives the name that the other computers resolve.
//!
//! One accepted socket is one link. macOS hands an accepted socket the
//! non-blocking mode of its listener, so the link is made blocking here, as the
//! link of the Unix socket is.

use std::{
    io,
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use horizon_core::LinkError;
use horizon_core::server_link::{JoinLink, ServerAddress, check_address, check_host};

use crate::server::{LinkTransport, ServerInfo, serve_tcp};
use crate::store::Store;

/// How long the server waits before it looks for a stop again, when no computer
/// connects.
const ACCEPT_PAUSE: Duration = Duration::from_millis(10);

/// Why the listener of a server could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    /// The address is not one that a link may cross.
    #[error(transparent)]
    Rule(#[from] LinkError),
    /// The address of the listener is not one IP address and one port.
    #[error(
        "the listener takes one IP address and one port, such as 127.0.0.1:7000: {reason} Use --name for the name that a link of this server shows"
    )]
    Address {
        /// The reason that the text is not one address.
        reason: String,
    },
    /// The name of the listener is not one DNS name.
    #[error("the name of the server is not one DNS name of letters, digits, `.` and `-`")]
    Name,
    /// The port cannot be bound, read or written.
    #[error("the listener at {address}: {error}")]
    Io {
        /// The address that the listener tried.
        address: SocketAddr,
        /// The reason that the system gave.
        error: io::Error,
    },
}

/// The listener of one server: its address, the name that a link of it shows, and
/// the port that its links hold.
///
/// The port is a value of its own because a test binds port 0 and reads the port
/// that the system gave back. A person never writes port 0: the rule of an address
/// refuses it.
#[derive(Clone, Debug)]
pub struct ListenerInfo {
    address: SocketAddr,
    port: Arc<AtomicU16>,
    name: Option<String>,
}

impl ListenerInfo {
    /// The listener of `address`, with `name` for the links that it hands out.
    ///
    /// # Errors
    /// Refuses an address outside the three ranges of a link, a text that is not
    /// one IP address and one port, and a name that is not one DNS name.
    pub fn new(listen: &str, name: Option<&str>) -> Result<Self, ListenError> {
        let address = ServerAddress::parse(listen)?;
        let Some(address) = address.socket_addr() else {
            return Err(ListenError::Address {
                reason: format!("{:?} is not one IP address.", address.host()),
            });
        };
        check_address(address.ip())?;
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        if let Some(name) = name
            && !check_host(name)
        {
            return Err(ListenError::Name);
        }
        Ok(Self {
            address,
            port: Arc::new(AtomicU16::new(address.port())),
            name: name.map(str::to_owned),
        })
    }

    /// One free port of loopback, for a test that reads the port back from the
    /// listener. The bind answers the port, and every link of this listener holds
    /// it.
    #[must_use]
    pub fn free(name: Option<&str>) -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            port: Arc::new(AtomicU16::new(0)),
            name: name.map(str::to_owned),
        }
    }

    /// The address that the listener binds.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The name of the server, when `--name` gave one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The port that a link of this listener holds: the port of its address, or
    /// the port that the system gave it when it bound port 0.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    /// The host that a link of this server holds: the name of `--name`, or the
    /// address of the listener.
    #[must_use]
    pub fn host(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => self.address.ip().to_string(),
        }
    }

    /// The join link of the code of one invitation.
    ///
    /// # Errors
    /// Refuses a code that is not one code.
    pub fn join_link(&self, code: &str) -> Result<String, LinkError> {
        JoinLink::new(&self.host(), self.port(), code).map(|link| link.to_uri())
    }
}

/// A listener of a server that answers links until it is stopped.
#[derive(Debug)]
pub struct TcpServer {
    info: ListenerInfo,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl TcpServer {
    /// The address that this listener bound.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.info.address()
    }

    /// The listener of this server.
    #[must_use]
    pub fn info(&self) -> &ListenerInfo {
        &self.info
    }

    /// Stops accepting links and ends the links that are open. The store stays
    /// with the caller.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
    }
}

/// Binds the address of `info` and answers every link of it with `store`.
///
/// # Errors
/// Refuses an address outside the three ranges of a link, and returns why the
/// listener cannot be bound.
pub fn run_server_on_tcp(info: &ListenerInfo, store: Store, server: &ServerInfo) -> Result<TcpServer, ListenError> {
    check_address(info.address().ip())?;
    let listener = TcpListener::bind(info.address()).map_err(|error| ListenError::Io {
        address: info.address(),
        error,
    })?;
    listener.set_nonblocking(true).map_err(|error| ListenError::Io {
        address: info.address(),
        error,
    })?;
    if let Ok(bound) = listener.local_addr() {
        info.port.store(bound.port(), Ordering::SeqCst);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let accepting_stop = Arc::clone(&stop);
    let accepting_server = server.clone_for_listener();
    let accepting = thread::Builder::new()
        .name("horizon-sync-tcp".to_owned())
        .spawn(move || accept_links(&listener, &store, &accepting_server, &accepting_stop))
        .map_err(|error| ListenError::Io {
            address: info.address(),
            error,
        })?;
    Ok(TcpServer {
        info: info.clone(),
        stop,
        accepting: Some(accepting),
    })
}

/// Answers one link of `listener` per connection, one thread each.
fn accept_links(listener: &TcpListener, store: &Store, server: &ServerInfo, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                // A link reads and writes with blocking calls. macOS hands an
                // accepted socket the non-blocking mode of its listener.
                if let Err(error) = stream.set_nonblocking(false) {
                    tracing::warn!(%error, %peer, "a link of the listener could not be made blocking");
                    continue;
                }
                let (Ok(reading), Ok(closing)) = (stream.try_clone(), stream.try_clone()) else {
                    tracing::warn!(%peer, "a link of the listener could not be read");
                    continue;
                };
                let Ok(writing) = stream.try_clone() else {
                    tracing::warn!(%peer, "a link of the listener could not be written");
                    continue;
                };
                let store = store.clone();
                let server = server.clone_for_listener();
                let transport: Arc<dyn LinkTransport> = Arc::new(closing);
                let spawned = thread::Builder::new()
                    .name("horizon-sync-connection".to_owned())
                    .spawn(move || {
                        if let Err(error) = serve_tcp(reading, writing, &store, &server, transport) {
                            tracing::debug!(%error, %peer, "a link of the listener ended");
                        }
                    });
                if let Err(error) = spawned {
                    tracing::warn!(%error, %peer, "a link of the listener could not start");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_PAUSE),
            Err(error) => {
                tracing::warn!(%error, "the listener could not accept a link");
                thread::sleep(ACCEPT_PAUSE);
            }
        }
    }
}

impl ServerInfo {
    /// The same registry and clock, for the thread of one listener.
    fn clone_for_listener(&self) -> Self {
        Self {
            listener: self.listener.clone(),
            registry: Arc::clone(&self.registry),
            clock: Arc::clone(&self.clock),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listener_refuses_every_address_that_is_not_one_of_a_link() {
        let allowed = ListenerInfo::new("127.0.0.1:7000", None).expect("a loopback listener");
        assert_eq!(allowed.address(), SocketAddr::from(([127, 0, 0, 1], 7000)));
        assert_eq!(allowed.host(), "127.0.0.1");
        assert_eq!(
            allowed.join_link("ABCD-EFGH-JKMN").expect("a link"),
            "horizon://127.0.0.1:7000/join#ABCD-EFGH-JKMN"
        );
        assert_eq!(
            ListenerInfo::new("[::1]:7000", Some("server-computer"))
                .expect("a loopback listener with a name")
                .join_link("ABCD-EFGH-JKMN")
                .expect("a link"),
            "horizon://server-computer:7000/join#ABCD-EFGH-JKMN",
            "the name of the server is the host of the link"
        );
        assert_eq!(
            ListenerInfo::new("127.0.0.1:7000", None)
                .expect("a loopback listener")
                .port(),
            7000
        );
        assert_eq!(
            ListenerInfo::free(None).port(),
            0,
            "the bind answers the port of a free one"
        );
        assert!(
            ListenerInfo::new("100.64.0.1:7000", None).is_ok(),
            "the range of the overlay network is allowed"
        );
        assert!(
            ListenerInfo::new("fd7a:115c:a1e0::1:7000", None).is_err(),
            "an IPv6 address needs brackets"
        );
        assert!(ListenerInfo::new("[fd7a:115c:a1e0::1]:7000", None).is_ok());

        for refused in [
            "0.0.0.0:7000",
            "[::]:7000",
            "8.8.8.8:7000",
            "10.0.0.5:7000",
            "192.168.1.2:7000",
        ] {
            let error = ListenerInfo::new(refused, None).expect_err(refused);
            let sentence = error.to_string();
            assert!(
                sentence.contains("100.64.0.0/10") || sentence.contains("IP address"),
                "{refused}: {sentence}"
            );
        }
        let named = ListenerInfo::new("server-computer:7000", None).expect_err("a name is not an address");
        assert!(named.to_string().contains("--name"), "{named}");
        assert!(
            ListenerInfo::new("127.0.0.1:7000", Some("server_name")).is_err(),
            "an underscore is not one DNS name"
        );
        assert!(horizon_core::allows_address("127.0.0.1".parse().expect("an address")));
        assert!(!horizon_core::allows_address("8.8.8.8".parse().expect("an address")));
    }
}
