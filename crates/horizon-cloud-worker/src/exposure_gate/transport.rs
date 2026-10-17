//! The same bounded relay operations for direct TCP and original Producer Unix sockets.
#[cfg(feature = "trusted-exposure-gate")]
use crate::trusted_tcp_producer::AcceptedStream;
use socket2::SockRef;
use std::{io, net::Shutdown};
use tokio::net::TcpStream;

pub(super) trait Transport: Send + Sync {
    fn readable(&self) -> impl Future<Output = io::Result<()>> + Send;
    fn writable(&self) -> impl Future<Output = io::Result<()>> + Send;
    fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize>;
    fn try_write(&self, bytes: &[u8]) -> io::Result<usize>;
    fn shutdown_socket(&self, direction: Shutdown) -> io::Result<()>;
    fn prepare(&self);
    fn finish(&self) -> impl Future<Output = io::Result<()>> + Send;
    fn failed(&self) -> impl Future<Output = ()> + Send;
}

impl Transport for TcpStream {
    fn readable(&self) -> impl Future<Output = io::Result<()>> + Send {
        TcpStream::readable(self)
    }
    fn writable(&self) -> impl Future<Output = io::Result<()>> + Send {
        TcpStream::writable(self)
    }
    fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        TcpStream::try_read(self, bytes)
    }
    fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        TcpStream::try_write(self, bytes)
    }
    fn shutdown_socket(&self, direction: Shutdown) -> io::Result<()> {
        SockRef::from(self).shutdown(direction)
    }
    fn prepare(&self) {
        let _ = self.set_nodelay(true);
    }
    fn finish(&self) -> impl Future<Output = io::Result<()>> + Send {
        std::future::ready(Ok(()))
    }
    fn failed(&self) -> impl Future<Output = ()> + Send {
        std::future::pending()
    }
}

#[cfg(feature = "trusted-exposure-gate")]
impl Transport for AcceptedStream {
    fn readable(&self) -> impl Future<Output = io::Result<()>> + Send {
        AcceptedStream::readable(self)
    }
    fn writable(&self) -> impl Future<Output = io::Result<()>> + Send {
        AcceptedStream::writable(self)
    }
    fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        AcceptedStream::try_read(self, bytes)
    }
    fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        AcceptedStream::try_write(self, bytes)
    }
    fn shutdown_socket(&self, direction: Shutdown) -> io::Result<()> {
        match direction {
            Shutdown::Write => self.shutdown_write(),
            Shutdown::Both => {
                self.cancel();
                Ok(())
            }
            Shutdown::Read => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Read-only Producer shutdown is unsupported",
            )),
        }
    }
    fn prepare(&self) {}
    fn finish(&self) -> impl Future<Output = io::Result<()>> + Send {
        AcceptedStream::finish(self)
    }
    async fn failed(&self) {
        if AcceptedStream::finish(self).await.is_ok() {
            std::future::pending::<()>().await;
        }
    }
}
