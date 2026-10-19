//! The egui side of a `horizon_vnc` session: its wakes repaint the viewport
//! that shows the viewer, its frames become texture images, and its untrusted
//! server texts become Horizon's bounded labels and native session metadata.
use std::sync::{Arc, Mutex, PoisonError};

use egui::{ColorImage, Context, ViewportId};
use horizon_core::{
    DevicePanelState,
    browser::manifest::device::{DeviceServerDetails, NativeSessionMetadata},
};
use horizon_vnc::{Config, Frame, InputEvent, Route, ViewOptions, Wake};

pub(super) use horizon_vnc::{Error as ViewError, Observation, Status, StreamEvidence};

use super::picture::color_image;

pub(super) struct Updates {
    pub image: Option<ColorImage>,
    pub produced_with: Option<ViewOptions>,
    pub status: Option<Status>,
    pub received_frame_sequence: u64,
    pub desktop: Option<[usize; 2]>,
    pub server_name: Option<String>,
    pub native_session: Option<NativeSessionMetadata>,
}

pub(super) struct Session {
    inner: horizon_vnc::Session,
    /// The viewport whose pass uploads for this viewer, so a closed detached
    /// window cannot keep its wakes.
    viewport: Arc<Mutex<ViewportId>>,
}

/// How the worker reaches the VNC server: directly, or through `ssh -W` on
/// the tunnel host, where the target is the endpoint as that host sees it.
fn route(device: &DevicePanelState) -> Route {
    match &device.ssh_tunnel {
        Some(connection) => Route::Tunnel {
            program: "ssh".into(),
            args: connection.stdio_forward_args(&device.target.address().to_string()),
        },
        None => Route::Direct(device.target.address()),
    }
}

/// Only native session metadata is kept from the server clipboard; ordinary
/// clipboard text never replaces it.
fn is_native_metadata(text: &str) -> bool {
    NativeSessionMetadata::from_wire_text(text).is_some()
}

/// A drawn viewer repaints for every frame; an undrawn one only asks for the
/// bounded background upload, so a hidden or off-canvas stream never drives
/// the host at its own rate.
fn wake_ui(ctx: &Context, wake: Wake, viewport: ViewportId) {
    match wake {
        Wake::Visible => ctx.request_repaint_of(viewport),
        Wake::Hidden => ctx.request_repaint_after_for(super::BACKGROUND_UPLOAD_INTERVAL, viewport),
    }
}

impl Session {
    pub(super) fn start(
        device: &DevicePanelState,
        ctx: Context,
        viewport: ViewportId,
        options: ViewOptions,
    ) -> Result<Self, ViewError> {
        let viewport = Arc::new(Mutex::new(viewport));
        let target = Arc::clone(&viewport);
        let config = Config {
            route: route(device),
            options,
            keep_server_text: is_native_metadata,
        };
        let inner = horizon_vnc::Session::start(config, move |wake| {
            let viewport = *target.lock().unwrap_or_else(PoisonError::into_inner);
            wake_ui(&ctx, wake, viewport);
        })?;
        Ok(Self { inner, viewport })
    }

    fn route_wakes_to(&self, viewport: ViewportId) {
        *self.viewport.lock().unwrap_or_else(PoisonError::into_inner) = viewport;
    }

    /// Queue input for the server. A worker that has already stopped drops it.
    pub(super) fn send_input(&self, events: impl IntoIterator<Item = InputEvent>) {
        self.inner.send_input(events);
    }

    pub(super) fn set_options(&self, options: ViewOptions) {
        self.inner.set_options(options);
    }

    pub(super) fn latest_full(&self) -> Option<ColorImage> {
        self.inner
            .latest_full()
            .map(|frame| color_image(Arc::unwrap_or_clone(frame)))
    }

    pub(super) fn latest_frame(&self) -> Option<Arc<Frame>> {
        self.inner.latest_full()
    }

    /// True when this session has a full desktop. Does not copy the pixels.
    pub(super) fn has_latest_full(&self) -> bool {
        self.inner.has_latest_full()
    }

    pub(super) fn recording_source(&self) -> impl Fn() -> Option<Arc<Frame>> + Send + 'static {
        self.inner.frame_source()
    }

    /// Marks the viewer undrawn and routes its wakes to `viewport`, the pass
    /// that uploads for it. Returns whether an image or status change is waiting.
    pub(super) fn pending_in_background(&self, viewport: ViewportId) -> bool {
        self.route_wakes_to(viewport);
        self.inner.pending_while_hidden()
    }

    pub(super) fn set_visible(&self, visible: bool) {
        self.inner.set_visible(visible);
    }

    pub(super) fn take_updates(&self, viewport: ViewportId) -> Updates {
        self.route_wakes_to(viewport);
        let updates = self.inner.take_updates();
        let (image, produced_with) = updates
            .image
            .map(|image| (color_image(image.frame), image.produced_with))
            .unzip();
        Updates {
            image,
            produced_with,
            status: updates.status,
            received_frame_sequence: updates.received_frame_sequence,
            desktop: updates.desktop,
            server_name: server_label(updates.server_name.as_deref()),
            native_session: native_metadata(updates.server_text.as_deref()),
        }
    }

    pub(super) fn take_server_details(&self) -> DeviceServerDetails {
        server_details(&self.inner.take_server_details())
    }

    pub(super) fn observation(&self) -> Observation {
        self.inner.observation()
    }

    pub(super) fn stream_evidence(&self) -> (StreamEvidence, bool) {
        (self.inner.stream_evidence(), false)
    }

    #[cfg(test)]
    pub(super) fn pending_frame(latest_full: ColorImage, image: ColorImage, produced_with: ViewOptions) -> Self {
        Self::pending_frame_with_input(latest_full, image, produced_with).0
    }

    /// A connected-looking session whose queued input the test can read back.
    #[cfg(test)]
    pub(super) fn pending_frame_with_input(
        latest_full: ColorImage,
        image: ColorImage,
        produced_with: ViewOptions,
    ) -> (Self, horizon_vnc::SentInput) {
        let (inner, input) = horizon_vnc::Session::fixture(
            super::picture::frame(latest_full),
            super::picture::frame(image),
            produced_with,
            is_native_metadata,
        );
        let session = Self {
            inner,
            viewport: Arc::new(Mutex::new(ViewportId::ROOT)),
        };
        (session, input)
    }

    #[cfg(test)]
    pub(super) fn native_text_for_test(&self, text: &str) -> bool {
        self.inner.push_server_text(text)
    }
}

/// Bound the untrusted handshake name and flatten its controls for display.
fn server_label(raw: Option<&str>) -> Option<String> {
    raw.and_then(DevicePanelState::server_label)
}

fn native_metadata(text: Option<&str>) -> Option<NativeSessionMetadata> {
    text.and_then(NativeSessionMetadata::from_wire_text)
}

fn server_details(details: &horizon_vnc::ServerDetails) -> DeviceServerDetails {
    DeviceServerDetails {
        name: server_label(details.name.as_deref()),
        native_session: native_metadata(details.text.as_deref()),
        desktop_size: details.desktop_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_names_are_bounded_plain_labels_before_anything_shows_them() {
        for (raw, expected) in [
            ("Lab desktop ÆØÅ", Some("Lab desktop ÆØÅ")),
            ("", None),
            ("\nLab\0desktop", Some("Lab desktop")),
        ] {
            let details = server_details(&horizon_vnc::ServerDetails {
                name: Some(raw.into()),
                desktop_size: Some([2, 2]),
                text: None,
            });
            assert_eq!(details.name.as_deref(), expected);
            assert_eq!(details.desktop_size, Some([2, 2]));
        }
    }

    #[test]
    fn hidden_wakes_schedule_the_background_upload_and_visible_ones_repaint_now() {
        use crate::test_egui::DiscardTextures;
        let ctx = Context::default();
        // Drain the first passes so coalescing cannot hide a background request.
        for _ in 0..3 {
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();
        }
        let delays = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&delays);
        ctx.set_request_repaint_callback(move |info| recorded.lock().unwrap().push(info.delay));
        wake_ui(&ctx, Wake::Hidden, ViewportId::ROOT);
        let hidden = std::mem::take(&mut *delays.lock().unwrap());
        assert!(!hidden.is_empty(), "a hidden frame still schedules its upload");
        assert!(
            hidden
                .iter()
                .all(|delay| !delay.is_zero() && *delay <= super::super::BACKGROUND_UPLOAD_INTERVAL),
            "nothing repaints the host at the stream rate: {hidden:?}"
        );
        wake_ui(&ctx, Wake::Visible, ViewportId::ROOT);
        assert!(delays.lock().unwrap().iter().any(std::time::Duration::is_zero));
    }

    #[test]
    fn a_tunnelled_viewer_reaches_its_target_through_ssh() {
        let mut device = DevicePanelState {
            target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5901").unwrap(),
            input: horizon_core::DeviceInput::Accepted,
            identity: None,
            connect_on_start: false,
            ssh_tunnel: None,
        };
        assert!(matches!(route(&device), Route::Direct(address) if address.port() == 5901));
        let connection = horizon_core::SshConnection {
            host: "lab.example".into(),
            ..Default::default()
        };
        device.ssh_tunnel = Some(connection.clone());
        let Route::Tunnel { program, args } = route(&device) else {
            panic!("expected a tunnel");
        };
        assert_eq!(program, "ssh");
        assert_eq!(args, connection.stdio_forward_args("127.0.0.1:5901"));
    }
}
