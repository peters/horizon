//! A live session of one frame source program.
//!
//! [`Session::start`] starts the program of the settings on a worker thread of its
//! own, gives it a Unix stream socket as descriptor 3 for its pictures and pipes as
//! its standard input and output for its control messages, and keeps only the
//! newest picture. The UI takes what changed when it draws and sends input at any
//! time, as it does with a VNC session.
//!
//! The order of one session is: the program says `hello`, it says `stream` before
//! its first picture, and then pictures arrive. A picture is converted on the
//! worker and only when the frame took the picture before it, so a slow frame
//! costs no conversion and never shows an older picture after a newer one. The
//! widget asks for the size that it draws at, after that size was stable for
//! [`VIEW_DEBOUNCE`], and for no picture while it does not draw the viewer.
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use horizon_vnc::{Frame, Image, Observation, Region, ServerDetails, Status, StreamEvidence, ViewOptions, Wake};

use crate::{
    Error, Picture,
    control::{Message, Outgoing, Stats},
    convert, text, worker,
};

/// How long one size of the viewer must be stable before the program is asked for
/// it, so a person who drags the window sends one request and not a hundred.
pub const VIEW_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long `close` waits for a program after `bye`, and then after `SIGTERM`.
pub const STOP_WAIT: Duration = Duration::from_secs(2);

/// How often a worker looks whether its program ended.
pub(crate) const POLL: Duration = Duration::from_millis(10);

/// How often a worker looks whether its program ended while it waits for a
/// picture or for a message, so a program that ended by itself is noticed without
/// a second thread for the state of a child.
pub(crate) const IDLE_POLL: Duration = Duration::from_millis(100);

/// The longest text that Horizon shows of one program, in characters.
pub const MAX_LABEL_CHARS: usize = 200;

/// What one frame source program is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The name of the source in the settings, for a sentence of a panel.
    pub name: String,
    /// The program of the line of the settings, as a person wrote it.
    pub program: PathBuf,
    /// The arguments of that line, after the program.
    pub arguments: Vec<std::ffi::OsString>,
}

/// What the program said about input, after `hello`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputHello {
    /// Whether the program reads `input` messages.
    pub allowed: bool,
    /// Why it reads none, as it said it, cut and cleaned.
    pub reason: Option<String>,
}

/// What changed after the last [`Session::take_updates`].
#[derive(Debug, Default)]
pub struct Updates {
    /// The newest converted picture. A picture that the frame did not take is
    /// replaced, never queued.
    pub image: Option<Image>,
    pub status: Option<Status>,
    /// How many pictures the worker published in this session.
    pub received_frame_sequence: u64,
    /// The size of the newest picture.
    pub desktop: Option<[usize; 2]>,
    /// The name that the program gave itself, cut and cleaned.
    pub server_name: Option<String>,
    /// What the program said about input, when it said it.
    pub input: Option<InputHello>,
    /// The counters of the last `stats` message.
    pub stats: Option<Stats>,
    /// The last input refusal of the program.
    pub input_error: Option<String>,
}

/// One message or one command of the session.
pub(crate) enum Event {
    /// A message of the viewer's UI.
    Send(Outgoing),
    /// A size that the viewer draws at, to ask for after [`VIEW_DEBOUNCE`].
    Size([u32; 2]),
    /// The viewer is drawn or not.
    Visible(bool),
    /// The program said something.
    Message(Message),
    /// The reader of the pictures ended, with the fault that ended it.
    PicturesEnded(Option<Error>),
    /// The reader of the control stream ended, with the fault that ended it.
    ControlEnded(Option<Error>),
    /// The last line that the program wrote on its standard error.
    Stderr(String),
    /// The UI closed the viewer.
    Stop,
}

/// The state that the worker writes and the UI takes.
pub(crate) struct State {
    pub(crate) stream: StreamEvidence,
    pub(crate) image: Option<Image>,
    pub(crate) status: Option<Status>,
    pub(crate) received_frame_sequence: u64,
    pub(crate) visible: bool,
    pub(crate) options: ViewOptions,
    pub(crate) desktop: Option<[usize; 2]>,
    pub(crate) source_size: Option<[usize; 2]>,
    pub(crate) server_name: Option<String>,
    pub(crate) hello: Option<InputHello>,
    pub(crate) stats: Option<Stats>,
    pub(crate) input_error: Option<String>,
    /// The newest picture that arrived and is not converted, because the frame did
    /// not take the picture before it.
    pub(crate) raw: Option<Picture>,
    /// Whether the frame took the published image.
    pub(crate) image_taken: bool,
    /// The last line of the standard error of the program.
    pub(crate) last_stderr: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            stream: StreamEvidence::default(),
            image: None,
            status: Some(Status::Connecting),
            received_frame_sequence: 0,
            visible: true,
            options: ViewOptions::default(),
            desktop: None,
            source_size: None,
            server_name: None,
            hello: None,
            stats: None,
            input_error: None,
            raw: None,
            image_taken: true,
            last_stderr: None,
        }
    }
}

/// What the worker and the UI share.
pub(crate) struct Shared {
    state: Mutex<State>,
    /// Signalled when the frame takes the published image, so the worker converts
    /// the picture that waited instead of doing it while nobody looks.
    taken: Condvar,
    wake: Box<dyn Fn(Wake) + Send + Sync>,
    /// The whole of the newest picture, uncropped, for a screenshot or a recording.
    pub(crate) latest_full: Mutex<Option<Arc<Frame>>>,
}

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn wake(&self, visible: bool) {
        (self.wake)(if visible { Wake::Visible } else { Wake::Hidden });
    }

    /// Publish `status` and wake the UI.
    pub(crate) fn publish_status(&self, status: Status) {
        let visible = {
            let mut state = self.lock();
            state.status = Some(status);
            state.visible
        };
        self.wake(visible);
    }

    /// Keep the newest picture, and convert it now when the frame took the last
    /// one. A picture that waits is replaced by the next one without a convert.
    ///
    /// Answers whether a picture still waits for the frame.
    pub(crate) fn publish_picture(&self, picture: Picture) -> bool {
        let (visible, waiting) = {
            let mut state = self.lock();
            state.raw = Some(picture);
            let waits = state.image.is_some() && !state.image_taken;
            if !waits && let Err(error) = convert_raw(&mut state, &self.latest_full) {
                // A picture that this process cannot convert is one typed failure
                // of the viewer, and the worker ends it.
                state.status = Some(Status::disconnected(error.to_string()));
            }
            (state.visible, waits || state.raw.is_some())
        };
        self.wake(visible);
        waiting
    }

    /// Convert the picture that waits, when the frame took the image before it.
    ///
    /// # Errors
    /// Reports a picture that this process cannot convert.
    pub(crate) fn convert_waiting(&self) -> Result<(), Error> {
        let visible = {
            let mut state = self.lock();
            if state.raw.is_none() || (state.image.is_some() && !state.image_taken) {
                return Ok(());
            }
            convert_raw(&mut state, &self.latest_full)?;
            state.visible
        };
        self.wake(visible);
        Ok(())
    }

    /// Wait until the frame takes the published image, or until `bound` passes.
    pub(crate) fn wait_for_taken(&self, bound: Duration) {
        let state = self.lock();
        let (state, _) = self
            .taken
            .wait_timeout_while(state, bound, |state| !state.image_taken)
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
    }
}

/// Convert the picture that waits and publish it.
fn convert_raw(state: &mut State, latest: &Mutex<Option<Arc<Frame>>>) -> Result<(), Error> {
    let Some(picture) = state.raw.take() else {
        return Ok(());
    };
    let frame = convert::frame(&picture)?;
    let size = frame.size();
    let presented = frame.present(state.options)?;
    *latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(frame));
    state.desktop = Some(size);
    state.received_frame_sequence = state.received_frame_sequence.saturating_add(1);
    state.stream = StreamEvidence {
        sequence: state.received_frame_sequence,
        last_frame: Some(Instant::now()),
    };
    state.image = Some(Image {
        frame: presented,
        produced_with: state.options,
        changed: Region::whole(size),
    });
    state.image_taken = false;
    Ok(())
}

/// What one size of the viewer is, and since when it is that size.
#[derive(Debug, Default)]
pub(crate) struct ViewQueue {
    pending: Option<([u32; 2], Instant)>,
    sent: Option<[u32; 2]>,
}

impl ViewQueue {
    /// Remember that the viewer draws at `size` now.
    pub(crate) fn offer(&mut self, size: [u32; 2], now: Instant) {
        if self.pending.map(|(pending, _)| pending) == Some(size) || self.sent == Some(size) {
            return;
        }
        self.pending = Some((size, now));
    }

    /// How long until the size of the viewer is stable enough to ask for.
    pub(crate) fn deadline(&self, now: Instant) -> Option<Duration> {
        self.pending
            .map(|(_, since)| (since + VIEW_DEBOUNCE).saturating_duration_since(now))
    }

    /// The size to ask for, once it has been stable for [`VIEW_DEBOUNCE`].
    pub(crate) fn due(&mut self, now: Instant) -> Option<[u32; 2]> {
        let (size, since) = self.pending?;
        if now < since + VIEW_DEBOUNCE {
            return None;
        }
        self.pending = None;
        self.sent = Some(size);
        Some(size)
    }
}

/// A live session of one frame source program. Dropping it stops the program;
/// call [`Session::detach`] to end it on a thread of its own.
pub struct Session {
    shared: Arc<Shared>,
    live: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Start the program on a worker thread. `wake` runs on worker threads for each
    /// picture and status; it must not block.
    ///
    /// # Errors
    /// The worker thread could not start. A program that cannot be started is a
    /// status of the session and not a failure of this call.
    pub fn start(config: Config, wake: impl Fn(Wake) + Send + Sync + 'static) -> Result<Self, Error> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            taken: Condvar::new(),
            wake: Box::new(wake),
            latest_full: Mutex::new(None),
        });
        let live = Arc::new(AtomicBool::new(true));
        let (events, receiver) = mpsc::channel();
        let worker_events = events.clone();
        let worker_shared = Arc::clone(&shared);
        let worker_live = Arc::clone(&live);
        let name = config.name.clone();
        let thread = std::thread::Builder::new()
            .name("frame-source".into())
            .spawn(move || {
                if let Err(error) = worker::run(config, Arc::clone(&worker_shared), worker_events, receiver) {
                    worker_shared.publish_status(Status::disconnected(error.to_string()));
                }
                worker_live.store(false, Ordering::Release);
            })
            .map_err(|error| Error::NotStarted(format!("the worker of {name} did not start: {error}")))?;
        Ok(Self {
            shared,
            live,
            events,
            thread: Some(thread),
        })
    }

    /// The name of the source that this session shows.
    #[must_use]
    pub fn program(&self) -> String {
        self.shared
            .lock()
            .server_name
            .clone()
            .unwrap_or_else(|| "The frame source program".to_owned())
    }

    /// Queue one input event for the program. A worker that has stopped drops it.
    pub fn send_input(&self, events: impl IntoIterator<Item = crate::InputEvent>) {
        for event in events {
            if self.events.send(Event::Send(Outgoing::Input(event))).is_err() {
                return;
            }
        }
    }

    /// Crop and scale for the next pictures, as a VNC session does.
    pub fn set_options(&self, options: ViewOptions) {
        self.shared.lock().options = options;
    }

    /// The size that the viewer draws at, in physical pixels. The program is asked
    /// for it after it has been stable for [`VIEW_DEBOUNCE`], and for the size of
    /// the source at most.
    pub fn set_view_size(&self, size: Option<[u32; 2]>) {
        if let Some(size) = size {
            let _ = self.events.send(Event::Size(size));
        }
    }

    /// Whether the UI draws this viewer. While it does not, the program sends no
    /// picture.
    pub fn set_visible(&self, visible: bool) {
        let _ = self.events.send(Event::Visible(visible));
    }

    /// The last full picture, uncropped and unscaled. It stays after the program
    /// ended, so a UI can present it again.
    #[must_use]
    pub fn latest_full(&self) -> Option<Arc<Frame>> {
        self.shared
            .latest_full
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// True when this session has a picture. Does not copy the pixels.
    #[must_use]
    pub fn has_latest_full(&self) -> bool {
        self.latest_full().is_some()
    }

    /// The newest picture for another thread, such as a recorder.
    pub fn frame_source(&self) -> impl Fn() -> Option<Arc<Frame>> + Send + 'static {
        let source = Arc::downgrade(&self.shared);
        let live = Arc::clone(&self.live);
        move || {
            if !live.load(Ordering::Acquire) {
                return None;
            }
            source
                .upgrade()?
                .latest_full
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// Marks the viewer undrawn and answers whether a picture or a status waits.
    #[must_use]
    pub fn pending_while_hidden(&self) -> bool {
        let mut state = self.shared.lock();
        state.visible = false;
        state.image.is_some() || state.status.is_some() || state.raw.is_some()
    }

    /// Take the waiting picture, status, name, counters and input refusal.
    #[must_use]
    pub fn take_updates(&self) -> Updates {
        let mut state = self.shared.lock();
        let updates = Updates {
            image: state.image.take(),
            status: state.status.take(),
            received_frame_sequence: state.received_frame_sequence,
            desktop: state.desktop,
            server_name: text::label(state.server_name.as_deref(), MAX_LABEL_CHARS),
            input: state.hello.clone(),
            stats: state.stats,
            input_error: text::label(state.input_error.as_deref(), MAX_LABEL_CHARS),
        };
        if updates.image.is_some() {
            state.image_taken = true;
        }
        drop(state);
        // The worker may hold a picture that arrived while the last image waited.
        self.shared.taken.notify_all();
        updates
    }

    /// Take the waiting name and size; the image stays.
    #[must_use]
    pub fn take_server_details(&self) -> ServerDetails {
        let mut state = self.shared.lock();
        ServerDetails {
            name: text::label(state.server_name.take().as_deref(), MAX_LABEL_CHARS),
            desktop_size: state.desktop,
            text: None,
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

    /// End the worker on a thread of its own, so the frame that closes a viewer
    /// waits for no program.
    pub fn detach(mut self) {
        let _ = self.events.send(Event::Stop);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("frame-source-reap".to_owned())
            .spawn(move || {
                if thread.join().is_err() {
                    tracing::warn!("a frame source worker panicked during shutdown");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the thread that ends a frame source worker");
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("a frame source worker panicked during shutdown");
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
mod fixture;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use fixture::SentFrameInput;
