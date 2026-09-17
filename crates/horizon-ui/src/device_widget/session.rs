use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use egui::{ColorImage, Context, ViewportId};
use tokio::sync::oneshot;
use vnc::{PixelFormat, VncConnector, VncEncoding, X11Event};

use super::frame::Framebuffer;

#[derive(Debug, thiserror::Error)]
pub(super) enum ViewError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Protocol(#[from] vnc::VncError),
    #[error("{0}")]
    Frame(&'static str),
    #[error("{0}")]
    Server(String),
    #[error("connection timed out")]
    Timeout,
}

#[derive(Default)]
pub(super) enum Status {
    #[default]
    Stopped,
    Connecting,
    Connected,
    Disconnected(String),
}

pub(super) struct Updates {
    pub image: Option<ColorImage>,
    pub status: Option<Status>,
    viewport: ViewportId,
}

pub(super) struct Session {
    updates: Arc<Mutex<Updates>>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub(super) fn start(address: SocketAddr, ctx: Context, viewport: ViewportId) -> Result<Self, ViewError> {
        let updates = Arc::new(Mutex::new(Updates {
            image: None,
            status: Some(Status::Connecting),
            viewport,
        }));
        let state = Arc::clone(&updates);
        let (stop, cancelled) = oneshot::channel();
        let thread = std::thread::Builder::new().name("device-view".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map(|runtime| {
                    runtime.block_on(async {
                        // Dropping the connection future and its runtime cancels every
                        // decoder/socket task, including a stalled handshake or read.
                        tokio::select! {
                            _ = cancelled => Ok(()),
                            result = connection(address, &state, &ctx) => result,
                        }
                    })
                });
            let status = match result {
                Ok(Ok(())) => Status::Stopped,
                Ok(Err(error)) => Status::Disconnected(error.to_string()),
                Err(error) => Status::Disconnected(error.to_string()),
            };
            publish_status(&state, &ctx, status);
        })?;
        Ok(Self {
            updates,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub(super) fn take_updates(&self, viewport: ViewportId) -> Updates {
        let mut state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.viewport = viewport;
        Updates {
            image: state.image.take(),
            status: state.status.take(),
            viewport,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("device viewer worker panicked during shutdown");
        }
    }
}

fn publish_status(updates: &Mutex<Updates>, ctx: &Context, status: Status) {
    let viewport = {
        let mut state = updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.status = Some(status);
        state.viewport
    };
    ctx.request_repaint_of(viewport);
}

async fn connection(address: SocketAddr, updates: &Mutex<Updates>, ctx: &Context) -> Result<(), ViewError> {
    let client = tokio::time::timeout(Duration::from_secs(5), async {
        let stream = tokio::net::TcpStream::connect(address).await?;
        stream.set_nodelay(true)?;
        // The local MVP has no credentials. Password-required servers fail
        // explicitly; input and clipboard are never forwarded by this viewer.
        let client = VncConnector::new(stream)
            .set_auth_method(async { Err(vnc::VncError::NoPassword) })
            .add_encoding(VncEncoding::Zrle)
            .add_encoding(VncEncoding::CopyRect)
            .add_encoding(VncEncoding::Raw)
            .add_encoding(VncEncoding::DesktopSizePseudo)
            .allow_shared(true)
            .set_pixel_format(PixelFormat::rgba())
            .build()?
            .try_start()
            .await?
            .finish()?;
        Ok::<_, ViewError>(client)
    })
    .await
    .map_err(|_| ViewError::Timeout)??;
    publish_status(updates, ctx, Status::Connected);
    let mut framebuffer = Framebuffer::default();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let mut changed = false;
        let started = std::time::Instant::now();
        let mut idle = false;
        for _ in 0..4096 {
            if started.elapsed() >= Duration::from_millis(10) {
                break;
            }
            if let Some(event) = client.poll_event().await? {
                changed |= framebuffer.apply(event)?;
                idle = false;
            } else if idle {
                break;
            } else {
                // Let the decoder refill its bounded queue before publishing a
                // partial frame; do not throttle every desktop to a few tiles.
                tokio::task::yield_now().await;
                idle = true;
            }
        }
        if changed {
            let image = framebuffer.image();
            let viewport = {
                let mut state = updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // Only the latest frame is retained; slow rendering cannot grow
                // an application-side queue of full desktop images.
                state.image = Some(image);
                state.viewport
            };
            ctx.request_repaint_of(viewport);
        }
        tokio::time::timeout(Duration::from_secs(5), client.input(X11Event::Refresh))
            .await
            .map_err(|_| ViewError::Timeout)??;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn close_cancels_a_server_that_never_sends_its_greeting() -> Result<(), ViewError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let session = Session::start(listener.local_addr()?, Context::default(), ViewportId::ROOT)?;
        let accepted = std::time::Instant::now();
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(accepted.elapsed() < Duration::from_secs(2), "viewer did not connect");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let start = std::time::Instant::now();
        drop(session);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(stream.read(&mut [0; 1])?, 0);
        Ok(())
    }

    #[test]
    fn close_cancels_a_connected_server_with_an_incomplete_frame() -> Result<(), ViewError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let session = Session::start(listener.local_addr()?, Context::default(), ViewportId::ROOT)?;
        let started = std::time::Instant::now();
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(started.elapsed() < Duration::from_secs(2), "viewer did not connect");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(b"RFB 003.008\n")?;
        let mut version = [0; 12];
        stream.read_exact(&mut version)?;
        assert_eq!(&version, b"RFB 003.008\n");
        stream.write_all(&[1, 1])?;
        let mut byte = [0; 1];
        stream.read_exact(&mut byte)?;
        assert_eq!(byte, [1], "no-auth security selected");
        stream.write_all(&[0; 4])?;
        stream.read_exact(&mut byte)?;
        assert_eq!(byte, [1], "shared connection requested");
        // A 2x2 true-color desktop, with no server name.
        stream.write_all(&[
            0, 2, 0, 2, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0, 0, 0, 0, 0,
        ])?;
        let mut pixel_format = [0; 20];
        stream.read_exact(&mut pixel_format)?;
        assert_eq!(pixel_format[0], 0);
        let mut encodings_header = [0; 4];
        stream.read_exact(&mut encodings_header)?;
        assert_eq!(encodings_header[0], 2);
        let count = usize::from(u16::from_be_bytes([encodings_header[2], encodings_header[3]]));
        assert_eq!(count, 4);
        stream.read_exact(&mut [0; 16])?;
        let mut refresh = [0; 10];
        stream.read_exact(&mut refresh)?;
        assert_eq!(refresh[0], 3, "handshake completed before cancellation");
        // Begin one Raw rectangle but leave its pixel payload unfinished.
        stream.write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0, 1, 2, 3, 0])?;
        assert!(matches!(
            session.take_updates(ViewportId::ROOT).status,
            Some(Status::Connected)
        ));
        let closed = std::time::Instant::now();
        drop(session);
        assert!(closed.elapsed() < Duration::from_secs(1));
        // Drain any queued refresh requests and prove the connection was closed.
        let mut remaining = Vec::new();
        match stream.read_to_end(&mut remaining) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => return Err(error.into()),
        }
        assert!(remaining.as_chunks::<10>().0.iter().all(|request| request[0] == 3));
        Ok(())
    }
}
