//! The session thread: connect, decode, publish the latest image and forward input.
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use vnc::{PixelFormat, VncConnector, VncEncoding, X11Event};

use super::{Image, Route, Shared, Status, host_key::TunnelHost, tunnel::SshTunnel};
use crate::{Error, Frame, InputEvent, Region, ViewOptions, framebuffer::Framebuffer};

const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Key exchange over a mesh network plus the VNC handshake behind it.
const TUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn connection(
    route: Route,
    shared: &Shared,
    latest_full: &Mutex<Option<Arc<Frame>>>,
    input: &mut mpsc::UnboundedReceiver<InputEvent>,
) -> Result<(), Error> {
    // The tunnel process lives exactly as long as this connection.
    let host = route.tunnel_host();
    let (client, mut tunnel) = connect_client(route).await?;
    let result = stream_desktop(client, shared, latest_full, input).await;
    match (result, tunnel.as_mut()) {
        // A forward that dies mid-session ends the stream; ssh says why.
        (Err(error), Some(tunnel)) => Err(tunnel.explain(error, host.as_ref()).await),
        (result, _) => result,
    }
}

async fn stream_desktop(
    client: vnc::VncClient,
    shared: &Shared,
    latest_full: &Mutex<Option<Arc<Frame>>>,
    input: &mut mpsc::UnboundedReceiver<InputEvent>,
) -> Result<(), Error> {
    let mut framebuffer = Framebuffer::default();
    let mut received_pixels = false;
    let mut published = None;
    // ServerInit queues resolution before the client is returned. Observe that
    // metadata before the first image refresh, independently of presentation.
    if let Some(event) = client.poll_event().await? {
        apply_frame_event(&mut framebuffer, &mut received_pixels, event)?;
    }
    {
        let mut state = shared.lock();
        let name = client.server_name();
        state.server_name = (!name.is_empty()).then(|| name.to_owned());
        state.desktop = (!framebuffer.size().contains(&0)).then(|| framebuffer.size());
    }
    shared.publish_status(Status::Connected);
    loop {
        let mut full_refresh = false;
        let options = shared.lock().options;
        forward_input_until(&client, input, options.interval()).await?;
        let mut changed = false;
        let started = Instant::now();
        let mut idle = false;
        for _ in 0..4096 {
            if started.elapsed() >= Duration::from_millis(10) {
                break;
            }
            if let Some(event) = client.poll_event().await? {
                if let vnc::VncEvent::Text(text) = &event {
                    shared.publish_text(text);
                    continue;
                }
                full_refresh |= matches!(event, vnc::VncEvent::SetResolution(_));
                let previous_size = framebuffer.size();
                changed |= apply_frame_event(&mut framebuffer, &mut received_pixels, event)?;
                if framebuffer.size() != previous_size {
                    shared.lock().desktop = Some(framebuffer.size());
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
        if received_pixels && changed && !framebuffer.size().contains(&0) {
            publish_image(&mut framebuffer, options, &mut published, shared, latest_full)?;
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
        .map_err(|_| Error::Timeout)??;
    }
}

/// Sample off the UI thread. The full desktop is retained so view controls can
/// re-present after disconnect or an options change.
fn publish_image(
    framebuffer: &mut Framebuffer,
    options: ViewOptions,
    published: &mut Option<ViewOptions>,
    shared: &Shared,
    latest_full: &Mutex<Option<Arc<Frame>>>,
) -> Result<(), Error> {
    let full = framebuffer.frame()?;
    let frame = full.present(options)?;
    let desktop = full.size();
    let produced_with = options.for_desktop(desktop);
    let changed = changed_region(
        framebuffer.take_damage(),
        *published,
        produced_with,
        desktop,
        frame.size(),
    );
    *published = Some(produced_with);
    *latest_full.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(full));
    let visible = {
        let mut state = shared.lock();
        // Only the latest frame is retained; slow rendering cannot grow an
        // application-side queue of full desktop images.
        let changed = untaken_changes(state.image.as_ref(), changed, frame.size());
        state.image = Some(Image {
            frame,
            produced_with,
            changed,
        });
        state.stream.sequence = state.stream.sequence.saturating_add(1);
        state.stream.last_frame = Some(Instant::now());
        state.desktop = Some(desktop);
        state.received_frame_sequence = state.received_frame_sequence.saturating_add(1);
        state.visible
    };
    shared.wake(visible);
    Ok(())
}

/// The pixels a UI that holds the previously published image must replace:
/// the written region while the image is the whole desktop at its own size
/// and the presentation did not change, otherwise the whole image.
fn changed_region(
    damage: Option<Region>,
    previous: Option<ViewOptions>,
    produced_with: ViewOptions,
    desktop: [usize; 2],
    image: [usize; 2],
) -> Region {
    let same_view = previous.is_some_and(|previous| previous.same_presentation(produced_with));
    let uncropped = produced_with
        .layout(desktop)
        .is_ok_and(|layout| layout.shows_all_of(desktop));
    match damage {
        Some(damage) if same_view && uncropped => damage,
        _ => Region::whole(image),
    }
}

/// An image that the UI did not take is replaced, so the new image must also
/// cover what changed in it.
fn untaken_changes(pending: Option<&Image>, changed: Region, image: [usize; 2]) -> Region {
    match pending {
        Some(pending) if pending.frame.size() == image => pending.changed.union(changed),
        Some(_) => Region::whole(image),
        None => changed,
    }
}

const fn x11_event(event: InputEvent) -> X11Event {
    match event {
        InputEvent::Key { keysym, down } => X11Event::KeyEvent(vnc::ClientKeyEvent { keycode: keysym, down }),
        InputEvent::Pointer { x, y, buttons } => X11Event::PointerEvent(vnc::ClientMouseEvent {
            position_x: x,
            position_y: y,
            bottons: buttons,
        }),
    }
}

/// Wait out the refresh cadence, sending any queued input as it arrives so a
/// click or keystroke never waits for the next frame.
async fn forward_input_until(
    client: &vnc::VncClient,
    input: &mut mpsc::UnboundedReceiver<InputEvent>,
    interval: Duration,
) -> Result<(), Error> {
    let deadline = Instant::now() + interval;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        match tokio::time::timeout(remaining, input.recv()).await {
            Ok(Some(event)) => {
                tokio::time::timeout(Duration::from_secs(5), client.input(x11_event(event)))
                    .await
                    .map_err(|_| Error::Timeout)??;
            }
            // The sender lives in the Session, which cancels this worker when it
            // drops; until then a closed channel just means no more input.
            Ok(None) => {
                tokio::time::sleep(remaining).await;
                return Ok(());
            }
            Err(_) => return Ok(()),
        }
    }
}

async fn connect_client(route: Route) -> Result<(vnc::VncClient, Option<SshTunnel>), Error> {
    match route {
        Route::Direct(address) => {
            let client = tokio::time::timeout(DIRECT_CONNECT_TIMEOUT, async {
                let stream = tokio::net::TcpStream::connect(address).await?;
                stream.set_nodelay(true)?;
                handshake(stream).await
            })
            .await
            .map_err(|_| Error::Timeout)??;
            Ok((client, None))
        }
        Route::Tunnel {
            program,
            args,
            host,
            port,
        } => {
            // A failed connect may be ssh refusing the host key, which only a
            // person can resolve.
            let host = TunnelHost {
                program: program.clone(),
                host,
                port,
            };
            let mut tunnel = SshTunnel::spawn(&program, &args)?;
            let stream = tunnel.take_stream()?;
            match tokio::time::timeout(TUNNEL_CONNECT_TIMEOUT, handshake(stream)).await {
                Ok(Ok(client)) => Ok((client, Some(tunnel))),
                Ok(Err(error)) => Err(tunnel.explain(error, Some(&host)).await),
                Err(_) => Err(tunnel.explain(Error::Timeout, Some(&host)).await),
            }
        }
    }
}

async fn handshake<S>(stream: S) -> Result<vnc::VncClient, Error>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    // No credentials are sent, so a server that requires a password fails
    // explicitly. The local clipboard is never sent.
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
) -> Result<bool, Error> {
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
mod tests {
    use super::*;

    #[test]
    fn extended_resize_waits_for_pixels_but_same_size_announcement_preserves_them() -> Result<(), Error> {
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

    #[test]
    fn only_an_unchanged_whole_desktop_view_reports_the_written_region() {
        let written = Region {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        };
        let whole = ViewOptions::default();
        let crop = ViewOptions {
            viewport: Some(Region::whole([2, 1])),
            ..whole
        };
        let small = ViewOptions { max_width: 1, ..whole };
        assert_eq!(
            changed_region(Some(written), Some(whole), whole, [4, 2], [4, 2]),
            written
        );
        assert_eq!(
            changed_region(Some(written), Some(whole), whole, [4, 2], [4, 2]),
            changed_region(
                Some(written),
                Some(ViewOptions { max_fps: 1, ..whole }),
                whole,
                [4, 2],
                [4, 2]
            ),
            "the refresh rate is not presentation"
        );
        for (previous, now, image, why) in [
            (None, whole, [4, 2], "the first image"),
            (Some(crop), whole, [4, 2], "the crop was removed"),
            (Some(crop), crop, [2, 1], "a cropped image"),
            (Some(small), small, [1, 1], "a scaled image"),
        ] {
            assert_eq!(
                changed_region(Some(written), previous, now, [4, 2], image),
                Region::whole(image),
                "{why}"
            );
        }
        assert_eq!(
            changed_region(None, Some(whole), whole, [4, 2], [4, 2]),
            Region::whole([4, 2])
        );
    }

    #[test]
    fn an_untaken_image_adds_its_changes_to_the_next() -> Result<(), Error> {
        let pending = Image {
            frame: Frame::new([4, 2], vec![0; 8])?,
            produced_with: ViewOptions::default(),
            changed: Region {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        };
        let next = Region {
            x: 3,
            y: 1,
            width: 1,
            height: 1,
        };
        assert_eq!(untaken_changes(None, next, [4, 2]), next);
        assert_eq!(untaken_changes(Some(&pending), next, [4, 2]), Region::whole([4, 2]));
        assert_eq!(
            untaken_changes(Some(&pending), next, [3, 2]),
            Region::whole([3, 2]),
            "a pending image of another size"
        );
        let beside = Region {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        };
        assert_eq!(
            untaken_changes(Some(&pending), beside, [4, 2]),
            Region {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            }
        );
        Ok(())
    }
}
