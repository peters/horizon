//! Original per-flow payload and cancellation custody; no independent authority.
use super::ancillary;
use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::UnixStream,
};

pub(super) struct Control {
    state: Mutex<ControlState>,
    reader: tokio::sync::Mutex<()>,
}

const DRAINED: u8 = 0x01;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ack {
    Waiting,
    Drained,
    Complete,
}

struct ControlState {
    endpoint: Option<Arc<UnixStream>>,
    acknowledged: Ack,
}

impl Control {
    pub(super) fn new(fd: OwnedFd) -> io::Result<Self> {
        validate(&fd)?;
        let socket = std::os::unix::net::UnixStream::from(fd);
        let endpoint = UnixStream::from_std(socket)?;
        Ok(Self {
            state: Mutex::new(ControlState {
                endpoint: Some(Arc::new(endpoint)),
                acknowledged: Ack::Waiting,
            }),
            reader: tokio::sync::Mutex::new(()),
        })
    }

    pub(super) fn with_endpoint<T>(&self, check: impl FnOnce(&UnixStream) -> io::Result<T>) -> io::Result<T> {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        check(guard.endpoint.as_deref().ok_or_else(ancillary::invalid)?)
    }

    pub(super) fn live(&self) -> io::Result<()> {
        self.with_endpoint(|endpoint| {
            let mut byte = [0];
            match rustix::net::recv(
                endpoint,
                &mut byte[..],
                rustix::net::RecvFlags::DONTWAIT | rustix::net::RecvFlags::PEEK,
            ) {
                Err(rustix::io::Errno::AGAIN) => Ok(()),
                Err(error) => Err(error.into()),
                Ok(_) => Err(ancillary::invalid()),
            }
        })
    }

    fn cancel(&self) {
        let endpoint = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .endpoint
            .take();
        if let Some(endpoint) = endpoint {
            // A finish future may retain a private Arc: shutdown wakes it now,
            // rather than waiting for its reference to disappear.
            let _ = rustix::net::shutdown(endpoint.as_ref(), rustix::net::Shutdown::Both);
        }
    }

    pub(super) fn prove(&self, daemon: &super::Daemon, until: tokio::time::Instant) -> Result<(), super::Error> {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        daemon.prove(guard.endpoint.as_deref().ok_or(super::Error::Custody)?, until)
    }

    async fn finish(&self) -> io::Result<()> {
        let _reader = self.reader.lock().await;
        let result = self.receive_finish().await;
        if result.is_err() {
            self.cancel();
        }
        result
    }

    async fn receive_finish(&self) -> io::Result<()> {
        loop {
            let (endpoint, acknowledged) = {
                let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                (
                    guard.endpoint.clone().ok_or_else(ancillary::invalid)?,
                    guard.acknowledged,
                )
            };
            if acknowledged == Ack::Complete {
                return Ok(());
            }
            let byte = ancillary::byte(&endpoint, false).await?.0;
            if (acknowledged == Ack::Waiting && byte != Some(DRAINED))
                || (acknowledged == Ack::Drained && byte.is_some())
            {
                return Err(ancillary::invalid());
            }
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.endpoint.is_none() {
                return Err(ancillary::invalid());
            }
            // Preserve progress if the caller drops this future on a Budget
            // transition between DRAINED and EOF. A retry cannot consume it twice.
            guard.acknowledged = match acknowledged {
                Ack::Waiting => Ack::Drained,
                Ack::Drained | Ack::Complete => Ack::Complete,
            };
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn validate(fd: &OwnedFd) -> io::Result<()> {
    use rustix::net::{SocketAddrUnix, SocketType, sockopt};
    // Compare the original encoded addresses. Locked rustix's TryFrom maps an
    // unnamed Linux address to abstract-empty; those must remain distinct.
    let unnamed = rustix::net::SocketAddrAny::from(SocketAddrUnix::new_unnamed());
    let local = rustix::net::getsockname(fd)?;
    let peer = rustix::net::getpeername(fd)?.ok_or_else(ancillary::invalid)?;
    if sockopt::socket_type(fd)? != SocketType::STREAM
        || sockopt::socket_acceptconn(fd)?
        || local != unnamed
        || peer != unnamed
        || !rustix::io::fcntl_getfd(fd)?.contains(rustix::io::FdFlags::CLOEXEC)
        || !rustix::fs::fcntl_getfl(fd)?.contains(rustix::fs::OFlags::NONBLOCK)
    {
        return Err(ancillary::invalid());
    }
    Ok(())
}

/// Original Producer payload with inseparable, private whole-flow cancellation.
/// This type cannot be constructed, cloned or restored by its consumers.
pub struct AcceptedStream {
    payload: UnixStream,
    control: Control,
}

impl AcceptedStream {
    pub(super) fn new(payload: UnixStream, control: Control) -> Self {
        Self { payload, control }
    }

    /// Immediately cancel the original payload and control, including held readers.
    pub fn cancel(&self) {
        let _ = rustix::net::shutdown(&self.payload, rustix::net::Shutdown::Both);
        self.control.cancel();
    }

    /// Wait for original daemon `DRAINED` plus EOF, preserving interrupted progress.
    /// The owner must bound this wait by its existing flow/grace deadline and
    /// cancellation. Dropping the wait retains custody for a later continuation.
    ///
    /// # Errors
    /// Refuses cancellation, missing/wrong/extra acknowledgement or control I/O failure.
    pub async fn finish(&self) -> io::Result<()> {
        let result = self.control.finish().await;
        if result.is_err() {
            self.cancel();
        }
        result
    }
    /// Wait for readiness of the original payload only.
    ///
    /// # Errors
    /// Returns the original payload's readiness error.
    pub async fn readable(&self) -> io::Result<()> {
        self.payload.readable().await
    }
    /// Wait for write readiness of the original payload only.
    ///
    /// # Errors
    /// Returns the original payload's readiness error.
    pub async fn writable(&self) -> io::Result<()> {
        self.payload.writable().await
    }
    /// Read available original payload bytes without blocking.
    ///
    /// # Errors
    /// Returns the original payload error, including `WouldBlock`.
    pub fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        self.payload.try_read(bytes)
    }
    /// Write available original payload capacity without blocking.
    ///
    /// # Errors
    /// Returns the original payload error, including `WouldBlock`.
    pub fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        self.payload.try_write(bytes)
    }
    /// Half-close the original payload while retaining whole-flow cancellation.
    ///
    /// # Errors
    /// Returns the original payload's write-shutdown error.
    pub fn shutdown_write(&self) -> io::Result<()> {
        rustix::net::shutdown(&self.payload, rustix::net::Shutdown::Write).map_err(Into::into)
    }
}

impl AsFd for AcceptedStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.payload.as_fd()
    }
}
impl AsyncRead for AcceptedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().payload).poll_read(cx, buf)
    }
}
impl AsyncWrite for AcceptedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().payload).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().payload).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().payload).poll_shutdown(cx)
    }
}
impl Drop for AcceptedStream {
    fn drop(&mut self) {
        self.cancel();
    }
}

// Mechanical test transport only: no identity, AcceptedPeer or named authority.
#[cfg(test)]
pub(crate) fn transport_fixture(payload: UnixStream, endpoint: UnixStream) -> AcceptedStream {
    let fd = OwnedFd::from(endpoint.into_std().unwrap());
    AcceptedStream::new(payload, Control::new(fd).unwrap())
}

#[cfg(test)]
#[path = "accepted/tests.rs"]
mod tests;
