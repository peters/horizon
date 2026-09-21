use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use egui::{ColorImage, Context, ViewportId};
use horizon_core::{DevicePanelState, DeviceViewOptions, browser::manifest::device::DeviceServerDetails};
use tokio::sync::{oneshot, watch};
use vnc::{PixelFormat, VncConnector, VncEncoding, X11Event};

use super::frame::{Framebuffer, present_image};

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
    pub produced_with: Option<DeviceViewOptions>,
    pub status: Option<Status>,
    viewport: ViewportId,
    visible: bool,
    options: DeviceViewOptions,
    pub desktop: Option<[usize; 2]>,
    pub server_name: Option<String>,
}

pub(super) struct Session {
    updates: Arc<Mutex<Updates>>,
    latest_full: Arc<Mutex<Option<ColorImage>>>,
    stop: Option<oneshot::Sender<()>>,
    visibility: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub(super) fn start(
        address: SocketAddr,
        ctx: Context,
        viewport: ViewportId,
        options: DeviceViewOptions,
    ) -> Result<Self, ViewError> {
        let updates = Arc::new(Mutex::new(Updates {
            image: None,
            produced_with: None,
            status: Some(Status::Connecting),
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
        let (visibility, visible) = watch::channel(true);
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
                            result = connection(address, &state, &retained, &ctx, visible) => result,
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
            visibility,
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
        let (visibility, _) = watch::channel(true);
        Self {
            updates: Arc::new(Mutex::new(Updates {
                image: Some(image),
                produced_with: Some(produced_with),
                status: Some(Status::Connected),
                viewport: ViewportId::ROOT,
                visible: true,
                options: produced_with,
                desktop: Some(latest_full.size),
                server_name: None,
            })),
            latest_full: Arc::new(Mutex::new(Some(latest_full))),
            stop: None,
            visibility,
            thread: None,
        }
    }

    pub(super) fn set_visible(&self, visible: bool) {
        self.updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .visible = visible;
        self.visibility.send_if_modified(|current| {
            if *current == visible {
                false
            } else {
                *current = visible;
                true
            }
        });
    }

    pub(super) fn take_updates(&self, viewport: ViewportId) -> Updates {
        let mut state = self.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.viewport = viewport;
        Updates {
            image: state.image.take(),
            produced_with: state.produced_with,
            status: state.status.take(),
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

    pub(super) fn take_status(&self) -> Option<Status> {
        self.updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
            .take()
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
    address: SocketAddr,
    updates: &Mutex<Updates>,
    latest_full: &Mutex<Option<ColorImage>>,
    ctx: &Context,
    mut visible: watch::Receiver<bool>,
) -> Result<(), ViewError> {
    let client = connect_client(address).await?;
    let mut framebuffer = Framebuffer::default();
    let mut received_pixels = false;
    // ServerInit queues resolution before the client is returned. Observe that
    // metadata even if a hidden viewer pauses before its first image refresh.
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
        let mut full_refresh = !*visible.borrow_and_update();
        if full_refresh {
            visible
                .wait_for(|visible| *visible)
                .await
                .map_err(|_| ViewError::Frame("viewer closed"))?;
        }
        let options = updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .options;
        tokio::time::sleep(options.interval()).await;
        if !*visible.borrow() {
            continue;
        }
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
                state.produced_with = Some(options.for_desktop(framebuffer.size()));
                state.desktop = Some(framebuffer.size());
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

async fn connect_client(address: SocketAddr) -> Result<vnc::VncClient, ViewError> {
    tokio::time::timeout(Duration::from_secs(5), async {
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
            .add_encoding(VncEncoding::ExtendedDesktopSizePseudo)
            .allow_shared(true)
            .set_pixel_format(PixelFormat::rgba())
            .build()?
            .try_start()
            .await?
            .finish()?;
        Ok::<_, ViewError>(client)
    })
    .await
    .map_err(|_| ViewError::Timeout)?
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
