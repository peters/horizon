//! Live H.264 casting: the host pushes encoded access units, this module
//! serves them as HLS and keeps the Default Media Receiver playing them.
mod h264;
mod hls;
mod http;
mod ts;

pub use h264::avcc_to_annexb;

use crate::{
    Application, CastClient, DEFAULT_MEDIA_RECEIVER, Error, Event, MediaLoad, MediaStatus, Result, StreamType,
    client::NS_CONNECTION, media::NS_MEDIA, receiver::NS_RECEIVER,
};
use hls::Segmenter;
use http::HttpServer;
use serde_json::Value;
use std::{
    cell::Cell,
    net::SocketAddr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const BUFFER_POLL: Duration = Duration::from_millis(50);
/// Matches no receiver media session: before LOAD only lifecycle events count.
const NO_MEDIA_SESSION: i64 = -1;
const EVENT_POLL: Duration = Duration::from_millis(250);
/// TV receivers report a brief BUFFERING between PLAYING updates every few
/// seconds while playback advances in real time. Only report buffering once a
/// BUFFERING report has gone this long without a PLAYING one after it.
const STALL_GRACE: Duration = Duration::from_secs(5);

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
    /// Half-second segments measured about 3 s glass-to-glass on a Google TV
    /// receiver, against 4 s for one-second segments. The window must cover
    /// that delay plus a refresh, or the receiver asks for segments already gone.
    fn default() -> Self {
        Self {
            segment: Duration::from_millis(500),
            window: 10,
            preroll: 2,
            title: "Live".to_owned(),
        }
    }
}

impl LiveOptions {
    fn validate(&self) -> Result<()> {
        if self.segment.is_zero() {
            return Err(Error::InvalidOptions("segment duration must be positive"));
        }
        if self.preroll == 0 {
            return Err(Error::InvalidOptions("pre-roll must be at least one segment"));
        }
        if self.window < self.preroll {
            return Err(Error::InvalidOptions("window must hold the pre-roll segments"));
        }
        // A live playlist must keep at least three target durations (RFC 8216
        // section 6.2.2); segments can close at 90% of `segment`.
        let kept_ms = self.segment.as_millis() * 9 / 10 * u128::try_from(self.window).unwrap_or(u128::MAX);
        if kept_ms < u128::from(hls::target_seconds(self.segment)) * 3 * 1000 {
            return Err(Error::InvalidOptions(
                "window must hold at least three target durations",
            ));
        }
        Ok(())
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
    /// Returns [`Error::InvalidOptions`] for unusable options, or an error if
    /// the local HTTP server cannot start.
    pub fn start(receiver: SocketAddr, options: LiveOptions) -> Result<Self> {
        options.validate()?;
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
                buffering_since: Cell::new(None),
                started: Cell::new(false),
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

    /// Adds one Annex B access unit with its presentation time. Input must be
    /// in presentation order (no B-frames): units whose timestamp goes
    /// backwards are dropped, since segments carry no separate decode time.
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

pub(crate) struct Session {
    receiver: SocketAddr,
    url: String,
    options: LiveOptions,
    segmenter: Arc<Mutex<Segmenter>>,
    state: Arc<Mutex<LiveState>>,
    stop: Arc<AtomicBool>,
    buffering_since: Cell<Option<Instant>>,
    /// Our media session has reported PLAYING at least once.
    started: Cell<bool>,
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
            if !client.is_open() {
                return Err(Error::Closed);
            }
            // The application can close or be replaced before LOAD; then leave it be.
            if let Some(event) = client.next_event(BUFFER_POLL)?
                && !self.follow_confirmed(&client, &event, &app, NO_MEDIA_SESSION)?
            {
                return Ok(());
            }
        }
        let load = client.media(&app).load(&MediaLoad {
            url: self.url.clone(),
            content_type: "application/x-mpegurl".to_owned(),
            stream_type: StreamType::Live,
            title: Some(self.options.title.clone()),
            hls_segment_format: Some("ts".to_owned()),
            hls_video_segment_format: Some("mpeg2_ts".to_owned()),
        });
        let loaded = match load {
            Ok(status) => status,
            Err(error) => {
                let _ = client.stop_application(&app.session_id);
                return Err(error);
            }
        };
        // The LOAD reply is consumed by its request, so apply its status here.
        let mut outcome = self.apply(&loaded, None);
        let result = loop {
            match outcome {
                Ok(true) => {}
                // The receiver ended playback or was taken over: leave it alone.
                Ok(false) => return Ok(()),
                Err(error) => break Err(error),
            }
            if self.stopped() {
                break Ok(());
            }
            if self.stalled() {
                self.set(LiveState::Buffering);
            }
            outcome = match client.next_event(EVENT_POLL)? {
                Some(event) => self.follow_confirmed(&client, &event, &app, loaded.media_session_id),
                None => Ok(true),
            };
        };
        // Stopping the application also ends its media session, in one round trip.
        let _ = client.stop_application(&app.session_id);
        result
    }

    /// Applies one receiver event; `Ok(false)` means the session is over.
    /// [`Self::follow`], except that a receiver status without our application
    /// is confirmed with a fresh status first: an older notification can still
    /// be queued from before the launch reply.
    fn follow_confirmed(
        &self,
        client: &CastClient,
        event: &Event,
        app: &Application,
        media_session_id: i64,
    ) -> Result<bool> {
        let going_on = self.follow(event, &app.session_id, &app.transport_id, media_session_id)?;
        if going_on {
            return Ok(true);
        }
        if event.namespace == NS_RECEIVER {
            let current = client.receiver_status()?;
            return Ok(current
                .applications
                .iter()
                .any(|running| running.session_id == app.session_id));
        }
        if event.namespace == NS_MEDIA && media_session_id != NO_MEDIA_SESSION {
            // Replies and notifications arrive on separate queues, so a media
            // update can be older than the state already applied: confirm.
            // The reply is not delivered again as an event, so apply it here.
            return match client.media(app).status()? {
                Some(status) if status.media_session_id == media_session_id => self.apply(&status, None),
                _ => Ok(false),
            };
        }
        Ok(false)
    }

    pub(crate) fn follow(
        &self,
        event: &Event,
        session_id: &str,
        transport_id: &str,
        media_session_id: i64,
    ) -> Result<bool> {
        if event.namespace == NS_CONNECTION && event.source == transport_id && event.kind() == Some("CLOSE") {
            return Ok(false);
        }
        if event.namespace == NS_RECEIVER && event.kind() == Some("RECEIVER_STATUS") {
            // Volume-only updates omit `applications`; only a listed set without
            // our session means another sender replaced it.
            let replaced = event.payload["status"]["applications"]
                .as_array()
                .is_some_and(|apps| !apps.iter().any(|app| app["sessionId"] == session_id));
            return Ok(!replaced);
        }
        let statuses = MediaStatus::from_event(event);
        // No media loaded on our transport after playback started: the stream
        // may have been unloaded (confirmed by the caller).
        if media_session_id != NO_MEDIA_SESSION
            && self.started.get()
            && event.source == transport_id
            && event.kind() == Some("MEDIA_STATUS")
            && statuses.is_empty()
        {
            return Ok(false);
        }
        // Media session ids grow per LOAD: an active newer session on our
        // transport means another sender loaded over this cast. It now owns the
        // application, so end without stopping it.
        if media_session_id != NO_MEDIA_SESSION
            && event.source == transport_id
            && statuses
                .iter()
                .any(|status| status.media_session_id > media_session_id && status.player_state != "IDLE")
        {
            return Ok(false);
        }
        // A joined receiver may still report the item our LOAD replaced.
        let ours = statuses
            .into_iter()
            .filter(|status| status.media_session_id == media_session_id);
        let detail = event.payload.get("status").map(Value::to_string);
        for status in ours {
            if !self.apply(&status, detail.clone())? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Applies one status of our media session; `Ok(false)` means playback ended.
    fn apply(&self, status: &MediaStatus, detail: Option<String>) -> Result<bool> {
        match (status.player_state.as_str(), status.idle_reason.as_deref()) {
            ("PLAYING", _) => {
                self.started.set(true);
                self.buffering_since.set(None);
                self.set(LiveState::Playing);
            }
            ("BUFFERING" | "LOADING", _) => {
                if self.buffering_since.get().is_none() {
                    self.buffering_since.set(Some(Instant::now()));
                }
                if *lock(&self.state) != LiveState::Playing {
                    self.set(LiveState::Buffering);
                }
            }
            ("IDLE", Some("ERROR")) => {
                return Err(Error::Rejected {
                    kind: "MEDIA_ERROR".to_owned(),
                    reason: detail.or_else(|| Some("ERROR".to_owned())),
                });
            }
            ("IDLE", Some(_)) => return Ok(false),
            // `idleReason` is optional. A freshly loaded item can report a bare
            // IDLE before it plays; once it has played, a bare IDLE means it ended.
            ("IDLE", None) if self.started.get() => return Ok(false),
            _ => {}
        }
        Ok(true)
    }

    /// Playing, but the receiver has reported buffering for longer than the grace period.
    fn stalled(&self) -> bool {
        *lock(&self.state) == LiveState::Playing
            && self
                .buffering_since
                .get()
                .is_some_and(|since| since.elapsed() >= STALL_GRACE)
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

#[cfg(test)]
pub(crate) fn test_session() -> Session {
    Session {
        receiver: SocketAddr::from(([192, 0, 2, 1], crate::DEFAULT_PORT)),
        url: String::new(),
        options: LiveOptions::default(),
        segmenter: Arc::new(Mutex::new(Segmenter::new(Duration::from_millis(500), 10))),
        state: Arc::new(Mutex::new(LiveState::Buffering)),
        stop: Arc::new(AtomicBool::new(false)),
        buffering_since: Cell::new(None),
        started: Cell::new(false),
    }
}
