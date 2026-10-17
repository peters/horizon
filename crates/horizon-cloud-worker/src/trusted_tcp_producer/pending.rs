//! Sole pre-receipt payload ownership, bound to one original backend acceptance.
use super::{Error, accepted::Control};
use std::{
    io,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::Interest,
    net::UnixStream,
    time::{Instant, timeout_at},
};

pub(super) struct Origin;

pub(super) struct Inner {
    pub(super) payload: UnixStream,
    origin: Arc<Origin>,
    control: Mutex<Option<Arc<Control>>>,
    cancelled: AtomicBool,
}
impl Inner {
    pub(super) fn cancel(&self) {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        self.cancelled.store(true, Ordering::Release);
        let _ = rustix::net::shutdown(&self.payload, rustix::net::Shutdown::Both);
        if let Some(control) = control.take() {
            control.cancel();
        } else {
            // The legitimate daemon's sole control FD is attached to the first P.
            // Linux recv without an ancillary buffer closes those attached rights.
            // Shutdown forbids a new receipt; one nonblocking byte also handles a
            // queued prefix while the original pending owner remains retained.
            let mut prefix = [0];
            let _ = rustix::net::recv(&self.payload, &mut prefix[..], rustix::net::RecvFlags::DONTWAIT);
        }
    }

    fn receive_control(&self) -> io::Result<Result<(), Error>> {
        // No await or blocking receive occurs while this lock is held. The FD
        // cannot sit between receipt and installation when cancel returns.
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if self.cancelled.load(Ordering::Acquire) || control.is_some() {
            return Ok(Err(Error::Custody));
        }
        let (prefix, descriptor) = super::ancillary::receive(&self.payload, true)?;
        match super::ancillary::original_control(prefix, descriptor) {
            Ok(original) => {
                *control = Some(Arc::new(original));
                Ok(Ok(()))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    pub(super) fn control(&self) -> io::Result<Arc<Control>> {
        self.control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(super::ancillary::invalid)
    }

    #[cfg(test)]
    fn install(&self, original: Control) -> Result<(), Error> {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if self.cancelled.load(Ordering::Acquire) || control.is_some() {
            original.cancel();
            return Err(Error::Custody);
        }
        *control = Some(Arc::new(original));
        Ok(())
    }
}

/// One original accepted socket. No raw descriptor, constructor or clone is exposed.
/// Dropping its consuming owner closes the sole payload descriptor and queued rights.
pub struct PendingStream {
    inner: Option<Arc<Inner>>,
}
impl PendingStream {
    pub(super) fn new(payload: UnixStream, origin: Arc<Origin>) -> Self {
        Self {
            inner: Some(Arc::new(Inner {
                payload,
                origin,
                control: Mutex::new(None),
                cancelled: AtomicBool::new(false),
            })),
        }
    }

    fn inner(&self) -> Result<&Arc<Inner>, Error> {
        let inner = self.inner.as_ref().ok_or(Error::Custody)?;
        if inner.cancelled.load(Ordering::Acquire) {
            return Err(Error::Custody);
        }
        Ok(inner)
    }

    pub(super) fn payload(&self) -> Result<&UnixStream, Error> {
        Ok(&self.inner()?.payload)
    }

    pub(super) fn has_origin(&self, origin: &Arc<Origin>) -> bool {
        self.inner().is_ok_and(|inner| Arc::ptr_eq(&inner.origin, origin))
    }

    #[cfg(test)]
    pub(super) fn install_control(&self, control: Control) -> Result<(), Error> {
        // Installation and cancellation share the same lock, so a late descriptor
        // cannot become live after the original pending owner was cancelled.
        self.inner()?.install(control)
    }

    pub(super) async fn receive_control(&self, until: Instant) -> Result<(), Error> {
        let original = self.inner()?;
        timeout_at(until, async {
            loop {
                original.payload.readable().await?;
                match original
                    .payload
                    .try_io(Interest::READABLE, || original.receive_control())
                {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => tokio::task::yield_now().await,
                    result => return result.map_err(Error::from)?,
                }
            }
        })
        .await
        .map_err(|_| Error::Deadline)?
    }

    pub(super) fn control(&self) -> Result<Arc<Control>, Error> {
        Ok(self.inner()?.control()?)
    }

    pub(super) fn into_inner(mut self) -> Result<Arc<Inner>, Error> {
        self.inner()?;
        self.inner.take().ok_or(Error::Custody)
    }

    /// Keep cancellation and comparison, without retaining any payload/control owner.
    #[must_use]
    pub fn cancellation(&self) -> PendingCancellation {
        PendingCancellation {
            original: self.inner.as_ref().map_or_else(Weak::new, Arc::downgrade),
        }
    }

    /// Apply the consuming Gate's requested buffer limit without exposing its descriptor.
    ///
    /// # Errors
    /// Refuses cancellation, socket errors or observed buffers above the supplied maximum.
    #[cfg(feature = "exposure-gate")]
    pub fn bound_buffers(&self, bytes: usize, maximum: usize) -> io::Result<()> {
        use rustix::net::sockopt;
        let stream = self.payload().map_err(io::Error::other)?;
        sockopt::set_socket_recv_buffer_size(stream, bytes)?;
        sockopt::set_socket_send_buffer_size(stream, bytes)?;
        if sockopt::socket_recv_buffer_size(stream)? > maximum || sockopt::socket_send_buffer_size(stream)? > maximum {
            return Err(io::Error::other("Pending socket buffer bound is unavailable"));
        }
        Ok(())
    }
}
impl Drop for PendingStream {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.cancel();
        }
    }
}

/// A weak handle to the same original connection across Pending and Accepted phases.
/// Cloning this handle never retains payload descriptors or queued `SCM_RIGHTS`.
#[derive(Clone)]
pub struct PendingCancellation {
    original: Weak<Inner>,
}
impl PendingCancellation {
    /// Physically cancel the original payload and any installed control endpoint.
    pub fn cancel(&self) {
        if let Some(original) = self.original.upgrade() {
            original.cancel();
        }
    }

    pub(super) fn matches(&self, original: &Arc<Inner>) -> bool {
        Weak::ptr_eq(&self.original, &Arc::downgrade(original))
    }
}

// Private transport mechanics only: this origin cannot match a real Backend.
#[cfg(test)]
pub(crate) fn fixture(payload: UnixStream) -> PendingStream {
    PendingStream::new(payload, Arc::new(Origin))
}

#[cfg(test)]
mod tests;
