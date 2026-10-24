//! A live VNC session. One worker thread connects, decodes the desktop and
//! keeps only the latest image, so a slow UI never queues frames. The UI
//! takes what changed when it draws and sends input at any time.
#[cfg(any(test, feature = "test-support"))]
mod fixture;
mod host_key;
mod tunnel;
mod worker;

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Instant,
};

use tokio::sync::{mpsc, oneshot};

use host_key::TunnelHost;

use crate::{Error, Frame, InputEvent, Region, ViewOptions};

#[cfg(any(test, feature = "test-support"))]
pub use fixture::SentInput;
pub use host_key::{HostKeyFailure, HostKeyRefusal, ObservedHostKey};

/// How the session reaches the VNC server.
#[derive(Clone, Debug)]
pub enum Route {
    /// A server that this machine reaches over TCP.
    Direct(SocketAddr),
    /// A program whose standard input and output carry the connection, for
    /// example `ssh -W host:port`. The program stops with the connection, and
    /// its last standard error lines are added to a failure message.
    Tunnel {
        program: String,
        args: Vec<String>,
        /// The SSH host the program reaches, for a host-key refusal. `ssh`
        /// itself resolves the address from this machine's configuration.
        host: String,
        /// The SSH port the route names; None leaves it to that configuration.
        port: Option<u16>,
    },
}

impl Route {
    /// The SSH host a tunnel route names, for a host-key refusal.
    fn tunnel_host(&self) -> Option<TunnelHost> {
        match self {
            Self::Direct(_) => None,
            Self::Tunnel {
                program, host, port, ..
            } => Some(TunnelHost {
                program: program.clone(),
                host: host.clone(),
                port: *port,
            }),
        }
    }
}

/// What a session connects to and how it presents the desktop.
#[derive(Clone, Debug)]
pub struct Config {
    pub route: Route,
    pub options: ViewOptions,
    /// Which server clipboard texts [`Updates::server_text`] keeps. A text
    /// that this refuses causes no wake and does not replace a kept text.
    pub keep_server_text: fn(&str) -> bool,
}

impl Config {
    /// Default view options, and every server clipboard text is kept.
    #[must_use]
    pub fn new(route: Route) -> Self {
        Self {
            route,
            options: ViewOptions::default(),
            keep_server_text: |_| true,
        }
    }
}

/// The state of the connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Stopped,
    Connecting,
    Connected,
    /// The connection failed or ended. `message` is what a viewer shows, and
    /// `host_key` types an SSH host-key refusal, which a caller can tell from
    /// an unreachable host.
    Disconnected {
        message: String,
        host_key: Option<HostKeyFailure>,
    },
}

impl Status {
    /// A failure that Horizon cannot type further, with the text a viewer shows.
    #[must_use]
    pub fn disconnected(message: impl Into<String>) -> Self {
        Self::Disconnected {
            message: message.into(),
            host_key: None,
        }
    }

    /// A failure as the transport reported it.
    fn from_error(error: &Error) -> Self {
        Self::Disconnected {
            message: error.to_string(),
            host_key: error.host_key_failure().cloned(),
        }
    }
}

/// Why the session asks the UI to draw again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// The UI draws this viewer: show the update now.
    Visible,
    /// The UI does not draw this viewer (see [`Session::set_visible`]). The
    /// update can wait, so a hidden stream does not drive the UI at its rate.
    Hidden,
}

/// A new image from the worker.
#[derive(Clone, Debug)]
pub struct Image {
    pub frame: Frame,
    /// The options that cropped and scaled it, as they apply to the desktop.
    pub produced_with: ViewOptions,
    /// Bounds every pixel that can differ from the image that the previous
    /// [`Session::take_updates`] returned. It is the whole image for the
    /// first image, after a resize, after a change of the crop or the size
    /// limits, and while the image is cropped or scaled.
    pub changed: Region,
}

/// What changed after the last [`Session::take_updates`].
#[derive(Debug, Default)]
pub struct Updates {
    /// The latest image. An image that the UI did not take is replaced.
    pub image: Option<Image>,
    pub status: Option<Status>,
    /// How many images the worker published in this session.
    pub received_frame_sequence: u64,
    /// The desktop size that the server announced last.
    pub desktop: Option<[usize; 2]>,
    /// The desktop name from the handshake. It is untrusted text of at most
    /// 4 KiB that can hold control characters: bound and clean it before a
    /// UI shows it.
    pub server_name: Option<String>,
    /// The latest server clipboard text that [`Config::keep_server_text`] kept.
    pub server_text: Option<String>,
}

/// When the worker published its last image. Taking updates does not reset it.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamEvidence {
    pub sequence: u64,
    pub last_frame: Option<Instant>,
}

/// A status change and the image count, without taking the image.
#[derive(Debug)]
pub struct Observation {
    pub status: Option<Status>,
    pub received_frame_sequence: u64,
}

/// What the server said about itself, without taking the image or the status.
#[derive(Debug, Default)]
pub struct ServerDetails {
    pub name: Option<String>,
    pub desktop_size: Option<[usize; 2]>,
    pub text: Option<String>,
}

/// What the worker writes and the UI takes.
struct State {
    stream: StreamEvidence,
    image: Option<Image>,
    status: Option<Status>,
    received_frame_sequence: u64,
    visible: bool,
    options: ViewOptions,
    desktop: Option<[usize; 2]>,
    server_name: Option<String>,
    server_text: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    wake: Box<dyn Fn(Wake) + Send + Sync>,
    keep_server_text: fn(&str) -> bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A drawn viewer is woken for every update; an undrawn one only hears
    /// that an update waits.
    fn wake(&self, visible: bool) {
        (self.wake)(if visible { Wake::Visible } else { Wake::Hidden });
    }

    fn publish_status(&self, status: Status) {
        let visible = {
            let mut state = self.lock();
            state.status = Some(status);
            state.visible
        };
        self.wake(visible);
    }

    fn publish_text(&self, text: &str) -> bool {
        if !(self.keep_server_text)(text) {
            return false;
        }
        let visible = {
            let mut state = self.lock();
            state.server_text = Some(text.to_owned());
            state.visible
        };
        self.wake(visible);
        true
    }
}

/// A connection to one VNC server. Dropping it cancels the worker, closes the
/// connection and stops a tunnel program; drop it and start a new session to
/// reconnect.
pub struct Session {
    shared: Arc<Shared>,
    latest_full: Arc<Mutex<Option<Arc<Frame>>>>,
    live: Arc<AtomicBool>,
    /// Pointer and key events that a person sends; the worker forwards them
    /// ahead of the next refresh.
    input: mpsc::UnboundedSender<InputEvent>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Start the worker thread. `wake` runs on that thread for each status,
    /// image and kept clipboard text; it must not block.
    ///
    /// # Errors
    /// The worker thread could not start.
    pub fn start(config: Config, wake: impl Fn(Wake) + Send + Sync + 'static) -> Result<Self, Error> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                stream: StreamEvidence::default(),
                image: None,
                status: Some(Status::Connecting),
                received_frame_sequence: 0,
                visible: true,
                options: config.options,
                desktop: None,
                server_name: None,
                server_text: None,
            }),
            wake: Box::new(wake),
            keep_server_text: config.keep_server_text,
        });
        let latest_full = Arc::new(Mutex::new(None));
        let worker_shared = Arc::clone(&shared);
        let retained = Arc::clone(&latest_full);
        let live = Arc::new(AtomicBool::new(true));
        let worker_live = Arc::clone(&live);
        let (stop, cancelled) = oneshot::channel();
        let (input, mut input_events) = mpsc::unbounded_channel();
        let route = config.route;
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
                            result = worker::connection(route, &worker_shared, &retained, &mut input_events) => result,
                        }
                    })
                });
            let status = match result {
                Ok(Ok(())) => Status::Stopped,
                Ok(Err(error)) => Status::from_error(&error),
                Err(error) => Status::disconnected(error.to_string()),
            };
            worker_live.store(false, Ordering::Release);
            worker_shared.publish_status(status);
        })?;
        Ok(Self {
            shared,
            latest_full,
            live,
            input,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// Queue input for the server. A worker that has already stopped drops it.
    pub fn send_input(&self, events: impl IntoIterator<Item = InputEvent>) {
        for event in events {
            if self.input.send(event).is_err() {
                return;
            }
        }
    }

    /// Crop, limits and refresh rate for the next images.
    pub fn set_options(&self, options: ViewOptions) {
        self.shared.lock().options = options;
    }

    /// The last full desktop, uncropped and unscaled. It stays after a
    /// disconnect, so a UI can present it again.
    #[must_use]
    pub fn latest_full(&self) -> Option<Arc<Frame>> {
        self.latest_full.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// True when this session has a full desktop. Does not copy the pixels.
    #[must_use]
    pub fn has_latest_full(&self) -> bool {
        self.latest_full
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// The latest full desktop for another thread, such as a recorder. It
    /// gives `None` after the worker stops or the session drops, and the same
    /// `Arc` until a new image arrives.
    pub fn frame_source(&self) -> impl Fn() -> Option<Arc<Frame>> + Send + 'static {
        let source = Arc::downgrade(&self.latest_full);
        let live = Arc::clone(&self.live);
        move || {
            if !live.load(Ordering::Acquire) {
                return None;
            }
            source.upgrade()?.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    /// Whether the UI draws this viewer. While it does not, wakes are
    /// [`Wake::Hidden`]; the worker still receives and keeps the latest image.
    pub fn set_visible(&self, visible: bool) {
        self.shared.lock().visible = visible;
    }

    /// Marks the viewer undrawn and returns whether an image, a status or a
    /// kept clipboard text is waiting.
    #[must_use]
    pub fn pending_while_hidden(&self) -> bool {
        let mut state = self.shared.lock();
        state.visible = false;
        state.image.is_some() || state.status.is_some() || state.server_text.is_some()
    }

    /// Take the waiting image, status, server name and clipboard text.
    #[must_use]
    pub fn take_updates(&self) -> Updates {
        let mut state = self.shared.lock();
        Updates {
            image: state.image.take(),
            status: state.status.take(),
            received_frame_sequence: state.received_frame_sequence,
            desktop: state.desktop,
            server_name: state.server_name.take(),
            server_text: state.server_text.take(),
        }
    }

    /// Take the waiting server name and clipboard text; the image stays.
    #[must_use]
    pub fn take_server_details(&self) -> ServerDetails {
        let mut state = self.shared.lock();
        ServerDetails {
            name: state.server_name.take(),
            desktop_size: state.desktop,
            text: state.server_text.take(),
        }
    }

    /// Take the waiting status; the image stays.
    #[must_use]
    pub fn observation(&self) -> Observation {
        let mut state = self.shared.lock();
        Observation {
            status: state.status.take(),
            received_frame_sequence: state.received_frame_sequence,
        }
    }

    #[must_use]
    pub fn stream_evidence(&self) -> StreamEvidence {
        self.shared.lock().stream
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

#[cfg(test)]
mod tests;
