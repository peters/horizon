use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use super::Cancellation;

struct CancelWake {
    waiting: Pin<Box<dyn Future<Output = ()> + Send>>,
    cancelled: bool,
}

impl CancelWake {
    fn new(control: &Cancellation) -> Self {
        let mut changed = control.subscribe();
        Self {
            waiting: Box::pin(async move {
                let _ = changed.wait_for(|cancelled| *cancelled).await;
            }),
            cancelled: false,
        }
    }

    fn check(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        self.cancelled = self.cancelled || self.waiting.as_mut().poll(cx).is_ready();
        if self.cancelled {
            Err(io::Error::from(io::ErrorKind::ConnectionAborted))
        } else {
            Ok(())
        }
    }
}

/// OS shutdown alone need not wake a pending reactor read on Windows. Register
/// cancellation with each I/O direction, independently of socket readiness.
pub(crate) struct Transport {
    socket: TcpStream,
    read_cancel: CancelWake,
    write_cancel: CancelWake,
}

impl Transport {
    pub(crate) fn new(socket: TcpStream, control: &Cancellation) -> Self {
        Self {
            socket,
            read_cancel: CancelWake::new(control),
            write_cancel: CancelWake::new(control),
        }
    }
}

impl AsyncRead for Transport {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.read_cancel.check(cx)?;
        Pin::new(&mut this.socket).poll_read(cx, buf)
    }
}

impl AsyncWrite for Transport {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.write_cancel.check(cx)?;
        Pin::new(&mut this.socket).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.write_cancel.check(cx)?;
        Pin::new(&mut this.socket).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.write_cancel.check(cx)?;
        Pin::new(&mut this.socket).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests;
