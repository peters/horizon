mod tunnel;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use egui::{ColorImage, Context, ViewportId};
use horizon_core::{
    DevicePanelState, DeviceViewOptions, SshConnection, browser::manifest::device::DeviceServerDetails,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use vnc::{PixelFormat, VncConnector, VncEncoding, X11Event};

use self::tunnel::SshTunnel;
use super::frame::{Framebuffer, present_image};

const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Key exchange over a mesh network plus the VNC handshake behind it.
const TUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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
    #[error("{0}")]
    Tunnel(String),
    #[error("connection timed out")]
    Timeout,
}

/// How the worker reaches the VNC server; owned so the worker thread can keep it.
#[derive(Clone, Debug)]
pub(super) enum DeviceRoute {
    Direct(SocketAddr),
    /// `remote` is the endpoint as seen from the SSH host.
    SshTunnel {
        connection: SshConnection,
        remote: SocketAddr,
    },
}

impl From<&DevicePanelState> for DeviceRoute {
    fn from(device: &DevicePanelState) -> Self {
        match &device.ssh_tunnel {
            Some(connection) => Self::SshTunnel {
                connection: connection.clone(),
                remote: device.target.address(),
            },
            None => Self::Direct(device.target.address()),
        }
    }
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
    pub stream: StreamEvidence,
    pub image: Option<ColorImage>,
    pub produced_with: Option<DeviceViewOptions>,
    pub status: Option<Status>,
    pub received_frame_sequence: u64,
    viewport: ViewportId,
    visible: bool,
    options: DeviceViewOptions,
    pub desktop: Option<[usize; 2]>,
    pub server_name: Option<String>,
}

#[derive(Clone, Copy, Default)]
pub(super) struct StreamEvidence {
    pub sequence: u64,
    pub last_frame: Option<Instant>,
}

pub(super) struct Observation {
    pub status: Option<Status>,
    pub received_frame_sequence: u64,
}

pub(super) struct Session {
    updates: Arc<Mutex<Updates>>,
    latest_full: Arc<Mutex<Option<ColorImage>>>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub(super) fn start(
        route: DeviceRoute,
        ctx: Context,
        viewport: ViewportId,
        options: DeviceViewOptions,
    ) -> Result<Self, ViewError> {
        let updates = Arc::new(Mutex::new(Updates {
            stream: StreamEvidence::default(),
            image: None,
            produced_with: None,
            status: Some(Status::Connecting),
            received_frame_sequence: 0,
            viewport,
            visible: true,
            options,
            desktop: None,
            server_name: None,
        }));
        let latest_full = Arc::new(Mutex::new(None));
        let state = Arc::clone(&updates);
        let retained = Arc::clone(&latest_full);
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
                            result = connection(route, &state, &retained, &ctx) => result,
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
            latest_full,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub(super) fn set_options(&self, options: DeviceViewOptions) {
        self.updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .options = options;
    }

    pub(super) fn latest_full(&self) -> Option<ColorImage> {
        self.latest_full
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    pub(super) fn pending_frame(latest_full: ColorImage, image: ColorImage, produced_with: DeviceViewOptions) -> Self {
        Self {
            updates: Arc::new(Mutex::new(Updates {
                stream: StreamEvidence {
                    sequence: 1,
                    last_frame: Some(Instant::now()),
                },
                image: Some(image),
                produced_with: Some(produced_with),
                status: Some(Status::Connected),
                received_frame_sequence: 1,
                viewport: ViewportId::ROOT,
                visible: true,
                options: produced_with,
                desktop: Some(latest_full.size),
                server_name: None,
            })),
            latest_full: Arc::new(Mutex::new(Some(latest_full))),
            stop: None,
            thread: None,
        }
    }

    pub(super) fn set_visible(&self, visible: bool) {
        self.updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .visible = visible;
    }

    pub(super) fn take_updates(&self, viewport: ViewportId) -> Updates {
        let mut state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.viewport = viewport;
        Updates {
            stream: state.stream,
            image: state.image.take(),
            produced_with: state.produced_with,
            status: state.status.take(),
            received_frame_sequence: state.received_frame_sequence,
            viewport,
            visible: state.visible,
            options: state.options,
            desktop: state.desktop,
            server_name: state.server_name.take(),
        }
    }

    pub(super) fn take_server_details(&self) -> DeviceServerDetails {
        let mut state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        DeviceServerDetails {
            name: state.server_name.take(),
            desktop_size: state.desktop,
        }
    }

    pub(super) fn observation(&self) -> Observation {
        let mut state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Observation {
            status: state.status.take(),
            received_frame_sequence: state.received_frame_sequence,
        }
    }

    pub(super) fn stream_evidence(&self) -> (StreamEvidence, bool) {
        let state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.stream, false)
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
        state.visible.then_some(state.viewport)
    };
    if let Some(viewport) = viewport {
        ctx.request_repaint_of(viewport);
    }
}

async fn connection(
    route: DeviceRoute,
    updates: &Mutex<Updates>,
    latest_full: &Mutex<Option<ColorImage>>,
    ctx: &Context,
) -> Result<(), ViewError> {
    // The tunnel process lives exactly as long as this connection.
    let (client, _tunnel) = connect_client(route).await?;
    let mut framebuffer = Framebuffer::default();
    let mut received_pixels = false;
    // ServerInit queues resolution before the client is returned. Observe that
    // metadata before the first image refresh, independently of presentation.
    if let Some(event) = client.poll_event().await? {
        apply_frame_event(&mut framebuffer, &mut received_pixels, event)?;
    }
    {
        let mut state = updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.server_name = DevicePanelState::server_label(client.server_name());
        state.desktop = (!framebuffer.size().contains(&0)).then(|| framebuffer.size());
    }
    publish_status(updates, ctx, Status::Connected);
    loop {
        let mut full_refresh = false;
        let options = updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .options;
        tokio::time::sleep(options.interval()).await;
        let mut changed = false;
        let started = std::time::Instant::now();
        let mut idle = false;
        for _ in 0..4096 {
            if started.elapsed() >= Duration::from_millis(10) {
                break;
            }
            if let Some(event) = client.poll_event().await? {
                full_refresh |= matches!(event, vnc::VncEvent::SetResolution(_));
                let previous_size = framebuffer.size();
                changed |= apply_frame_event(&mut framebuffer, &mut received_pixels, event)?;
                if framebuffer.size() != previous_size {
                    updates
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .desktop = Some(framebuffer.size());
                }
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
        // Sample off the UI thread. The full desktop is retained so view
        // controls can re-present after disconnect or an options change.
        if received_pixels && changed && !framebuffer.size().contains(&0) {
            let full = framebuffer.full_image()?;
            let image = present_image(&full, options)?;
            *latest_full.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(full);
            let viewport = {
                let mut state = updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // Only the latest frame is retained; slow rendering cannot grow
                // an application-side queue of full desktop images.
                state.image = Some(image);
                state.stream.sequence = state.stream.sequence.saturating_add(1);
                state.stream.last_frame = Some(Instant::now());
                state.produced_with = Some(options.for_desktop(framebuffer.size()));
                state.desktop = Some(framebuffer.size());
                state.received_frame_sequence = state.received_frame_sequence.saturating_add(1);
                state.visible.then_some(state.viewport)
            };
            if let Some(viewport) = viewport {
                ctx.request_repaint_of(viewport);
            }
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            client.input(if full_refresh {
                X11Event::FullRefresh
            } else {
                X11Event::Refresh
            }),
        )
        .await
        .map_err(|_| ViewError::Timeout)??;
    }
}

async fn connect_client(route: DeviceRoute) -> Result<(vnc::VncClient, Option<SshTunnel>), ViewError> {
    match route {
        DeviceRoute::Direct(address) => {
            let client = tokio::time::timeout(DIRECT_CONNECT_TIMEOUT, async {
                let stream = tokio::net::TcpStream::connect(address).await?;
                stream.set_nodelay(true)?;
                handshake(stream).await
            })
            .await
            .map_err(|_| ViewError::Timeout)??;
            Ok((client, None))
        }
        DeviceRoute::SshTunnel { connection, remote } => {
            let mut tunnel = SshTunnel::spawn("ssh", &connection.stdio_forward_args(&remote.to_string()))?;
            let stream = tunnel.take_stream()?;
            match tokio::time::timeout(TUNNEL_CONNECT_TIMEOUT, handshake(stream)).await {
                Ok(Ok(client)) => Ok((client, Some(tunnel))),
                Ok(Err(error)) => Err(tunnel.explain(error).await),
                Err(_) => Err(tunnel.explain(ViewError::Timeout).await),
            }
        }
    }
}

async fn handshake<S>(stream: S) -> Result<vnc::VncClient, ViewError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    // The local MVP has no credentials. Password-required servers fail
    // explicitly; input and clipboard are never forwarded by this viewer.
    let client = VncConnector::new(stream)
        .set_auth_method(async { Err(vnc::VncError::NoPassword) })
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::CopyRect)
        .add_encoding(VncEncoding::Raw)
        .add_encoding(VncEncoding::DesktopSizePseudo)
        .add_encoding(VncEncoding::ExtendedDesktopSizePseudo)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::rgba())
        .build()?
        .try_start()
        .await?
        .finish()?;
    Ok(client)
}

fn apply_frame_event(
    framebuffer: &mut Framebuffer,
    received_pixels: &mut bool,
    event: vnc::VncEvent,
) -> Result<bool, ViewError> {
    let reset = matches!(event, vnc::VncEvent::SetResolution(_));
    let pixels = matches!(event, vnc::VncEvent::RawImage(_, _));
    let previous_size = framebuffer.size();
    let changed = framebuffer.apply(event)?;
    if reset || framebuffer.size() != previous_size {
        *received_pixels = false;
    }
    *received_pixels |= pixels;
    Ok(changed)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod image_evidence_tests {
    use super::*;

    #[test]
    fn extended_resize_waits_for_pixels_but_same_size_announcement_preserves_them() -> Result<(), ViewError> {
        let mut framebuffer = Framebuffer::default();
        let mut received_pixels = false;
        for width in [2, 2, 4] {
            let previous_size = framebuffer.size();
            let event = vnc::VncEvent::DesktopUpdate(vnc::DesktopUpdate {
                reason: vnc::DesktopReason::Server,
                status: vnc::DesktopStatus::Success,
                layout: Some(vnc::DesktopLayout {
                    width,
                    height: 1,
                    screens: Vec::new(),
                }),
            });
            let changed = apply_frame_event(&mut framebuffer, &mut received_pixels, event)?;
            let resized = previous_size != framebuffer.size();
            assert_eq!(changed, resized);
            assert_eq!(
                received_pixels, !resized,
                "a resized blank buffer is not received pixels"
            );
            apply_frame_event(
                &mut framebuffer,
                &mut received_pixels,
                vnc::VncEvent::RawImage(
                    vnc::Rect {
                        x: 0,
                        y: 0,
                        width,
                        height: 1,
                    },
                    vec![255; usize::from(width) * 4],
                ),
            )?;
            assert!(received_pixels, "new pixels resume image publication");
        }
        Ok(())
    }
}
