//! Default-off proof of original Serve ingress through a private Unix backend.
//! Neither a PROXY header nor a serialized identity is connection authority.
mod accepted;
mod ancillary;
mod backend;
pub use accepted::AcceptedStream;
#[cfg(test)]
pub(crate) use accepted::transport_fixture;
mod daemon;
mod procfs;
mod proxy;
mod whois;

use daemon::Daemon;
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::UnixStream,
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

const IDENTIFY_TIME: Duration = Duration::from_secs(3);
const MAX_PENDING: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Trusted producer requires the root worker controller")]
    Root,
    #[error("Original daemon, enrollment or socket custody could not be proved")]
    Custody,
    #[error("Original socket observations changed during verification")]
    Observation,
    #[error("Trusted producer identification is at capacity")]
    Busy,
    #[error("Trusted producer identification expired or was cancelled")]
    Deadline,
    #[error("Invalid bounded producer response")]
    Encoding,
    #[error("Trusted producer transport failed")]
    Io(#[from] io::Error),
}
impl From<rustix::io::Errno> for Error {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

/// Fixed root-managed daemon endpoints. Arbitrary runtime paths are not accepted.
#[derive(Clone, Copy)]
pub enum Runtime {
    Worker,
    Kernel,
}
impl Runtime {
    fn socket(self) -> &'static str {
        match self {
            Self::Worker => "/run/horizon-tailnet/tailscaled.sock",
            Self::Kernel => "/run/tailscale/tailscaled.sock",
        }
    }
}

/// One original daemon and enrollment generation, with immutable Serve/backend ports.
/// Root owns that service binding; this type does not create or adopt Serve config.
pub struct Producer {
    daemon: Daemon,
    generation: whois::Generation,
    virtual_port: u16,
    backend_port: u16,
    backend: backend::Backend,
    pending: Arc<Semaphore>,
}
impl Producer {
    /// Pin live UNIX credentials and Running Self identity before accepting headers.
    ///
    /// # Errors
    /// Refuses non-root callers, untrusted paths, absent enrollment and invalid ports.
    pub async fn pin(runtime: Runtime, virtual_port: u16, backend_port: u16) -> Result<Self, Error> {
        if !rustix::process::geteuid().is_root() {
            return Err(Error::Root);
        }
        if !(20000..=20031).contains(&virtual_port) || !(20032..=20063).contains(&backend_port) {
            return Err(Error::Encoding);
        }
        let until = Instant::now() + IDENTIFY_TIME;
        let daemon = Daemon::pin(runtime.socket(), until).await?;
        let generation = daemon.generation(until).await?;
        let backend = backend::Backend::bind(runtime, backend_port)?;
        Ok(Self {
            daemon,
            generation,
            virtual_port,
            backend_port,
            backend,
            pending: Arc::new(Semaphore::new(MAX_PENDING)),
        })
    }

    #[must_use]
    pub fn virtual_port(&self) -> u16 {
        self.virtual_port
    }

    #[must_use]
    pub fn backend_port(&self) -> u16 {
        self.backend_port
    }

    #[must_use]
    pub fn backend_path(&self) -> &std::path::Path {
        self.backend.path()
    }

    /// Accept from the original root-private listener before bounded identification.
    ///
    /// # Errors
    /// Refuses changed listener paths or transport failure.
    pub async fn accept(&self) -> Result<UnixStream, Error> {
        self.backend.accept().await
    }

    /// Finish only after every cancelled CLI supervisor has killed and reaped its child.
    /// The consuming owner must first stop and join its identification callers.
    ///
    /// # Errors
    /// A bounded drain failure supplies no clean retirement acknowledgement.
    pub async fn retire(self) -> Result<(), Error> {
        let daemon = self.daemon.retire(Instant::now() + IDENTIFY_TIME).await;
        let backend = self.backend.retire();
        daemon.and(backend)
    }

    /// Identify an owned accepted stream. Consuming the result preserves that stream.
    /// Dropping this future shuts the stream; a CLI child is killed and reaped separately.
    ///
    /// # Errors
    /// Every ambiguous, stale, oversized or unsupported observation refuses admission.
    pub fn identify(&self, stream: UnixStream) -> impl Future<Output = Result<AcceptedPeer, Error>> + '_ {
        let admission = begin_identification(stream, Arc::clone(&self.pending));
        async move {
            let (stream, _slot) = admission.await?;
            self.identify_owned(stream).await
        }
    }

    async fn identify_owned(&self, mut stream: Closing) -> Result<AcceptedPeer, Error> {
        let until = Instant::now() + IDENTIFY_TIME;
        let socket = stream.payload.as_ref().ok_or(Error::Custody)?;
        self.backend.verify_stream(socket, until)?;
        self.daemon.prove(socket, until)?;
        self.same_generation(until).await?;
        self.daemon.prove(socket, until)?;
        let socket = stream.payload.as_ref().ok_or(Error::Custody)?;
        stream.control = Some(ancillary::receipt(socket, until).await?);
        let control = stream.control.as_ref().ok_or(Error::Custody)?;
        self.backend.verify_stream(socket, until)?;
        self.daemon.prove(socket, until)?;
        control.prove(&self.daemon, until)?;
        control.live()?;
        let header = proxy::read(socket, until).await?;
        verify_destination(header.destination, self.virtual_port, &self.generation)?;
        let socket = stream.payload.as_ref().ok_or(Error::Custody)?;
        self.backend.verify_stream(socket, until)?;
        self.daemon.prove(socket, until)?;
        let identity = self.daemon.whois(header.source, until).await?;
        self.same_generation(until).await?;
        self.backend.verify_stream(socket, until)?;
        self.daemon.prove(socket, until)?;
        let control = stream.control.as_ref().ok_or(Error::Custody)?;
        control.prove(&self.daemon, until)?;
        control.live()?;
        Ok(AcceptedPeer {
            stream: AcceptedStream::new(
                stream.payload.take().ok_or(Error::Custody)?,
                stream.control.take().ok_or(Error::Custody)?,
            ),
            identity,
            source: header.source,
        })
    }

    async fn same_generation(&self, until: Instant) -> Result<(), Error> {
        if self.daemon.generation(until).await? != self.generation {
            return Err(Error::Custody);
        }
        Ok(())
    }
}

fn verify_destination(destination: SocketAddr, virtual_port: u16, generation: &whois::Generation) -> Result<(), Error> {
    // The pinned Unix Serve patch writes the virtual port; no TCP backend exists.
    if destination.port() != virtual_port || !generation.addresses.contains(&destination.ip()) {
        return Err(Error::Custody);
    }
    Ok(())
}

/// Opaque connection-bound facts. There is no Deserialize, Clone or public constructor.
pub struct AcceptedPeer {
    stream: AcceptedStream,
    identity: whois::Identity,
    source: SocketAddr,
}
impl AcceptedPeer {
    #[must_use]
    pub fn stable_id(&self) -> &str {
        &self.identity.stable_id
    }
    #[must_use]
    pub fn full_dns_name(&self) -> &str {
        &self.identity.name
    }
    #[must_use]
    pub fn tags(&self) -> &[String] {
        &self.identity.tags
    }
    #[must_use]
    pub fn original_source(&self) -> SocketAddr {
        self.source
    }
    /// Transfer custody of this original stream, never a caller-supplied replacement.
    #[must_use]
    pub fn into_stream(self) -> AcceptedStream {
        self.stream
    }
}

struct Closing {
    payload: Option<UnixStream>,
    control: Option<accepted::Control>,
}
impl Closing {
    fn new(payload: UnixStream) -> Self {
        Self {
            payload: Some(payload),
            control: None,
        }
    }
}

fn begin_identification(
    stream: UnixStream,
    pending: Arc<Semaphore>,
) -> impl Future<Output = Result<(Closing, OwnedSemaphorePermit), Error>> {
    // Construct before returning the future: unpolled cancellation still shuts the socket.
    let stream = Closing::new(stream);
    async move {
        let slot = pending.try_acquire_owned().map_err(|_| Error::Busy)?;
        Ok((stream, slot))
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        if let Some(stream) = &self.payload {
            let _ = rustix::net::shutdown(stream, rustix::net::Shutdown::Both);
        }
    }
}
fn live(until: Instant) -> Result<(), Error> {
    if Instant::now() >= until {
        return Err(Error::Deadline);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
