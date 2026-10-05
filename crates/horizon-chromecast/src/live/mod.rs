//! Live H.264 casting: the host pushes encoded access units, this module
//! serves them as HLS and keeps the Default Media Receiver playing them.
mod h264;
mod hls;
mod http;
mod ts;

pub use h264::avcc_to_annexb;

use crate::{
    CastClient, DEFAULT_MEDIA_RECEIVER, Error, Event, MediaLoad, MediaStatus, Result, StreamType,
    client::NS_CONNECTION, receiver::NS_RECEIVER,
};
use hls::Segmenter;
use http::HttpServer;
use serde_json::Value;
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

const BUFFER_POLL: Duration = Duration::from_millis(50);
const EVENT_POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct LiveOptions {
    /// Target segment length; segments are cut on the first keyframe after it.
    pub segment: Duration,
    /// Completed segments kept for the receiver.
    pub window: usize,
    /// Completed segments required before the receiver is asked to play.
    pub preroll: usize,
    pub title: String,
}

impl Default for LiveOptions {
    fn default() -> Self {
        Self {
            segment: Duration::from_secs(1),
            window: 6,
            preroll: 2,
            title: "Live".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveState {
    Connecting,
    Buffering,
    Playing,
    /// The receiver stopped playing, or another sender took it over.
    Ended,
    Failed(String),
}

pub struct LiveCast {
    segmenter: Arc<Mutex<Segmenter>>,
    state: Arc<Mutex<LiveState>>,
    stop: Arc<AtomicBool>,
    control: Option<JoinHandle<()>>,
    url: String,
    _server: HttpServer,
}

impl LiveCast {
    /// Starts serving the stream and connects to `receiver` in the background.
    /// Follow progress through [`LiveCast::state`].
    /// # Errors
    /// Returns an error if the local HTTP server cannot start.
    pub fn start(receiver: SocketAddr, options: LiveOptions) -> Result<Self> {
        let local = http::route_address(receiver)?;
        let segmenter = Arc::new(Mutex::new(Segmenter::new(options.segment, options.window)));
        let server = HttpServer::start(segmenter.clone())?;
        let url = format!("http://{local}:{}/{}/live.m3u8", server.port(), server.token());
        let state = Arc::new(Mutex::new(LiveState::Connecting));
        let stop = Arc::new(AtomicBool::new(false));
        let control = {
            let session = Session {
                receiver,
                url: url.clone(),
                options,
                segmenter: segmenter.clone(),
                state: state.clone(),
                stop: stop.clone(),
            };
            std::thread::Builder::new()
                .name("chromecast-live".to_owned())
                .spawn(move || session.run())?
        };
        Ok(Self {
            segmenter,
            state,
            stop,
            control: Some(control),
            url,
            _server: server,
        })
    }

    /// Adds one Annex B access unit with its presentation time.
    pub fn push_annexb(&self, annexb: &[u8], pts: Duration, keyframe: bool) {
        lock(&self.segmenter).push(annexb, pts, keyframe);
    }

    /// True when the next frame should be a keyframe so the open segment can close.
    #[must_use]
    pub fn wants_keyframe(&self, pts: Duration) -> bool {
        lock(&self.segmenter).wants_keyframe(pts)
    }

    #[must_use]
    pub fn state(&self) -> LiveState {
        lock(&self.state).clone()
    }

    /// Playlist URL handed to the receiver.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops playback on the receiver and ends the session.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(control) = self.control.take() {
            let _ = control.join();
        }
    }
}

impl Drop for LiveCast {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Session {
    receiver: SocketAddr,
    url: String,
    options: LiveOptions,
    segmenter: Arc<Mutex<Segmenter>>,
    state: Arc<Mutex<LiveState>>,
    stop: Arc<AtomicBool>,
}

impl Session {
    fn run(self) {
        let end = match self.cast() {
            Ok(()) => LiveState::Ended,
            Err(error) => {
                tracing::warn!(%error, "chromecast live session failed");
                LiveState::Failed(error.to_string())
            }
        };
        self.set(end);
    }

    fn cast(&self) -> Result<()> {
        let client = CastClient::connect(self.receiver)?;
        let app = client.launch(DEFAULT_MEDIA_RECEIVER)?;
        self.set(LiveState::Buffering);
        while lock(&self.segmenter).ready_segments() < self.options.preroll {
            if self.stopped() {
                let _ = client.stop_application(&app.session_id);
                return Ok(());
            }
            std::thread::sleep(BUFFER_POLL);
        }
        client.media(&app).load(&MediaLoad {
            url: self.url.clone(),
            content_type: "application/x-mpegurl".to_owned(),
            stream_type: StreamType::Live,
            title: Some(self.options.title.clone()),
            hls_segment_format: Some("ts".to_owned()),
            hls_video_segment_format: Some("mpeg2_ts".to_owned()),
        })?;
        let result = loop {
            if self.stopped() {
                break Ok(());
            }
            let Some(event) = client.next_event(EVENT_POLL)? else {
                continue;
            };
            match self.follow(&event, &app.session_id, &app.transport_id) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => break Err(error),
            }
        };
        // Stopping the application also ends its media session, in one round trip.
        let _ = client.stop_application(&app.session_id);
        result
    }

    /// Applies one receiver event; `Ok(false)` means the session is over.
    fn follow(&self, event: &Event, session_id: &str, transport_id: &str) -> Result<bool> {
        if event.namespace == NS_CONNECTION && event.source == transport_id && event.kind() == Some("CLOSE") {
            return Ok(false);
        }
        if event.namespace == NS_RECEIVER && event.kind() == Some("RECEIVER_STATUS") {
            let ours = event.payload["status"]["applications"]
                .as_array()
                .is_some_and(|apps| apps.iter().any(|app| app["sessionId"] == session_id));
            return Ok(ours);
        }
        for status in MediaStatus::from_event(event) {
            match (status.player_state.as_str(), status.idle_reason.as_deref()) {
                ("PLAYING", _) => self.set(LiveState::Playing),
                ("BUFFERING" | "LOADING", _) => self.set(LiveState::Buffering),
                ("IDLE", Some("ERROR")) => {
                    return Err(Error::Rejected {
                        kind: "MEDIA_ERROR".to_owned(),
                        reason: event.payload.get("status").map(Value::to_string),
                    });
                }
                ("IDLE", Some(_)) => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    fn set(&self, state: LiveState) {
        *lock(&self.state) = state;
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
