//! Live H.264 casting: the host pushes encoded access units, this module
//! serves them on the LAN and keeps the Default Media Receiver playing them,
//! either as one progressive fragmented MP4 stream (low latency) or as HLS.
mod edge;
mod hls;
mod http;
mod mp4;
mod progressive;
#[cfg(feature = "encoder")]
mod sink;
mod ts;

/// Converts a length-prefixed (AVCC) sample to Annex B, putting
/// `parameter_sets` in front of IDR pictures.
/// # Errors
/// Returns [`Error::H264`] for an invalid NAL length size or a truncated NAL unit.
pub fn avcc_to_annexb(sample: &[u8], length_size: usize, parameter_sets: &[&[u8]]) -> Result<Vec<u8>> {
    Ok(horizon_media::h264::avcc_to_annexb(
        sample,
        length_size,
        parameter_sets,
    )?)
}
#[cfg(feature = "encoder")]
pub use sink::LiveCastSink;

use crate::{
    Application, CastClient, DEFAULT_MEDIA_RECEIVER, Error, Event, MediaController, MediaLoad, MediaStatus, Result,
    StreamType,
    client::{NS_CONNECTION, REQUEST_TIMEOUT},
    media::NS_MEDIA,
    receiver::NS_RECEIVER,
};
use hls::Segmenter;
use http::{HttpServer, Source};
use progressive::Stream;
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
pub(crate) const STALL_GRACE: Duration = Duration::from_secs(5);
/// How often the live-edge controller looks at the receiver while it plays.
const EDGE_POLL: Duration = Duration::from_millis(400);
/// Wall time kept for sending normal speed after a status poll while playback
/// is already fast. A lost reply must not spend this as well.
const RESTORE_BUDGET: Duration = Duration::from_millis(750);
/// Shortest status poll while playback is fast. Below this, a slow receiver
/// and a lost reply look the same, so a thinner buffer restores at once.
const MIN_STATUS_WAIT: Duration = Duration::from_millis(250);
/// Attempts to return to normal speed before the session fails, so a receiver
/// is never left running fast.
const RESTORE_ATTEMPTS: u8 = 3;
const RESTORE_RETRY: Duration = Duration::from_millis(500);
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_RETRY: Duration = Duration::from_secs(2);

/// How the stream reaches the receiver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    /// One endless fragmented MP4 response. The player starts further behind
    /// the newest frame; playback speeds up until it is about 0.4 s behind,
    /// then closer, for as long as the picture keeps moving.
    #[default]
    Progressive,
    /// Rolling HLS of MPEG-TS segments; about three seconds behind live.
    Hls,
}

/// An AAC-LC audio track: up to 48 kHz, mono or stereo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Clone, Debug)]
pub struct LiveOptions {
    pub transport: Transport,
    /// Declares an AAC-LC track fed through [`LiveCast::push_aac`]
    /// (progressive only). Once declared, feed audio continuously, silence
    /// included: receivers wait for both tracks before they play.
    pub audio: Option<AudioFormat>,
    /// Target segment length; segments are cut on the first keyframe after it.
    /// Progressive streams ask for a keyframe this often so receivers can join.
    pub segment: Duration,
    /// Completed segments kept for the receiver (HLS only).
    pub window: usize,
    /// Completed segments required before the receiver is asked to play (HLS only).
    pub preroll: usize,
    pub title: String,
}

impl Default for LiveOptions {
    /// Half-second segments measured about 3 s glass-to-glass on a Google TV
    /// receiver, against 4 s for one-second segments. The window must cover
    /// that delay plus a refresh, or the receiver asks for segments already gone.
    fn default() -> Self {
        Self {
            transport: Transport::default(),
            audio: None,
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
        if let Some(format) = self.audio {
            if self.transport != Transport::Progressive {
                return Err(Error::InvalidOptions("audio needs the progressive transport"));
            }
            if mp4::audio_specific_config(format).is_none() {
                return Err(Error::InvalidOptions(
                    "audio must be AAC-LC at a standard rate up to 48 kHz, mono or stereo",
                ));
            }
        }
        // The progressive stream has no playlist: window and pre-roll are HLS only.
        if self.transport == Transport::Progressive {
            return Ok(());
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

/// Where pushed access units go.
#[derive(Clone)]
enum Sink {
    Hls(Arc<Mutex<Segmenter>>),
    Progressive(Arc<Stream>),
}

impl Sink {
    fn new(options: &LiveOptions) -> Self {
        match options.transport {
            Transport::Hls => Self::Hls(Arc::new(Mutex::new(Segmenter::new(options.segment, options.window)))),
            Transport::Progressive => Self::Progressive(Arc::new(Stream::new(options.segment, options.audio))),
        }
    }

    fn source(&self) -> Source {
        match self {
            Self::Hls(segmenter) => Source::Hls(segmenter.clone()),
            Self::Progressive(stream) => Source::Progressive(stream.clone()),
        }
    }

    fn file(&self) -> &'static str {
        match self {
            Self::Hls(_) => "live.m3u8",
            Self::Progressive(_) => "live.mp4",
        }
    }
}

pub struct LiveCast {
    sink: Sink,
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
        let sink = Sink::new(&options);
        let server = HttpServer::start(sink.source())?;
        let url = format!("http://{local}:{}/{}/{}", server.port(), server.token(), sink.file());
        let state = Arc::new(Mutex::new(LiveState::Connecting));
        let stop = Arc::new(AtomicBool::new(false));
        let control = {
            let session = Session {
                receiver,
                url: url.clone(),
                options,
                sink: sink.clone(),
                state: state.clone(),
                stop: stop.clone(),
                buffering_since: Cell::new(None),
                started: Cell::new(false),
                playback: Cell::new(Playback::new()),
            };
            std::thread::Builder::new()
                .name("chromecast-live".to_owned())
                .spawn(move || session.run())?
        };
        Ok(Self {
            sink,
            state,
            stop,
            control: Some(control),
            url,
            _server: server,
        })
    }

    /// Adds one raw AAC-LC frame (no ADTS header) for the audio track declared
    /// in [`LiveOptions::audio`], timed on the same clock as the video.
    /// Ignored without a declared audio track, and before the first video unit.
    pub fn push_aac(&self, frame: &[u8], pts: Duration) {
        if let Sink::Progressive(stream) = &self.sink {
            stream.push_audio(frame, pts);
        }
    }

    /// Adds one Annex B access unit with its presentation time. Input must be
    /// in presentation order (no B-frames): units whose timestamp goes
    /// backwards are dropped, since segments carry no separate decode time.
    pub fn push_annexb(&self, annexb: &[u8], pts: Duration, keyframe: bool) {
        match &self.sink {
            Sink::Hls(segmenter) => lock(segmenter).push(annexb, pts, keyframe),
            Sink::Progressive(stream) => stream.push(annexb, pts, keyframe),
        }
    }

    /// True when the next frame should be a keyframe so the open segment can close.
    #[must_use]
    pub fn wants_keyframe(&self, pts: Duration) -> bool {
        match &self.sink {
            Sink::Hls(segmenter) => lock(segmenter).wants_keyframe(pts),
            Sink::Progressive(stream) => stream.wants_keyframe(pts),
        }
    }

    #[must_use]
    pub fn state(&self) -> LiveState {
        lock(&self.state).clone()
    }

    /// Stream URL handed to the receiver.
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
        if let Sink::Progressive(stream) = &self.sink {
            stream.close();
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
    sink: Sink,
    state: Arc<Mutex<LiveState>>,
    stop: Arc<AtomicBool>,
    buffering_since: Cell<Option<Instant>>,
    /// Our media session has reported PLAYING at least once.
    started: Cell<bool>,
    playback: Cell<Playback>,
}

/// Live-edge controller plus a rate command that still has to be confirmed.
#[derive(Clone, Copy, Debug)]
struct Playback {
    edge: edge::Edge,
    unsupported: bool,
    retry: Option<Retry>,
    next_check: Instant,
    checked: Instant,
    /// `checked` is a real playback sample, not session start or a gap.
    sampled: bool,
    /// Last finite lag. Bounds the next status poll while playback is fast.
    lag: Option<f64>,
}

/// A playback-rate command to send again at `at`. `failures` counts failed
/// returns to normal speed.
#[derive(Clone, Copy, Debug)]
struct Retry {
    at: Instant,
    rate: f64,
    failures: u8,
}

impl Playback {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            edge: edge::Edge::default(),
            unsupported: false,
            retry: None,
            next_check: now,
            checked: now,
            sampled: false,
            lag: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CatchUp {
    Watching,
    /// Playing fast until this instant; then returning to normal speed, with
    /// the failed attempts so far.
    Until(Instant, u8),
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
        let Some(client) = self.connect()? else {
            return Ok(());
        };
        let app = client.launch(DEFAULT_MEDIA_RECEIVER)?;
        self.set(LiveState::Buffering);
        while !self.ready_to_load() {
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
        let mut media = client.media(&app);
        let load = media.load(&self.media_load());
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
            outcome = if self.stalled() {
                self.check_stall(&client, &app, loaded.media_session_id)
            } else {
                Ok(true)
            };
            if !matches!(outcome, Ok(true)) {
                continue;
            }
            if let Err(error) = self.keep_up(&media) {
                break Err(error);
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
        let going_on = match self.follow(event, &app.session_id, &app.transport_id, media_session_id) {
            Ok(going_on) => going_on,
            // A queued error may predate a replacement load: confirm it too.
            Err(_) if event.namespace == NS_MEDIA && media_session_id != NO_MEDIA_SESSION => false,
            Err(error) => return Err(error),
        };
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

    /// Before reporting a stall, applies a fresh status: the BUFFERING that
    /// started the timer may have been queued behind a newer PLAYING reply.
    fn check_stall(&self, client: &CastClient, app: &Application, media_session_id: i64) -> Result<bool> {
        let going_on = match client.media(app).status()? {
            Some(status) if status.media_session_id == media_session_id => self.apply(&status, None)?,
            _ => false,
        };
        if going_on && self.stalled() {
            self.set(LiveState::Buffering);
        }
        Ok(going_on)
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

    /// A rate command's own status. Correlated replies are not replayed as events.
    fn absorb_rate_status(&self, playback: &mut Playback, status: &MediaStatus, observed: Instant) {
        match status.player_state.as_str() {
            "PLAYING" => {
                if let Some(since) = self.buffering_since.take() {
                    close_buffer(&mut playback.edge, observed.saturating_duration_since(since));
                    playback.sampled = false;
                } else {
                    playback.edge.resume();
                }
            }
            "BUFFERING" | "LOADING" => {
                if self.buffering_since.get().is_none() {
                    self.buffering_since.set(Some(observed));
                    if let Some(lag) = self.open_lag(status.current_time) {
                        playback.edge.note_buffer(lag);
                    }
                    playback.edge.settled_for = Duration::ZERO;
                    playback.sampled = false;
                }
            }
            "PAUSED" => {
                // This reply is not delivered again as an event. Judge the
                // buffer here, or the pause drops its duration.
                let lasted = self
                    .buffering_since
                    .take()
                    .map(|since| observed.saturating_duration_since(since));
                close_ended_buffer(&mut playback.edge, "PAUSED", lasted);
                playback.sampled = false;
                playback.edge.settled_for = Duration::ZERO;
            }
            _ => {
                self.buffering_since.set(None);
            }
        }
    }

    fn open_lag(&self, current_time: f64) -> Option<f64> {
        let Sink::Progressive(stream) = &self.sink else {
            return None;
        };
        note_open_lag(stream.newest_media_time(), current_time)
    }

    /// Records the lag at the start of a progressive buffer. Events arrive
    /// before the next poll, and by then new frames have already increased it.
    fn note_buffer_lag(&self, current_time: f64) {
        let Sink::Progressive(stream) = &self.sink else {
            return;
        };
        let Some(lag) = note_open_lag(stream.newest_media_time(), current_time) else {
            return;
        };
        let mut playback = self.playback.get();
        playback.edge.note_buffer(lag);
        self.playback.set(playback);
    }

    /// Applies one status of our media session; `Ok(false)` means playback ended.
    fn apply(&self, status: &MediaStatus, detail: Option<String>) -> Result<bool> {
        match (status.player_state.as_str(), status.idle_reason.as_deref()) {
            ("PLAYING", _) => {
                self.started.set(true);
                let lasted = self.buffering_since.take().map(|since| since.elapsed());
                self.set(LiveState::Playing);
                let mut playback = self.playback.get();
                if let Some(lasted) = lasted {
                    close_buffer(&mut playback.edge, lasted);
                    playback.sampled = false;
                } else {
                    playback.edge.resume();
                }
                self.playback.set(playback);
            }
            ("BUFFERING" | "LOADING", _) => {
                if self.buffering_since.get().is_none() {
                    self.buffering_since.set(Some(Instant::now()));
                    self.note_buffer_lag(status.current_time);
                    let mut playback = self.playback.get();
                    playback.edge.settled_for = Duration::ZERO;
                    playback.sampled = false;
                    self.playback.set(playback);
                }
                if *lock(&self.state) != LiveState::Playing {
                    self.set(LiveState::Buffering);
                }
            }
            ("PAUSED", _) => {
                // The pause is not part of the buffer. Judge what was already
                // open, then drop the calm clock so the pause cannot extend it.
                let lasted = self.buffering_since.take().map(|since| since.elapsed());
                let mut playback = self.playback.get();
                if let Some(lasted) = lasted {
                    close_buffer(&mut playback.edge, lasted);
                }
                playback.sampled = false;
                playback.edge.settled_for = Duration::ZERO;
                self.playback.set(playback);
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

    /// The first connection can fail while the host OS asks for local network
    /// access, or while a TV wakes its network; retry I/O failures briefly.
    /// `None` when the cast was stopped while waiting to retry.
    fn connect(&self) -> Result<Option<CastClient>> {
        let mut attempt = 1;
        loop {
            match CastClient::connect(self.receiver) {
                Err(Error::Io(error)) if attempt < CONNECT_ATTEMPTS && !self.stopped() => {
                    tracing::debug!(%error, attempt, "retrying the receiver connection");
                    std::thread::sleep(CONNECT_RETRY);
                    if self.stopped() {
                        return Ok(None);
                    }
                    attempt += 1;
                }
                result => return result.map(Some),
            }
        }
    }

    fn ready_to_load(&self) -> bool {
        match &self.sink {
            Sink::Hls(segmenter) => lock(segmenter).ready_segments() >= self.options.preroll,
            Sink::Progressive(stream) => stream.ready(),
        }
    }

    fn media_load(&self) -> MediaLoad {
        let hls = matches!(self.sink, Sink::Hls(_));
        MediaLoad {
            url: self.url.clone(),
            content_type: if hls { "application/x-mpegurl" } else { "video/mp4" }.to_owned(),
            stream_type: StreamType::Live,
            title: Some(self.options.title.clone()),
            hls_segment_format: hls.then(|| "ts".to_owned()),
            hls_video_segment_format: hls.then(|| "mpeg2_ts".to_owned()),
        }
    }

    /// Progressive receivers start with a few seconds of buffer. Measure how
    /// far playback trails the newest frame and play faster until that lag is
    /// back at the live-edge target.
    /// # Errors
    /// Fails when playback cannot be returned to normal speed.
    fn keep_up(&self, media: &MediaController<'_>) -> Result<()> {
        let Sink::Progressive(stream) = &self.sink else {
            return Ok(());
        };
        let mut playback = self.playback.get();
        let now = Instant::now();
        // A restore still has to land after a faster rate was refused.
        if let Some(retry) = playback.retry {
            playback.sampled = false;
            if now < retry.at {
                self.playback.set(playback);
                return Ok(());
            }
            return self.push_rate(media, playback, retry.rate, retry.failures, now);
        }
        if playback.unsupported {
            return Ok(());
        }
        if !self.started.get() || *lock(&self.state) != LiveState::Playing {
            // A stall report stops the poll. A faster rate still has to come down,
            // or the receiver keeps eating the buffer with no restore deadline.
            playback.sampled = false;
            if self.started.get() && playback.edge.rate > 1.0 + 1e-3 {
                return self.push_rate(media, playback, 1.0, 0, now);
            }
            self.playback.set(playback);
            return Ok(());
        }
        if now < playback.next_check {
            return Ok(());
        }
        playback.next_check = now + EDGE_POLL;
        // GET_STATUS blocks this loop, so a lost reply cannot be told from a
        // slow one until the timeout. While playback is already fast, stop
        // waiting in time to send normal speed. A buffer thinner than that
        // restores without asking.
        let Some(wait) = status_wait(playback.edge.rate, playback.lag) else {
            playback.sampled = false;
            return self.push_rate(media, playback, 1.0, 0, now);
        };
        // Read the source position after the status call returns. A newest
        // timestamp from before the call compared with the later receiver clock
        // makes the lag look smaller than it is.
        let status = media.status_within(wait);
        let newest = stream.newest_media_time();
        let (Some(newest), Ok(Some(status))) = (newest, status) else {
            // The receiver may already be playing fast. Without a sample there
            // is no signal to stop, so return to normal speed.
            playback.sampled = false;
            if playback.edge.rate > 1.0 + 1e-3 {
                return self.push_rate(media, playback, 1.0, 0, now);
            }
            self.playback.set(playback);
            return Ok(());
        };
        let lag = newest - status.current_time;
        if lag.is_finite() {
            playback.lag = Some(lag);
        }
        // The status call blocks. Time the buffer from when the reply arrived,
        // not from when the request was sent, or a slow reply looks like a stall.
        let observed = Instant::now();
        let lasted = self
            .buffering_since
            .get()
            .map(|since| observed.saturating_duration_since(since));
        let mut open = self.buffering_since.get();
        let buffering_for = buffering_age(&mut open, status.player_state.as_str(), observed);
        self.buffering_since.set(open);
        let playing = status.player_state == "PLAYING";
        // A polled pause is not playing, and `buffering_age` has already
        // cleared the timer. Judge the episode before the pause discards it.
        close_ended_buffer(&mut playback.edge, status.player_state.as_str(), lasted);
        // Calm time runs between replies. The wait before the first playing
        // sample is not calm, and a pause or a buffer must not be either.
        let elapsed = note_sample(&mut playback, playing, observed);
        let plan = plan_trim(
            playback.edge,
            edge::Sample {
                lag,
                buffering_for,
                elapsed,
            },
            playing,
        );
        playback.edge = plan.edge;
        let Some(rate) = plan.command else {
            self.playback.set(playback);
            return Ok(());
        };
        tracing::debug!(lag, rate, target = plan.edge.target, "trimming live playback");
        self.push_rate(media, playback, rate, 0, now)
    }

    /// Applies `rate`. An explicit refusal of a faster rate stops further
    /// trimming. A rate the receiver may already be using is not recorded as
    /// 1.0 until a restore is confirmed. A lost reply while speeding up is
    /// treated as if the rate applied, so the next command restores normal speed.
    /// # Errors
    /// Fails when playback cannot be returned to normal speed.
    fn push_rate(
        &self,
        media: &MediaController<'_>,
        mut playback: Playback,
        rate: f64,
        failures: u8,
        now: Instant,
    ) -> Result<()> {
        let wait = rate_wait(playback.edge.rate, rate);
        let (answer, error) = match media.set_playback_rate_within(rate, wait) {
            Ok(status) => {
                // The reply is not delivered again as an event. A PLAYING
                // status here has to close a short buffer before the next poll.
                self.absorb_rate_status(&mut playback, &status, Instant::now());
                (RateAnswer::Applied, None)
            }
            Err(error @ Error::Rejected { .. }) => (RateAnswer::Refused, Some(error)),
            Err(error) => (RateAnswer::Unconfirmed, Some(error)),
        };
        if apply_rate_reply(&mut playback, rate, failures, answer, now) {
            self.playback.set(playback);
            return Err(error.unwrap_or(Error::Protocol("playback rate was not confirmed")));
        }
        self.playback.set(playback);
        Ok(())
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

/// What the receiver did with a `SET_PLAYBACK_RATE` command.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RateAnswer {
    Applied,
    /// The receiver explicitly refused the command, so it kept the old rate.
    Refused,
    /// The reply was lost. The command may or may not have applied.
    Unconfirmed,
}

/// Elapsed calm time. The clock starts at the first real sample, so launch
/// and LOAD do not count, and a gap without a sample does not count either.
fn calm_elapsed(playback: &Playback, now: Instant) -> Duration {
    if playback.sampled {
        now.saturating_duration_since(playback.checked)
    } else {
        Duration::ZERO
    }
}

/// Records one observed sample and returns the calm time since the previous
/// one. `observed` is when the reply arrived, not when the request was sent.
fn note_sample(playback: &mut Playback, playing: bool, observed: Instant) -> Duration {
    let elapsed = calm_elapsed(playback, observed);
    if playing {
        playback.checked = observed;
    }
    playback.sampled = playing;
    elapsed
}

/// How long a status poll may block. `None` means the buffer can run out
/// before a poll and a restore would both finish, so restore immediately.
fn status_wait(rate: f64, lag: Option<f64>) -> Option<Duration> {
    if rate <= 1.0 + 1e-3 {
        return Some(REQUEST_TIMEOUT);
    }
    let Some(lag) = lag.filter(|lag| lag.is_finite()) else {
        return Some(MIN_STATUS_WAIT);
    };
    if lag <= 0.0 {
        return None;
    }
    let excess = rate - 1.0;
    let secs = lag / excess;
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    // Longer than the ordinary request timeout: the cap below is enough, and
    // `Duration` cannot represent an unbounded lag.
    if secs >= REQUEST_TIMEOUT.as_secs_f64() + RESTORE_BUDGET.as_secs_f64() {
        return Some(REQUEST_TIMEOUT);
    }
    let remaining = Duration::from_secs_f64(secs);
    if remaining <= RESTORE_BUDGET.saturating_add(MIN_STATUS_WAIT) {
        return None;
    }
    Some(remaining.saturating_sub(RESTORE_BUDGET).min(REQUEST_TIMEOUT))
}

/// A return to normal speed, while a faster rate is already applied, cannot
/// use the ordinary request timeout. The buffer would be gone first.
fn rate_wait(applied: f64, commanded: f64) -> Duration {
    let restoring = (commanded - 1.0).abs() <= 1e-3;
    if applied > 1.0 + 1e-3 && restoring {
        RESTORE_BUDGET
    } else {
        REQUEST_TIMEOUT
    }
}

/// How long the polled player has been buffering. Any other state closes the
/// timer, so a pause cannot be counted as part of the next buffer.
fn buffering_age(open_since: &mut Option<Instant>, player_state: &str, now: Instant) -> Option<Duration> {
    match player_state {
        "BUFFERING" | "LOADING" => {
            let start = *open_since.get_or_insert(now);
            Some(now.saturating_duration_since(start))
        }
        _ => {
            *open_since = None;
            None
        }
    }
}

/// Lag when a progressive buffer opens. `None` for other transports, or before
/// the first frame.
fn note_open_lag(newest: Option<f64>, current_time: f64) -> Option<f64> {
    newest.map(|newest| newest - current_time)
}

/// The buffer ended. Judge it, drop the calm time it interrupted, then allow
/// the next episode. The confirmed playback rate is left alone.
fn close_buffer(edge: &mut edge::Edge, lasted: Duration) {
    edge.finish_episode(lasted);
    edge.settled_for = Duration::ZERO;
    edge.resume();
}

/// Judge a buffer that ended by playing or pausing. An open buffering sample
/// is left alone so a later sample can see the whole episode.
fn close_ended_buffer(edge: &mut edge::Edge, player_state: &str, lasted: Option<Duration>) {
    if matches!(player_state, "PLAYING" | "PAUSED")
        && let Some(lasted) = lasted
    {
        close_buffer(edge, lasted);
    }
}

/// What to ask the receiver, with the rate we already believe is applied.
struct Trim {
    edge: edge::Edge,
    /// `None` when no command should be sent. The edge rate stays at the last
    /// confirmed rate until a reply says otherwise.
    command: Option<f64>,
}

/// Plans the next rate command. The requested rate is not stored as applied:
/// a refusal must still see the rate the receiver was already using.
fn plan_trim(edge: edge::Edge, sample: edge::Sample, playing: bool) -> Trim {
    let previous = edge.rate;
    let held = edge;
    let mut next = edge.step(sample);
    // A pause is neither playback nor a buffer. Counting it as calm would
    // lower the target. Buffering still steps back, and a fast rate is restored.
    if !playing && sample.buffering_for.is_none() {
        next.target = held.target;
        next.floor = held.floor;
        next.settled_for = Duration::ZERO;
        next.backed_off = held.backed_off;
        next.episode_lag = held.episode_lag;
    }
    let mut requested = next.rate;
    // An open buffer must not keep a faster rate. The stall state stops the
    // poll entirely, and this covers the reports before that.
    if !playing && previous > 1.0 + 1e-3 {
        requested = 1.0;
    }
    next.rate = previous;
    let speed_up = requested > previous + 1e-3;
    let changed = (requested - previous).abs() > 1e-3;
    // Do not speed up into an open buffer. Do send the return to normal speed.
    let command = if changed && (playing || !speed_up) {
        Some(requested)
    } else {
        None
    };
    Trim { edge: next, command }
}

/// Records `answer` to `commanded`. Returns whether the session must fail
/// because normal speed could not be restored.
fn apply_rate_reply(playback: &mut Playback, commanded: f64, failures: u8, answer: RateAnswer, now: Instant) -> bool {
    let restoring = (commanded - 1.0).abs() <= 1e-3;
    match answer {
        RateAnswer::Applied => {
            playback.edge.rate = commanded;
            playback.retry = None;
            // The reply can take long enough to look like a calm stretch.
            if (commanded - 1.0).abs() <= 1e-3 {
                playback.sampled = false;
            }
            false
        }
        // The previous faster rate is still applied. Ask for 1.0 and stop
        // trying to go faster. Record 1.0 only when that restore is confirmed.
        RateAnswer::Refused if !restoring => {
            playback.unsupported = true;
            if playback.edge.rate > 1.0 + 1e-3 {
                playback.retry = Some(Retry {
                    at: now,
                    rate: 1.0,
                    failures: 0,
                });
            } else {
                playback.edge.rate = 1.0;
                playback.retry = None;
            }
            false
        }
        // The faster rate may have applied before the reply was lost.
        RateAnswer::Unconfirmed if !restoring => {
            playback.retry = Some(Retry {
                at: now,
                rate: 1.0,
                failures: 0,
            });
            false
        }
        _ => match after_restore(false, failures, now) {
            Some(CatchUp::Until(at, failures)) => {
                playback.retry = Some(Retry {
                    at,
                    rate: 1.0,
                    failures,
                });
                false
            }
            Some(CatchUp::Watching) => {
                playback.edge.rate = 1.0;
                playback.retry = None;
                false
            }
            None => true,
        },
    }
}

/// What follows an attempt to return to normal speed: watching again, another
/// attempt shortly, or `None` when the session must fail.
fn after_restore(restored: bool, failures: u8, now: Instant) -> Option<CatchUp> {
    if restored {
        Some(CatchUp::Watching)
    } else if failures + 1 >= RESTORE_ATTEMPTS {
        None
    } else {
        Some(CatchUp::Until(now + RESTORE_RETRY, failures + 1))
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
        sink: Sink::Hls(Arc::new(Mutex::new(Segmenter::new(Duration::from_millis(500), 10)))),
        state: Arc::new(Mutex::new(LiveState::Buffering)),
        stop: Arc::new(AtomicBool::new(false)),
        buffering_since: Cell::new(None),
        started: Cell::new(false),
        playback: Cell::new(Playback::new()),
    }
}

#[cfg(test)]
mod catch_up_tests {
    use super::*;

    #[test]
    fn a_failed_return_to_normal_speed_is_retried_and_then_fails_the_session() {
        let now = Instant::now();
        assert_eq!(after_restore(true, 2, now), Some(CatchUp::Watching));
        assert_eq!(
            after_restore(false, 0, now),
            Some(CatchUp::Until(now + RESTORE_RETRY, 1))
        );
        assert_eq!(
            after_restore(false, 1, now),
            Some(CatchUp::Until(now + RESTORE_RETRY, 2))
        );
        assert_eq!(after_restore(false, RESTORE_ATTEMPTS - 1, now), None);
    }

    #[test]
    fn a_refused_slower_trim_restores_the_rate_already_applied() {
        let now = Instant::now();
        let mut playback = Playback::new();
        playback.edge.rate = 1.5;
        assert!(!apply_rate_reply(&mut playback, 1.08, 0, RateAnswer::Refused, now));
        assert!(playback.unsupported);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);
        let retry = playback.retry.expect("restore scheduled");
        assert!((retry.rate - 1.0).abs() < 1e-9);
        assert_eq!(retry.failures, 0);

        assert!(!apply_rate_reply(
            &mut playback,
            1.0,
            retry.failures,
            RateAnswer::Applied,
            now
        ));
        assert!(playback.unsupported);
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!(playback.retry.is_none());
    }

    #[test]
    fn a_refused_first_speed_up_does_not_invent_a_restore() {
        let now = Instant::now();
        let mut playback = Playback::new();
        assert!(!apply_rate_reply(&mut playback, 1.08, 0, RateAnswer::Refused, now));
        assert!(playback.unsupported);
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!(playback.retry.is_none());
    }

    #[test]
    fn a_lost_faster_command_is_followed_by_a_restore() {
        let now = Instant::now();
        let mut playback = Playback::new();
        assert!(!apply_rate_reply(&mut playback, 1.5, 0, RateAnswer::Unconfirmed, now));
        let retry = playback.retry.expect("restore scheduled");
        assert!((retry.rate - 1.0).abs() < 1e-9);
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_first_calm_sample_ignores_time_before_playback() {
        let mut playback = Playback::new();
        playback.checked = Instant::now()
            .checked_sub(Duration::from_secs(30))
            .expect("instant in range");
        let now = Instant::now();
        assert_eq!(calm_elapsed(&playback, now), Duration::ZERO);
        playback.sampled = true;
        assert!(calm_elapsed(&playback, now) >= Duration::from_secs(29));
    }

    #[test]
    fn a_polled_buffer_starts_and_a_playing_status_closes_it() {
        let start = Instant::now();
        let mut open = None;
        assert_eq!(buffering_age(&mut open, "BUFFERING", start), Some(Duration::ZERO));
        let later = start + Duration::from_millis(600);
        assert_eq!(
            buffering_age(&mut open, "BUFFERING", later),
            Some(Duration::from_millis(600))
        );
        assert_eq!(buffering_age(&mut open, "PLAYING", later), None);
        assert!(open.is_none());
        assert_eq!(buffering_age(&mut open, "LOADING", later), Some(Duration::ZERO));
        let paused = later + Duration::from_secs(5);
        assert_eq!(buffering_age(&mut open, "PAUSED", paused), None);
        assert!(open.is_none());
        assert_eq!(buffering_age(&mut open, "BUFFERING", paused), Some(Duration::ZERO));
    }

    #[test]
    fn a_slow_status_reply_does_not_count_the_wait_as_buffering() {
        let sent = Instant::now();
        let seen = sent + Duration::from_millis(600);
        let mut open = None;
        assert_eq!(buffering_age(&mut open, "BUFFERING", seen), Some(Duration::ZERO));
        let playing = seen + Duration::from_millis(50);
        let lasted = playing.saturating_duration_since(open.expect("timer started when seen"));
        assert!(lasted < Duration::from_millis(500));
        let mut edge = edge::Edge {
            target: 0.25,
            floor: 0.25,
            rate: 1.08,
            episode_lag: Some(0.30),
            ..edge::Edge::default()
        };
        close_buffer(&mut edge, lasted);
        assert!((edge.target - 0.25).abs() < 1e-9);
        assert!((edge.rate - 1.08).abs() < 1e-9);
    }

    #[test]
    fn a_restore_reply_that_is_already_playing_closes_a_short_buffer() {
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.25;
        playback.edge.rate = 1.5;
        playback.edge.note_buffer(0.30);
        let playing = MediaStatus {
            media_session_id: 1,
            player_state: "PLAYING".to_owned(),
            idle_reason: None,
            current_time: 1.0,
        };
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(450))
                .expect("test clock"),
        ));
        session.absorb_rate_status(&mut playback, &playing, Instant::now());
        assert!(session.buffering_since.get().is_none());
        assert!((playback.edge.target - 0.25).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);

        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(650))
                .expect("test clock"),
        ));
        playback.edge.note_buffer(0.30);
        session.absorb_rate_status(&mut playback, &playing, Instant::now());
        assert!((playback.edge.target - 0.30).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_playing_event_opens_a_new_buffer_episode() {
        let mut edge = edge::Edge {
            target: 0.25,
            floor: 0.25,
            backed_off: true,
            episode_lag: Some(0.30),
            ..edge::Edge::default()
        };
        edge.resume();
        assert!(!edge.backed_off);
        assert!(edge.episode_lag.is_none());
        edge.note_buffer(0.50);
        let grown = edge.step(edge::Sample {
            lag: 0.95,
            buffering_for: Some(Duration::from_millis(500)),
            elapsed: Duration::from_millis(400),
        });
        assert!((grown.target - 0.30).abs() < 1e-9);
        let missed = edge::Edge {
            target: 0.25,
            floor: 0.25,
            ..edge::Edge::default()
        };
        let late = missed.step(edge::Sample {
            lag: 0.95,
            buffering_for: Some(Duration::from_millis(500)),
            elapsed: Duration::from_millis(400),
        });
        assert!((late.target - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_restored_normal_speed_does_not_keep_the_calm_clock() {
        let now = Instant::now();
        let mut playback = Playback::new();
        playback.sampled = true;
        playback.edge.rate = 1.5;
        assert!(!apply_rate_reply(&mut playback, 1.0, 0, RateAnswer::Applied, now));
        assert!(!playback.sampled);
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn time_stopped_does_not_start_the_calm_clock() {
        let mut playback = Playback::new();
        let then = Instant::now();
        playback.checked = then;
        playback.sampled = false;
        let later = then + Duration::from_secs(4);
        assert_eq!(calm_elapsed(&playback, later), Duration::ZERO);
        playback.sampled = true;
        playback.checked = later;
        let after = later + Duration::from_millis(400);
        let elapsed = calm_elapsed(&playback, after);
        assert!(elapsed >= Duration::from_millis(400));
        assert!(elapsed < Duration::from_secs(2));
    }

    #[test]
    fn the_wait_before_the_first_reply_is_not_calm() {
        let mut playback = Playback::new();
        let sent = Instant::now();
        let reply = sent + Duration::from_secs(4);
        assert_eq!(note_sample(&mut playback, true, reply), Duration::ZERO);
        assert!(playback.sampled);
        let next = reply + Duration::from_millis(400);
        let elapsed = note_sample(&mut playback, true, next);
        assert!(elapsed >= Duration::from_millis(400));
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn a_fast_status_poll_leaves_time_to_restore() {
        // 0.21 s of lag at 1.08× is gone in about 2.6 s. The poll must end
        // with the restore budget still unused.
        let wait = status_wait(1.08, Some(0.21)).expect("poll");
        let drain = Duration::from_secs_f64(0.21 / 0.08);
        assert!(wait < REQUEST_TIMEOUT);
        assert!(wait.saturating_add(RESTORE_BUDGET) <= drain);
        assert_eq!(status_wait(1.0, Some(0.40)), Some(REQUEST_TIMEOUT));
        assert_eq!(status_wait(1.08, None), Some(MIN_STATUS_WAIT));
        assert!(status_wait(1.5, Some(0.20)).is_none());
        assert_eq!(rate_wait(1.08, 1.0), RESTORE_BUDGET);
        assert_eq!(rate_wait(1.0, 1.08), REQUEST_TIMEOUT);
    }

    #[test]
    fn a_buffer_that_ends_between_polls_still_steps_back() {
        let mut edge = edge::Edge {
            target: 0.25,
            floor: 0.25,
            rate: 1.08,
            ..edge::Edge::default()
        };
        edge.note_buffer(0.35);
        let during = edge.step(edge::Sample {
            lag: 0.40,
            buffering_for: Some(Duration::from_millis(300)),
            elapsed: Duration::from_millis(400),
        });
        assert!((during.target - 0.25).abs() < 1e-9);
        close_buffer(&mut edge, Duration::from_millis(600));
        assert!((edge.target - 0.30).abs() < 1e-9);
        assert!((edge.floor - 0.30).abs() < 1e-9);
        assert!((edge.rate - 1.08).abs() < 1e-9);
        assert_eq!(edge.settled_for, Duration::ZERO);
        assert!(edge.episode_lag.is_none());
    }

    #[test]
    fn a_short_interruption_does_not_spend_the_calm_time() {
        let edge = edge::Edge::default();
        let next = plan_trim(
            edge,
            edge::Sample {
                lag: 0.41,
                buffering_for: None,
                elapsed: Duration::from_millis(400),
            },
            true,
        );
        assert!((next.edge.target - 0.40).abs() < 1e-9);
    }

    #[test]
    fn a_buffer_that_already_stepped_back_does_not_step_again_when_it_ends() {
        let mut edge = edge::Edge {
            target: 0.30,
            floor: 0.30,
            backed_off: true,
            episode_lag: Some(0.35),
            ..edge::Edge::default()
        };
        close_buffer(&mut edge, Duration::from_millis(800));
        assert!((edge.target - 0.30).abs() < 1e-9);
    }

    #[test]
    fn a_refused_first_speed_up_is_judged_from_the_rate_already_applied() {
        let now = Instant::now();
        let plan = plan_trim(
            edge::Edge::default(),
            edge::Sample {
                lag: 0.65,
                buffering_for: None,
                elapsed: Duration::from_millis(400),
            },
            true,
        );
        assert!((plan.edge.rate - 1.0).abs() < 1e-9);
        let rate = plan.command.expect("speed up");
        assert!((rate - 1.08).abs() < 1e-9);
        let mut playback = Playback::new();
        playback.edge = plan.edge;
        assert!(!apply_rate_reply(&mut playback, rate, 0, RateAnswer::Refused, now));
        assert!(playback.unsupported);
        assert!(playback.retry.is_none());
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn buffering_while_fast_asks_for_normal_speed_without_pretending_it_applied() {
        let edge = edge::Edge {
            rate: 1.5,
            ..edge::Edge::default()
        };
        let plan = plan_trim(
            edge,
            edge::Sample {
                lag: 2.0,
                buffering_for: Some(Duration::from_secs(1)),
                elapsed: Duration::from_millis(400),
            },
            false,
        );
        assert_eq!(plan.command, Some(1.0));
        assert!((plan.edge.rate - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_paused_rate_reply_judges_the_open_buffer() {
        let session = test_session();
        let paused = MediaStatus {
            media_session_id: 1,
            player_state: "PAUSED".to_owned(),
            idle_reason: None,
            current_time: 1.0,
        };
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.25;
        playback.edge.rate = 1.5;
        playback.edge.note_buffer(0.30);
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(350))
                .expect("test clock"),
        ));
        session.absorb_rate_status(&mut playback, &paused, Instant::now());
        assert!(session.buffering_since.get().is_none());
        assert!((playback.edge.target - 0.25).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);

        playback.edge.note_buffer(0.30);
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(650))
                .expect("test clock"),
        ));
        session.absorb_rate_status(&mut playback, &paused, Instant::now());
        assert!((playback.edge.target - 0.30).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);
        assert!(!playback.sampled);
    }

    #[test]
    fn a_polled_pause_keeps_the_backoff_the_buffer_earned() {
        let mut edge = edge::Edge {
            target: 0.25,
            floor: 0.25,
            rate: 1.5,
            ..edge::Edge::default()
        };
        edge.note_buffer(0.30);
        close_ended_buffer(&mut edge, "PAUSED", Some(Duration::from_millis(650)));
        let plan = plan_trim(
            edge,
            edge::Sample {
                lag: 0.40,
                buffering_for: None,
                elapsed: Duration::from_millis(400),
            },
            false,
        );
        assert!((plan.edge.target - 0.30).abs() < 1e-9);
        assert!((plan.edge.floor - 0.30).abs() < 1e-9);
        assert!((plan.edge.rate - 1.5).abs() < 1e-9);
        assert_eq!(plan.command, Some(1.0));
    }

    #[test]
    fn a_pause_event_closes_a_short_buffer_and_still_judges_a_long_one() {
        let session = test_session();
        let paused = MediaStatus {
            media_session_id: 1,
            player_state: "PAUSED".to_owned(),
            idle_reason: None,
            current_time: 1.0,
        };
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.25;
        playback.edge.settled_for = Duration::from_secs(3);
        playback.sampled = true;
        playback.edge.note_buffer(0.30);
        session.playback.set(playback);
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(350))
                .expect("test clock"),
        ));
        assert!(session.apply(&paused, None).is_ok());
        assert!(session.buffering_since.get().is_none());
        let after = session.playback.get();
        assert!((after.edge.target - 0.25).abs() < 1e-9);
        assert_eq!(after.edge.settled_for, Duration::ZERO);
        assert!(!after.sampled);
        let playing = MediaStatus {
            player_state: "PLAYING".to_owned(),
            ..paused.clone()
        };
        assert!(session.apply(&playing, None).is_ok());
        assert!((session.playback.get().edge.target - 0.25).abs() < 1e-9);

        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.25;
        playback.edge.note_buffer(0.30);
        session.playback.set(playback);
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(650))
                .expect("test clock"),
        ));
        assert!(session.apply(&paused, None).is_ok());
        assert!((session.playback.get().edge.target - 0.30).abs() < 1e-9);
    }

    #[test]
    fn a_pause_does_not_lower_the_target() {
        let edge = edge::Edge {
            settled_for: Duration::from_millis(3_600),
            ..edge::Edge::default()
        };
        let sample = edge::Sample {
            lag: 0.41,
            buffering_for: None,
            elapsed: Duration::from_millis(400),
        };
        let paused = plan_trim(edge, sample, false);
        assert!((paused.edge.target - 0.40).abs() < 1e-9);
        assert_eq!(paused.edge.settled_for, Duration::ZERO);
        assert!(paused.command.is_none());
        let playing = plan_trim(edge, sample, true);
        assert!((playing.edge.target - 0.35).abs() < 1e-9);
    }
}
