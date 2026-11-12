//! The egui side of a native Device session: its wakes repaint the viewport that
//! shows the viewer, its frames become texture images, and its untrusted server
//! texts become Horizon's bounded labels and native session metadata.
//!
//! A viewer shows one of two kinds of source: a VNC desktop on this machine or
//! another, or a frame source program of this computer. The methods of this module
//! are the seam: the widget draws the same picture, takes the same updates and
//! sends the same kinds of message, and only the source behind them differs.
use std::sync::{Arc, Mutex, PoisonError};

use egui::{ColorImage, Context, ViewportId};
use horizon_core::{
    DevicePanelState,
    browser::manifest::device::{DeviceServerDetails, NativeSessionMetadata},
};
use horizon_frame_source::{
    Config as FrameConfig, InputEvent as FrameInputEvent, InputHello, Session as FrameSession, Stats,
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
    /// The counters of the last `stats` message of a frame source program.
    pub stats: Option<Stats>,
    /// What a frame source program said about input.
    pub input: Option<InputHello>,
    /// The last input refusal of a frame source program.
    pub input_error: Option<String>,
}

/// One viewer: a VNC session or a frame source program.
pub(super) enum Session {
    Vnc(VncSession),
    Frame(Box<FrameSession>),
}

/// The viewport whose pass uploads for this viewer, so a closed detached window
/// cannot keep its wakes.
type Viewport = Arc<Mutex<ViewportId>>;

pub(super) struct VncSession {
    inner: Option<horizon_vnc::Session>,
    viewport: Viewport,
}

impl Session {
    /// The live session of this viewer, for a caller that needs the VNC one.
    fn vnc(&self) -> Option<&horizon_vnc::Session> {
        match self {
            Self::Vnc(session) => session.inner.as_ref(),
            Self::Frame(_) => None,
        }
    }

    pub(super) fn is_frame_source(&self) -> bool {
        matches!(self, Self::Frame(_))
    }

    /// Ends the worker on a thread of its own, so the frame that closes a viewer
    /// waits for nothing.
    pub(super) fn detach(self) {
        match self {
            Self::Vnc(session) => session.detach(),
            Self::Frame(session) => session.detach(),
        }
    }

    /// Start the source of this viewer. A frame source viewer needs the run of its
    /// source, which the settings resolve; a VNC viewer ignores it.
    pub(super) fn start(
        device: &DevicePanelState,
        frame: Option<FrameConfig>,
        ctx: Context,
        viewport: ViewportId,
        options: ViewOptions,
    ) -> Result<Self, String> {
        let vnc_started = || VncSession::start(device, ctx.clone(), viewport, options);
        if let Some(config) = frame {
            let viewport = Arc::new(Mutex::new(viewport));
            let target = Arc::clone(&viewport);
            let session = FrameSession::start(config, move |wake| {
                let viewport = *target.lock().unwrap_or_else(PoisonError::into_inner);
                wake_ui(&ctx, wake, viewport);
            })
            .map_err(|error| error.to_string())?;
            Ok(Self::Frame(Box::new(session)))
        } else {
            vnc_started().map(Self::Vnc).map_err(|error| error.to_string())
        }
    }

    /// Queue input for a VNC server. A worker that has already stopped drops it.
    pub(super) fn send_input(&self, events: impl IntoIterator<Item = InputEvent>) {
        if let Some(session) = self.vnc() {
            session.send_input(events);
        }
    }

    /// Queue input for a frame source program.
    pub(super) fn send_frame_input(&self, events: impl IntoIterator<Item = FrameInputEvent>) {
        if let Self::Frame(session) = self {
            session.send_input(events);
        }
    }

    pub(super) fn set_options(&self, options: ViewOptions) {
        match self {
            Self::Vnc(session) => session.inner().set_options(options),
            Self::Frame(session) => session.set_options(options),
        }
    }

    /// The size that the viewer draws at, in physical pixels, for a source program
    /// that sends the size that it draws. A VNC desktop keeps its own resolution.
    pub(super) fn set_view_size(&self, size: Option<[u32; 2]>) {
        if let Self::Frame(session) = self {
            session.set_view_size(size);
        }
    }

    /// The last picture as the pixels of the widget, uncropped and unscaled.
    pub(super) fn latest_full(&self) -> Option<ColorImage> {
        self.latest_frame()
            .map(|frame| color_image(Arc::unwrap_or_clone(frame)))
    }

    /// The last full picture, for a screenshot or a recording.
    pub(super) fn latest_frame(&self) -> Option<Arc<Frame>> {
        match self {
            Self::Vnc(session) => session.latest_full(),
            Self::Frame(session) => session.latest_full(),
        }
    }

    /// True when this session has a full picture. Does not copy the pixels.
    pub(super) fn has_latest_full(&self) -> bool {
        match self {
            Self::Vnc(session) => session.has_latest_full(),
            Self::Frame(session) => session.has_latest_full(),
        }
    }

    pub(super) fn recording_source(&self) -> Box<dyn Fn() -> Option<Arc<Frame>> + Send + 'static> {
        match self {
            Self::Vnc(session) => Box::new(session.inner().frame_source()),
            Self::Frame(session) => Box::new(session.frame_source()),
        }
    }

    /// Marks the viewer undrawn and routes its wakes to `viewport`, the pass that
    /// uploads for it. Returns whether an update is waiting.
    pub(super) fn pending_in_background(&self, viewport: ViewportId) -> bool {
        match self {
            Self::Vnc(session) => session.pending_in_background(viewport),
            Self::Frame(session) => {
                session.set_visible(false);
                session.pending_while_hidden()
            }
        }
    }

    pub(super) fn set_visible(&self, visible: bool) {
        match self {
            Self::Vnc(session) => session.set_visible(visible),
            Self::Frame(session) => session.set_visible(visible),
        }
    }

    pub(super) fn take_updates(&self, viewport: ViewportId) -> Updates {
        match self {
            Self::Vnc(session) => session.take_updates(viewport),
            Self::Frame(session) => {
                let updates = session.take_updates();
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
                    native_session: None,
                    stats: updates.stats,
                    input: updates.input,
                    input_error: updates.input_error,
                }
            }
        }
    }

    pub(super) fn take_server_details(&self) -> DeviceServerDetails {
        match self {
            Self::Vnc(session) => session.take_server_details(),
            Self::Frame(session) => {
                let details = session.take_server_details();
                DeviceServerDetails {
                    name: server_label(details.name.as_deref()),
                    native_session: None,
                    desktop_size: details.desktop_size,
                }
            }
        }
    }

    pub(super) fn observation(&self) -> Observation {
        match self {
            Self::Vnc(session) => session.observation(),
            Self::Frame(session) => session.observation(),
        }
    }

    pub(super) fn stream_evidence(&self) -> (StreamEvidence, bool) {
        match self {
            Self::Vnc(session) => session.stream_evidence(),
            Self::Frame(session) => (session.stream_evidence(), false),
        }
    }
}

impl VncSession {
    /// The live session of this viewer. It is `Some` for the whole life of the
    /// wrapper, and only the drop takes it.
    fn inner(&self) -> &horizon_vnc::Session {
        self.inner
            .as_ref()
            .unwrap_or_else(|| unreachable!("a viewer session is live until its drop"))
    }

    /// Ends the worker on a thread of its own, so the frame that closes a viewer
    /// waits for nothing.
    fn detach(mut self) {
        if let Some(session) = self.inner.take() {
            session.detach();
        }
    }

    fn start(
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
        Ok(Self {
            inner: Some(inner),
            viewport,
        })
    }

    fn route_wakes_to(&self, viewport: ViewportId) {
        *self.viewport.lock().unwrap_or_else(PoisonError::into_inner) = viewport;
    }

    fn set_visible(&self, visible: bool) {
        self.inner().set_visible(visible);
    }

    fn pending_in_background(&self, viewport: ViewportId) -> bool {
        self.route_wakes_to(viewport);
        self.inner().pending_while_hidden()
    }

    fn take_updates(&self, viewport: ViewportId) -> Updates {
        self.route_wakes_to(viewport);
        let updates = self.inner().take_updates();
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
            stats: None,
            input: None,
            input_error: None,
        }
    }

    fn take_server_details(&self) -> DeviceServerDetails {
        server_details(&self.inner().take_server_details())
    }

    fn latest_full(&self) -> Option<Arc<Frame>> {
        self.inner().latest_full()
    }

    fn has_latest_full(&self) -> bool {
        self.inner().has_latest_full()
    }

    fn observation(&self) -> Observation {
        self.inner().observation()
    }

    fn stream_evidence(&self) -> (StreamEvidence, bool) {
        (self.inner().stream_evidence(), false)
    }
}

impl Drop for VncSession {
    fn drop(&mut self) {
        // Closing a viewer must not wait for its worker or its tunnel: the wake
        // of that worker reaches a viewport that no longer draws.
        if let Some(session) = self.inner.take() {
            session.detach();
        }
    }
}

/// How the worker of a VNC viewer reaches its server: directly, or through `ssh -W`
/// on the tunnel host, where the target is the endpoint as that host sees it.
fn route(device: &DevicePanelState) -> Route {
    let address = vnc_address(device);
    match &device.ssh_tunnel {
        Some(connection) => Route::Tunnel {
            program: "ssh".into(),
            args: connection.stdio_forward_args(&address.to_string()),
            host: connection.host.clone(),
            port: connection.port,
        },
        None => Route::Direct(address),
    }
}

/// The address of the VNC desktop of a viewer that is not a frame source.
fn vnc_address(device: &DevicePanelState) -> std::net::SocketAddr {
    device
        .target
        .vnc_address()
        .unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], 5900)))
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

impl Session {
    /// Push one clipboard text into the VNC session of this viewer, for a test of
    /// the native session metadata.
    #[cfg(test)]
    pub(super) fn native_text_for_test(&self, text: &str) -> bool {
        self.vnc().is_some_and(|session| session.push_server_text(text))
    }

    #[cfg(test)]
    pub(super) fn pending_frame(latest_full: ColorImage, image: ColorImage, produced_with: ViewOptions) -> Self {
        Self::Vnc(VncSession::pending_frame(latest_full, image, produced_with))
    }

    /// A connected-looking session whose queued input the test can read back.
    #[cfg(test)]
    pub(super) fn pending_frame_with_input(
        latest_full: ColorImage,
        image: ColorImage,
        produced_with: ViewOptions,
    ) -> (Self, horizon_vnc::SentInput) {
        let (session, input) = VncSession::pending_frame_with_input(latest_full, image, produced_with);
        (Self::Vnc(session), input)
    }
}

impl VncSession {
    #[cfg(test)]
    fn pending_frame(latest_full: ColorImage, image: ColorImage, produced_with: ViewOptions) -> Self {
        Self::pending_frame_with_input(latest_full, image, produced_with).0
    }

    /// A connected-looking session whose queued input the test can read back.
    #[cfg(test)]
    fn pending_frame_with_input(
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
            inner: Some(inner),
            viewport: Arc::new(Mutex::new(ViewportId::ROOT)),
        };
        (session, input)
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
        let Route::Tunnel {
            program,
            args,
            host,
            port,
        } = route(&device)
        else {
            panic!("expected a tunnel");
        };
        assert_eq!(program, "ssh");
        assert_eq!(args, connection.stdio_forward_args("127.0.0.1:5901"));
        assert_eq!(host, "lab.example", "the refusal names the host ssh was given");
        assert_eq!(port, None, "an unset port stays with the SSH configuration");
    }
}
