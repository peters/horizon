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
    Application, CastClient, DEFAULT_MEDIA_RECEIVER, Error, MediaController, MediaLoad, MediaStatus, Result,
    StreamType,
    client::{NS_CONNECTION, QueuedEvent, REQUEST_TIMEOUT},
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
/// After the estimated deadline, a restore retry still waits this long. The
/// command is already queued; a zero wait fails the session while the
/// receiver may already have returned to normal speed.
const ACK_WINDOW: Duration = Duration::from_millis(250);
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
    /// Last finite lag, and when it was observed.
    lag: Option<f64>,
    lag_at: Option<Instant>,
    /// Fastest rate that may already be applied. A lost speed-up reply leaves
    /// `edge.rate` unchanged, but the receiver may already be running at this.
    threat: f64,
}

/// A playback-rate command to send again at `at`. `failures` counts failed
/// returns to normal speed.
#[derive(Clone, Copy, Debug)]
struct Retry {
    at: Instant,
    rate: f64,
    failures: u8,
}

/// Which media session a folded notice belongs to.
struct CastMedia<'a> {
    session: &'a str,
    transport: &'a str,
    media_session: i64,
}

impl<'a> CastMedia<'a> {
    fn new(app: &'a Application, media_session: i64) -> Self {
        Self {
            session: &app.session_id,
            transport: &app.transport_id,
            media_session,
        }
    }
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
            lag_at: None,
            threat: 1.0,
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
        if let Sink::Progressive(stream) = &self.sink {
            let stream = Arc::clone(stream);
            client.watch_media_time(move || stream.newest_media_time());
        }
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
            if let Some(event) = client.next_notice(BUFFER_POLL)?
                && !self.follow_confirmed(&client, &event, &app, NO_MEDIA_SESSION)?
            {
                return Ok(());
            }
        }
        let mut media = client.media(&app);
        let load = media.load_observed(&self.media_load());
        let (loaded, receipt) = match load {
            Ok(status) => status,
            Err(error) => {
                let _ = client.stop_application(&app.session_id);
                return Err(error);
            }
        };
        // The LOAD reply is consumed by its request, so apply its status here.
        // Notices queued while LOAD blocked are older than that reply.
        let queued = buffer_notices(&client, &app.transport_id, loaded.media_session_id, receipt.at)?;
        let loaded_media = CastMedia::new(&app, loaded.media_session_id);
        let mut outcome = self.judge_polled_status(&loaded_media, &queued, &loaded, receipt.media_time, receipt.at);
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
            match self.keep_up(&media, &app) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => break Err(error),
            }
            // A restore that is already due cannot sit behind the event poll.
            // The buffer keeps shrinking for the whole wait.
            let poll = if self
                .playback
                .get()
                .retry
                .is_some_and(|retry| retry.at <= Instant::now())
            {
                Duration::ZERO
            } else {
                EVENT_POLL
            };
            outcome = match client.next_notice(poll)? {
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
        event: &QueuedEvent,
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
            // The queued event was discarded. Its source time is not this
            // reply's: sample immediately before the confirming request.
            let newest = self.source_media_time();
            let (reply, receipt) = client.media(app).status_observed(REQUEST_TIMEOUT)?;
            // Notices that arrived while this confirm blocked are older than
            // the reply. Judge them first, or the reply's later timestamp
            // stretches or hides the episode they already describe.
            let queued = buffer_notices(client, &app.transport_id, media_session_id, receipt.at)?;
            let media = CastMedia::new(app, media_session_id);
            return match reply {
                Some(status) if status.media_session_id == media_session_id => {
                    self.judge_polled_status(&media, &queued, &status, newest, receipt.at)
                }
                _ => {
                    self.fold_queued(&media, &queued)?;
                    Ok(false)
                }
            };
        }
        Ok(false)
    }

    /// Before reporting a stall, applies a fresh status: the BUFFERING that
    /// started the timer may have been queued behind a newer PLAYING reply.
    fn check_stall(&self, client: &CastClient, app: &Application, media_session_id: i64) -> Result<bool> {
        let newest = self.source_media_time();
        let (reply, receipt) = client.media(app).status_observed(REQUEST_TIMEOUT)?;
        let queued = buffer_notices(client, &app.transport_id, media_session_id, receipt.at)?;
        let media = CastMedia::new(app, media_session_id);
        let going_on = match reply {
            Some(status) if status.media_session_id == media_session_id => {
                self.judge_polled_status(&media, &queued, &status, newest, receipt.at)?
            }
            _ => {
                self.fold_queued(&media, &queued)?;
                false
            }
        };
        if going_on && self.stalled() {
            self.set(LiveState::Buffering);
        }
        Ok(going_on)
    }

    pub(crate) fn follow(
        &self,
        event: &QueuedEvent,
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
            if !self.apply(&status, detail.clone(), event.media_time, event.received_at)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A rate command's own status. Correlated replies are not replayed as events.
    fn absorb_rate_status(
        &self,
        playback: &mut Playback,
        status: &MediaStatus,
        observed: Instant,
        newest_at_send: Option<f64>,
    ) {
        match status.player_state.as_str() {
            "PLAYING" => {
                // This reply is not delivered again as an event. A stall that
                // moved the session to Buffering has to see it here, or the
                // poll never resumes.
                self.observe_transport_state("PLAYING");
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
                    if let Some(lag) = note_open_lag(newest_at_send, status.current_time) {
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

    /// Progressive media time now, if this session has that clock.
    fn source_media_time(&self) -> Option<f64> {
        let Sink::Progressive(stream) = &self.sink else {
            return None;
        };
        stream.newest_media_time()
    }

    /// Records the lag at the start of a progressive buffer. `newest` is the
    /// source position when the report was received. Sampling again here is
    /// too late: frames keep arriving while the event waits behind a request.
    fn note_buffer_lag(&self, current_time: f64, newest: Option<f64>) {
        let Some(lag) = note_open_lag(newest.or_else(|| self.source_media_time()), current_time) else {
            return;
        };
        let mut playback = self.playback.get();
        playback.edge.note_buffer(lag);
        self.playback.set(playback);
    }

    /// Applies one status of our media session; `Ok(false)` means playback ended.
    fn apply(
        &self,
        status: &MediaStatus,
        detail: Option<String>,
        media_time: Option<f64>,
        at: Instant,
    ) -> Result<bool> {
        match (status.player_state.as_str(), status.idle_reason.as_deref()) {
            ("PLAYING", _) => {
                self.started.set(true);
                let lasted = self
                    .buffering_since
                    .take()
                    .map(|since| at.saturating_duration_since(since));
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
                // The first notice wins the opening time. A later poll must be
                // folded in receipt order before it reaches this branch, or
                // this guard would ignore the earlier notice.
                if self.buffering_since.get().is_none() {
                    self.buffering_since.set(Some(at));
                    self.note_buffer_lag(status.current_time, media_time);
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
                let lasted = self
                    .buffering_since
                    .take()
                    .map(|since| at.saturating_duration_since(since));
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

    /// Applies buffer notices that were already queued, oldest first, then
    /// `status`. The reply was received after those notices. Judging it first
    /// would time the episode from the reply.
    fn judge_polled_status(
        &self,
        media: &CastMedia<'_>,
        queued: &[QueuedEvent],
        status: &MediaStatus,
        media_time: Option<f64>,
        at: Instant,
    ) -> Result<bool> {
        self.fold_queued(media, queued)?;
        self.apply(status, None, media_time, at)
    }

    /// Drains buffer notices and applies them before the caller judges a reply.
    fn fold_ready(&self, client: &CastClient, app: &Application, media_session_id: i64, at: Instant) -> Result<()> {
        let queued = buffer_notices(client, &app.transport_id, media_session_id, at)?;
        self.fold_queued(&CastMedia::new(app, media_session_id), &queued)
    }

    /// Applies `queued` in receipt order. A notice that ends the session does
    /// not stop this fold: the reply the caller is about to judge is newer.
    fn fold_queued(&self, media: &CastMedia<'_>, queued: &[QueuedEvent]) -> Result<()> {
        let mut ordered: Vec<&QueuedEvent> = queued.iter().collect();
        ordered.sort_by_key(|event| event.received_at);
        for event in ordered {
            let _newer_reply_decides = self.follow(event, media.session, media.transport, media.media_session)?;
        }
        Ok(())
    }

    /// Copies the edge a folded notice just stored. The caller holds its own
    /// playback copy and would otherwise write that edge back out.
    fn adopt_folded_edge(&self, playback: &mut Playback) {
        let folded = self.playback.get();
        playback.edge = folded.edge;
        playback.sampled = folded.sampled;
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
    fn keep_up(&self, media: &MediaController<'_>, app: &Application) -> Result<bool> {
        let Sink::Progressive(stream) = &self.sink else {
            return Ok(true);
        };
        let mut playback = self.playback.get();
        let now = Instant::now();
        // A restore still has to land after a faster rate was refused.
        if let Some(retry) = playback.retry {
            playback.sampled = false;
            if now < retry.at {
                self.playback.set(playback);
                return Ok(true);
            }
            return self.push_rate(media, app, playback, retry.rate, retry.failures);
        }
        if playback.unsupported {
            return Ok(true);
        }
        let state = lock(&self.state).clone();
        if !polls_after_stall(self.started.get(), &state) {
            // Not playing yet, or the session is already over. A faster rate
            // still has to come down.
            playback.sampled = false;
            if self.started.get() && fastest(&playback) > 1.0 + 1e-3 {
                return self.push_rate(media, app, playback, 1.0, 0);
            }
            self.playback.set(playback);
            return Ok(true);
        }
        if state != LiveState::Playing {
            // A stall moved us to Buffering. Keep polling: the next PLAYING
            // reply is often only the status we ask for, not an event.
            playback.sampled = false;
            if fastest(&playback) > 1.0 + 1e-3 {
                return self.push_rate(media, app, playback, 1.0, 0);
            }
        }
        if now < playback.next_check {
            self.playback.set(playback);
            return Ok(true);
        }
        playback.next_check = now + EDGE_POLL;
        // GET_STATUS blocks this loop, so a lost reply cannot be told from a
        // slow one until the timeout. While playback is already fast, stop
        // waiting in time to send normal speed. A buffer thinner than that
        // restores without asking.
        let Some(wait) = status_wait(&playback, now) else {
            playback.sampled = false;
            return self.push_rate(media, app, playback, 1.0, 0);
        };
        // Newest frame before the call. A buffer that opens during the wait
        // keeps this lag; the timestamp after the reply has moved on with the
        // source. The lag used for the rate decision is still read afterwards,
        // or a slow reply makes the receiver look closer than it is.
        let newest_at_send = stream.newest_media_time();
        let status = media.status_observed(wait);
        if matches!(&status, Ok((None, _))) {
            return Ok(false);
        }
        if let Ok((Some(status), _)) = &status
            && !self.owns_reply(media, status)?
        {
            return Ok(false);
        }
        let newest = stream.newest_media_time();
        let (Some(newest), Ok((Some(status), receipt))) = (newest, status) else {
            // The receiver may already be playing fast. Without a sample there
            // is no signal to stop, so return to normal speed.
            playback.sampled = false;
            if fastest(&playback) > 1.0 + 1e-3 {
                return self.push_rate(media, app, playback, 1.0, 0);
            }
            self.playback.set(playback);
            return Ok(true);
        };
        let lag = receipt.media_time.unwrap_or(newest) - status.current_time;
        // Notices queued during the poll were received before this reply.
        // Fold them first, or a short gap is measured out to the reply and a
        // real stall can miss the opening time on its own notice.
        self.fold_ready(media.client(), app, media.session_id()?, receipt.at)?;
        self.adopt_folded_edge(&mut playback);
        // The status call blocks. Time the buffer, and the lag sample, from
        // when the reply arrived. A deadline taken at send time is already
        // partly spent.
        let observed = receipt.at;
        if lag.is_finite() {
            playback.lag = Some(lag);
            playback.lag_at = Some(observed);
            playback.threat = fastest(&playback);
        }
        let lasted = self
            .buffering_since
            .get()
            .map(|since| observed.saturating_duration_since(since));
        let mut open = self.buffering_since.get();
        let buffering_for = buffering_age(&mut open, status.player_state.as_str(), observed);
        self.buffering_since.set(open);
        let playing = status.player_state == "PLAYING";
        self.observe_transport_state(status.player_state.as_str());
        // A pause is not a buffer. Seeding the episode here would make a
        // later, larger stall look like it opened near the edge.
        if let Some(opening) = opening_from_status(status.player_state.as_str(), newest_at_send, status.current_time) {
            playback.edge.note_buffer(opening);
        }
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
            return Ok(true);
        };
        tracing::debug!(lag, rate, target = plan.edge.target, "trimming live playback");
        self.push_rate(media, app, playback, rate, 0)
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
        app: &Application,
        mut playback: Playback,
        rate: f64,
        failures: u8,
    ) -> Result<bool> {
        // The status poll that led here may have blocked. This wait starts now.
        let now = Instant::now();
        // A speed-up with no room for an acknowledgement is not queued. The
        // send happens before the wait, so a zero timeout would still deliver
        // it and the restore would leave with it. One that does not fit at the
        // asked rate still goes out more slowly, so the target can keep moving.
        // When nothing safe is faster and playback is already fast, ask for
        // normal speed now. Waiting for the next poll leaves that rate on.
        let Some(rate) = command_after_admission(&playback, rate, now) else {
            self.playback.set(playback);
            return Ok(true);
        };
        let Some(wait) = begin_rate(&mut playback, rate, now) else {
            self.playback.set(playback);
            return Ok(true);
        };
        let newest_at_send = self.source_media_time();
        let (answer, error) = match media.rate_observed(rate, wait) {
            Ok((status, receipt)) => {
                if !self.owns_reply(media, &status)? {
                    return Ok(false);
                }
                // The reply is not delivered again as an event. Notices queued
                // while the command blocked are older than it. Store this
                // playback first: the fold loads the session copy, and the
                // planned edge still lives only in this local value.
                self.playback.set(playback);
                self.fold_ready(media.client(), app, media.session_id()?, receipt.at)?;
                playback = self.playback.get();
                self.absorb_rate_status(&mut playback, &status, receipt.at, newest_at_send);
                (RateAnswer::Applied, None)
            }
            Err(error @ Error::Rejected { .. }) => (RateAnswer::Refused, Some(error)),
            Err(error) => (RateAnswer::Unconfirmed, Some(error)),
        };
        let answered = Instant::now();
        if apply_rate_reply(&mut playback, rate, failures, answer, answered) {
            self.playback.set(playback);
            return Err(error.unwrap_or(Error::Protocol("playback rate was not confirmed")));
        }
        // A lost speed-up has to be followed by normal speed before the event
        // poll. The restore uses whatever the speed-up wait left of the buffer.
        let due = playback.retry.is_some_and(|retry| retry.at <= Instant::now());
        if (rate - 1.0).abs() > 1e-3 && due {
            let Some(retry) = playback.retry.take() else {
                self.playback.set(playback);
                return Ok(true);
            };
            return self.push_rate(media, app, playback, retry.rate, retry.failures);
        }
        self.playback.set(playback);
        Ok(true)
    }

    /// A correlated reply must still belong to the original LOAD.
    fn owns_reply(&self, media: &MediaController<'_>, status: &MediaStatus) -> Result<bool> {
        if status.media_session_id != media.session_id()? {
            return Ok(false);
        }
        if status.player_state == "IDLE" {
            return self.apply(status, None, None, Instant::now());
        }
        Ok(true)
    }

    /// Playing, but the receiver has reported buffering for longer than the grace period.
    /// A PLAYING reply, including one that is not replayed as an event, puts
    /// the session back where the live-edge poll runs.
    fn observe_transport_state(&self, player_state: &str) {
        if player_state == "PLAYING" {
            self.started.set(true);
            self.set(LiveState::Playing);
        }
    }

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

/// Move the lag sample to `now`, subtracting what the current fastest rate
/// ate since it was taken. The next rate then starts from that remainder.
fn rebase_lag(playback: &mut Playback, now: Instant) {
    let Some(lag) = playback.lag.filter(|lag| lag.is_finite()) else {
        return;
    };
    let Some(from) = playback.lag_at else {
        return;
    };
    let rate = fastest(playback);
    let age = now.saturating_duration_since(from).as_secs_f64();
    let eaten = if rate > 1.0 + 1e-3 { age * (rate - 1.0) } else { 0.0 };
    playback.lag = Some(lag - eaten);
    playback.lag_at = Some(now);
}

/// Fastest rate that may already be applied, confirmed or only sent.
fn fastest(playback: &Playback) -> f64 {
    playback.threat.max(playback.edge.rate)
}

/// When lag reaches the current target if `fastest` has been applied since
/// the sample. `None` when playback is not eating the buffer. Stopping at
/// zero would spend the headroom the target is there to keep.
fn buffer_gone(playback: &Playback) -> Option<Instant> {
    let lag = playback.lag.filter(|lag| lag.is_finite())?;
    let from = playback.lag_at?;
    let rate = fastest(playback);
    if rate <= 1.0 + 1e-3 {
        return None;
    }
    let headroom = lag - playback.edge.target;
    if !headroom.is_finite() {
        return None;
    }
    if headroom <= 0.0 {
        return Some(from);
    }
    let secs = headroom / (rate - 1.0);
    if !secs.is_finite() || secs <= 0.0 {
        return Some(from);
    }
    // An unbounded lag does not fit in `Duration`. Past the ordinary request
    // timeout the exact instant no longer changes the cap.
    let span = if secs >= REQUEST_TIMEOUT.as_secs_f64() + RESTORE_BUDGET.as_secs_f64() {
        REQUEST_TIMEOUT.saturating_add(RESTORE_BUDGET)
    } else {
        Duration::from_secs_f64(secs)
    };
    from.checked_add(span)
}

/// Wall time left before lag reaches the target.
fn time_left(playback: &Playback, now: Instant) -> Option<Duration> {
    buffer_gone(playback).map(|gone| gone.saturating_duration_since(now))
}

/// How long a status poll may block. `None` means the buffer can run out
/// before a poll and a restore would both finish, so restore immediately.
/// The clock is the sample's, so time already spent playing fast counts.
fn status_wait(playback: &Playback, now: Instant) -> Option<Duration> {
    if fastest(playback) <= 1.0 + 1e-3 {
        return Some(REQUEST_TIMEOUT);
    }
    if playback.lag.is_some_and(|lag| lag.is_finite() && lag <= 0.0) {
        return None;
    }
    let Some(left) = time_left(playback, now) else {
        return Some(MIN_STATUS_WAIT);
    };
    if left <= RESTORE_BUDGET.saturating_add(MIN_STATUS_WAIT) {
        return None;
    }
    Some(left.saturating_sub(RESTORE_BUDGET).min(REQUEST_TIMEOUT))
}

/// How long a rate command may block. A speed-up leaves the restore budget.
/// A restore, including one whose speed-up reply was lost, uses only the
/// time that budget still has.
fn command_wait(playback: &Playback, commanded: f64, now: Instant) -> Duration {
    let restoring = (commanded - 1.0).abs() <= 1e-3;
    let Some(left) = time_left(playback, now) else {
        if fastest(playback) <= 1.0 + 1e-3 {
            return REQUEST_TIMEOUT;
        }
        return if restoring { RESTORE_BUDGET } else { MIN_STATUS_WAIT };
    };
    if restoring {
        // `left` is zero once the estimate has passed. Still wait for the
        // acknowledgement: the command was queued before this timeout.
        return left.min(RESTORE_BUDGET).max(ACK_WINDOW);
    }
    // A speed-up needs a full acknowledgement window and the restore budget
    // after it. Anything shorter is refused by [`begin_rate`]: queueing it
    // would time out at once and the restore would be queued behind it.
    let reserved = RESTORE_BUDGET.saturating_add(ACK_WINDOW);
    if left < reserved {
        return Duration::ZERO;
    }
    left.saturating_sub(RESTORE_BUDGET).min(REQUEST_TIMEOUT)
}

/// Starts a rate command. `None` means do not queue it. [`crate::client::CastClient::request_within`]
/// sends before it waits, so a window shorter than [`ACK_WINDOW`] still
/// delivers a speed-up and then reports it lost.
fn begin_rate(playback: &mut Playback, commanded: f64, now: Instant) -> Option<Duration> {
    let restoring = (commanded - 1.0).abs() <= 1e-3;
    let wait = if restoring {
        command_wait(playback, commanded, now)
    } else {
        let mut feared = *playback;
        feared.threat = feared.threat.max(commanded);
        command_wait(&feared, commanded, now)
    };
    if !restoring && wait < ACK_WINDOW {
        return None;
    }
    if commanded > 1.0 + 1e-3 {
        playback.threat = playback.threat.max(commanded);
    }
    Some(wait)
}

/// Whether `rate` leaves an acknowledgement window and the restore budget.
fn rate_fits(playback: &Playback, rate: f64, now: Instant) -> bool {
    let mut feared = *playback;
    feared.threat = feared.threat.max(rate);
    command_wait(&feared, rate, now) >= ACK_WINDOW
}

/// The rate to queue. A speed-up that would finish before it can be undone
/// is slowed until the same headroom lasts for the acknowledgement and the
/// restore. `None` when there is nothing safe to send.
fn admit_rate(playback: &Playback, wanted: f64, now: Instant) -> Option<f64> {
    if wanted <= 1.0 + 1e-3 {
        return Some(wanted);
    }
    if rate_fits(playback, wanted, now) {
        return Some(wanted);
    }
    let lag = playback.lag.filter(|lag| lag.is_finite())?;
    let headroom = lag - playback.edge.target;
    if headroom <= 1e-3 || !headroom.is_finite() {
        return None;
    }
    // A millisecond past the budget, so the boundary does not round back to a hold.
    let reserved = RESTORE_BUDGET.saturating_add(ACK_WINDOW).as_secs_f64() + 0.001;
    let excess = headroom / reserved;
    if excess <= 1e-3 || !excess.is_finite() {
        return None;
    }
    let slower = (1.0 + excess).min(wanted);
    rate_fits(playback, slower, now).then_some(slower)
}

/// The rate to queue after [`admit_rate`]. `None` means send nothing. A fast
/// rate that cannot take another trim returns to normal speed instead of
/// staying fast until the next check.
fn command_after_admission(playback: &Playback, wanted: f64, now: Instant) -> Option<f64> {
    if let Some(rate) = admit_rate(playback, wanted, now) {
        return Some(rate);
    }
    (fastest(playback) > 1.0 + 1e-3).then_some(1.0)
}

/// Buffer notices already queued for our media session. Anything else is put
/// back, in order, so the cast loop still sees a terminal event.
fn buffer_notices(
    client: &CastClient,
    transport_id: &str,
    media_session_id: i64,
    at: Instant,
) -> Result<Vec<QueuedEvent>> {
    client.take_notices(|event| event.received_at <= at && buffer_notice(event, transport_id, media_session_id))
}

/// A playing, paused, or buffering status for our session. Idle and receiver
/// notices stay on the queue: they decide whether the session continues.
fn buffer_notice(event: &QueuedEvent, transport_id: &str, media_session_id: i64) -> bool {
    event.source == transport_id
        && MediaStatus::from_event(event).iter().any(|status| {
            status.media_session_id == media_session_id
                && matches!(
                    status.player_state.as_str(),
                    "BUFFERING" | "LOADING" | "PLAYING" | "PAUSED"
                )
        })
}

/// Whether the live-edge poll still runs. A stall reports Buffering, and the
/// recovery often arrives only as a later status reply.
fn polls_after_stall(started: bool, state: &LiveState) -> bool {
    started && matches!(state, LiveState::Playing | LiveState::Buffering)
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

/// Lag when a progressive buffer opens. `None` for other transports, before
/// the first frame, or for a player state that is not buffering.
fn note_open_lag(newest: Option<f64>, current_time: f64) -> Option<f64> {
    newest.map(|newest| newest - current_time)
}

/// Opening lag from a polled status. A pause has no buffer episode.
fn opening_from_status(player_state: &str, newest: Option<f64>, current_time: f64) -> Option<f64> {
    if !matches!(player_state, "BUFFERING" | "LOADING") {
        return None;
    }
    note_open_lag(newest, current_time)
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
            // The lag sample was eaten at the previous fastest rate. Rebase it
            // before recording a slower one, or the next poll treats the whole
            // wait as if the slower rate had already been applied.
            rebase_lag(playback, now);
            playback.edge.rate = commanded;
            playback.threat = commanded;
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
            // The command did not apply. The confirmed rate is still the one
            // in `edge`, not the rate we had started to fear.
            playback.threat = playback.edge.rate;
            if playback.edge.rate > 1.0 + 1e-3 {
                playback.retry = Some(Retry {
                    at: now,
                    rate: 1.0,
                    failures: 0,
                });
            } else {
                playback.edge.rate = 1.0;
                playback.threat = 1.0;
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
            Some(CatchUp::Until(_, failures)) => {
                // A half-second pause is too long once the buffer is already
                // inside the restore budget. Try again at once.
                let at = match time_left(playback, now) {
                    Some(left) if left <= RESTORE_BUDGET.saturating_add(RESTORE_RETRY) => now,
                    _ => now + RESTORE_RETRY,
                };
                playback.retry = Some(Retry {
                    at,
                    rate: 1.0,
                    failures,
                });
                false
            }
            Some(CatchUp::Watching) => {
                playback.edge.rate = 1.0;
                playback.threat = 1.0;
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
        session.absorb_rate_status(&mut playback, &playing, Instant::now(), None);
        assert!(session.buffering_since.get().is_none());
        assert!((playback.edge.target - 0.25).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);

        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(650))
                .expect("test clock"),
        ));
        playback.edge.note_buffer(0.30);
        session.absorb_rate_status(&mut playback, &playing, Instant::now(), None);
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

    fn paced(rate: f64, lag: f64, at: Instant) -> Playback {
        let mut playback = Playback::new();
        playback.edge.rate = rate;
        playback.threat = rate;
        playback.lag = Some(lag);
        playback.lag_at = Some(at);
        playback
    }

    #[test]
    fn a_fast_status_poll_leaves_time_to_restore() {
        // One second of lag at 1.5×, aiming at 0.15 s, leaves 1.7 s. The poll
        // must end with the restore budget still unused, and with the target
        // still ahead.
        let now = Instant::now();
        let mut fast = paced(1.5, 1.0, now);
        fast.edge.target = 0.15;
        let wait = status_wait(&fast, now).expect("poll");
        let until_target = Duration::from_secs_f64((1.0 - 0.15) / 0.5);
        assert!(wait < REQUEST_TIMEOUT);
        assert!(wait.saturating_add(RESTORE_BUDGET) <= until_target);
        assert_eq!(status_wait(&paced(1.0, 0.40, now), now), Some(REQUEST_TIMEOUT));
        let mut unknown = Playback::new();
        unknown.edge.rate = 1.08;
        unknown.threat = 1.08;
        assert_eq!(status_wait(&unknown, now), Some(MIN_STATUS_WAIT));
        assert!(status_wait(&paced(1.5, 0.20, now), now).is_none());
        // 0.21 s at 1.08× with a 0.15 s target is only the restore budget.
        // Polling would spend the headroom before normal speed is requested.
        let mut near = paced(1.08, 0.21, now);
        near.edge.target = 0.15;
        assert!(status_wait(&near, now).is_none());
        let restore = command_wait(&fast, 1.0, now);
        assert!(restore <= RESTORE_BUDGET);
        assert!(restore < REQUEST_TIMEOUT);
    }

    #[test]
    fn an_aged_fast_sample_shortens_the_status_poll() {
        // 500 ms later the same sample has 500 ms less life above the target.
        let sampled = Instant::now();
        let mut playback = paced(1.5, 1.0, sampled);
        playback.edge.target = 0.15;
        let later = sampled + Duration::from_millis(500);
        let wait = status_wait(&playback, later).expect("poll");
        let until_target = Duration::from_secs_f64((1.0 - 0.15) / 0.5);
        assert!(
            Duration::from_millis(500)
                .saturating_add(wait)
                .saturating_add(RESTORE_BUDGET)
                <= until_target
        );
    }

    #[test]
    fn a_lost_near_edge_speed_up_leaves_time_to_restore() {
        let sampled = Instant::now();
        let mut playback = paced(1.0, 1.0, sampled);
        playback.edge.target = 0.15;
        playback.threat = playback.threat.max(1.5);
        let wait = command_wait(&playback, 1.5, sampled);
        let drain = Duration::from_secs_f64((1.0 - 0.15) / 0.5);
        assert!(wait < REQUEST_TIMEOUT);
        assert!(wait.saturating_add(RESTORE_BUDGET) <= drain);

        let lost_at = sampled + wait;
        assert!(!apply_rate_reply(
            &mut playback,
            1.5,
            0,
            RateAnswer::Unconfirmed,
            lost_at
        ));
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!((playback.threat - 1.5).abs() < 1e-9);
        let restore = command_wait(&playback, 1.0, lost_at);
        assert!(restore <= RESTORE_BUDGET);
        assert!(restore < REQUEST_TIMEOUT);
        assert!(wait.saturating_add(restore) <= drain);
    }

    #[test]
    fn a_restore_retry_after_the_deadline_still_waits_for_the_ack() {
        let sampled = Instant::now();
        let mut playback = paced(1.0, 0.21, sampled);
        playback.threat = 1.08;
        let speed = command_wait(&playback, 1.08, sampled);
        let lost_at = sampled + speed;
        assert!(!apply_rate_reply(
            &mut playback,
            1.08,
            0,
            RateAnswer::Unconfirmed,
            lost_at
        ));
        let first = command_wait(&playback, 1.0, lost_at);
        assert!(first >= ACK_WINDOW);
        assert!(first <= RESTORE_BUDGET);
        let retried_at = lost_at + first;
        assert!(!apply_rate_reply(
            &mut playback,
            1.0,
            0,
            RateAnswer::Unconfirmed,
            retried_at
        ));
        let second = command_wait(&playback, 1.0, retried_at);
        assert!(second >= ACK_WINDOW);
        assert!(second < REQUEST_TIMEOUT);
        assert!(!apply_rate_reply(
            &mut playback,
            1.0,
            1,
            RateAnswer::Applied,
            retried_at + second
        ));
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!((playback.threat - 1.0).abs() < 1e-9);
        assert!(playback.retry.is_none());
    }

    #[test]
    fn a_slower_rate_does_not_rewrite_the_lag_already_eaten() {
        // 0.90 s at 1.5×, then 1.08× acknowledged a second later. The second
        // ate 0.50 s, so 0.40 s remains. A poll 250 ms after that has 4.75 s
        // until empty at 1.08×, and less than that until the 0.15 s target.
        let sampled = Instant::now();
        let mut playback = paced(1.5, 0.90, sampled);
        playback.edge.target = 0.15;
        let acked = sampled + Duration::from_secs(1);
        assert!(!apply_rate_reply(&mut playback, 1.08, 0, RateAnswer::Applied, acked));
        assert!((playback.edge.rate - 1.08).abs() < 1e-9);
        assert!((playback.threat - 1.08).abs() < 1e-9);
        let left = playback.lag.expect("rebased");
        assert!((left - 0.40).abs() < 1e-6);
        let poll = acked + Duration::from_millis(250);
        let wait = status_wait(&playback, poll).expect("poll");
        let true_left = Duration::from_secs_f64(4.75);
        assert!(wait.saturating_add(RESTORE_BUDGET) <= true_left);
        assert!(wait < Duration::from_secs(5));
    }

    #[test]
    fn a_late_buffer_reply_keeps_the_opening_lag() {
        let mut edge = edge::Edge {
            target: 0.25,
            floor: 0.15,
            ..edge::Edge::default()
        };
        // Newest frame at the request, minus the stopped receiver clock.
        let opening = note_open_lag(Some(10.30), 10.0).expect("opening");
        assert!((opening - 0.30).abs() < 1e-9);
        edge.note_buffer(opening);
        let next = edge.step(edge::Sample {
            lag: 0.90,
            buffering_for: Some(Duration::from_millis(600)),
            elapsed: Duration::from_millis(600),
        });
        assert!((next.episode_lag.expect("opening kept") - 0.30).abs() < 1e-9);
        assert!((next.target - 0.30).abs() < 1e-9);
    }

    #[test]
    fn a_stall_keeps_polling_until_a_playing_reply() {
        assert!(polls_after_stall(true, &LiveState::Buffering));
        assert!(polls_after_stall(true, &LiveState::Playing));
        assert!(!polls_after_stall(false, &LiveState::Buffering));
        assert!(!polls_after_stall(true, &LiveState::Ended));
        assert!(!polls_after_stall(true, &LiveState::Failed(String::new())));
    }

    #[test]
    fn a_playing_rate_reply_resumes_a_stall() {
        let session = test_session();
        session.started.set(true);
        session.set(LiveState::Buffering);
        session.buffering_since.set(Some(
            Instant::now().checked_sub(Duration::from_secs(6)).expect("test clock"),
        ));
        let mut playback = Playback::new();
        playback.edge.note_buffer(0.30);
        let playing = MediaStatus {
            media_session_id: 1,
            player_state: "PLAYING".to_owned(),
            idle_reason: None,
            current_time: 1.0,
        };
        session.absorb_rate_status(&mut playback, &playing, Instant::now(), None);
        assert_eq!(*lock(&session.state), LiveState::Playing);
        assert!(session.started.get());
        assert!(session.buffering_since.get().is_none());
        session.set(LiveState::Buffering);
        session.observe_transport_state("PLAYING");
        assert_eq!(*lock(&session.state), LiveState::Playing);
        session.observe_transport_state("BUFFERING");
        assert_eq!(*lock(&session.state), LiveState::Playing);
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
        session.absorb_rate_status(&mut playback, &paused, Instant::now(), None);
        assert!(session.buffering_since.get().is_none());
        assert!((playback.edge.target - 0.25).abs() < 1e-9);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);

        playback.edge.note_buffer(0.30);
        session.buffering_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(650))
                .expect("test clock"),
        ));
        session.absorb_rate_status(&mut playback, &paused, Instant::now(), None);
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
        assert!(session.apply(&paused, None, None, Instant::now()).is_ok());
        assert!(session.buffering_since.get().is_none());
        let after = session.playback.get();
        assert!((after.edge.target - 0.25).abs() < 1e-9);
        assert_eq!(after.edge.settled_for, Duration::ZERO);
        assert!(!after.sampled);
        let playing = MediaStatus {
            player_state: "PLAYING".to_owned(),
            ..paused.clone()
        };
        assert!(session.apply(&playing, None, None, Instant::now()).is_ok());
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
        assert!(session.apply(&paused, None, None, Instant::now()).is_ok());
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

    /// Models `request_within`: the command is queued before the wait. A lost
    /// speed-up is followed at once by the restore, the same way `push_rate` chains it.
    fn drive_rate(
        playback: &mut Playback,
        commanded: f64,
        now: Instant,
        answer: RateAnswer,
    ) -> std::result::Result<Vec<f64>, ()> {
        let Some(wait) = begin_rate(playback, commanded, now) else {
            return Ok(Vec::new());
        };
        let mut sent = vec![commanded];
        let seen = if wait.is_zero() {
            RateAnswer::Unconfirmed
        } else {
            answer
        };
        let answered = now + wait;
        if apply_rate_reply(playback, commanded, 0, seen, answered) {
            return Err(());
        }
        let due = playback.retry.is_some_and(|retry| retry.at <= answered);
        if (commanded - 1.0).abs() > 1e-3 && due {
            let retry = playback.retry.take().expect("due retry");
            sent.extend(drive_rate(playback, retry.rate, answered, answer)?);
        }
        Ok(sent)
    }

    #[test]
    fn a_near_edge_speed_up_is_not_queued_with_its_restore() {
        // 0.405 s after the target drops to 0.35 s is 687.5 ms at 1.08×,
        // inside the restore budget. Queuing 1.08× would time out at once.
        let now = Instant::now();
        let mut feared = paced(1.0, 0.405, now);
        feared.edge.target = 0.35;
        feared.threat = 1.08;
        assert_eq!(command_wait(&feared, 1.08, now), Duration::ZERO);

        for answer in [RateAnswer::Applied, RateAnswer::Refused] {
            let mut playback = paced(1.0, 0.405, now);
            playback.edge.target = 0.35;
            let sent = drive_rate(&mut playback, 1.08, now, answer).expect("held");
            assert!(sent.is_empty(), "{sent:?} {answer:?}");
            assert!((playback.edge.rate - 1.0).abs() < 1e-9);
            assert!((playback.threat - 1.0).abs() < 1e-9);
            assert!(playback.retry.is_none());
            assert!(!playback.unsupported);
        }
    }

    #[test]
    fn a_rejecting_receiver_is_not_failed_by_an_unsent_speed_up() {
        let now = Instant::now();
        let mut playback = paced(1.0, 0.405, now);
        playback.edge.target = 0.35;
        for _ in 0..RESTORE_ATTEMPTS {
            let sent = drive_rate(&mut playback, 1.08, now, RateAnswer::Refused).expect("held");
            assert!(sent.is_empty());
        }
        assert!(!playback.unsupported);
        assert!(playback.retry.is_none());
    }

    #[test]
    fn an_accepting_receiver_keeps_a_speed_up_that_has_room() {
        let now = Instant::now();
        let mut playback = paced(1.0, 1.0, now);
        playback.edge.target = 0.15;
        let sent = drive_rate(&mut playback, 1.5, now, RateAnswer::Applied).expect("applied");
        assert_eq!(sent, vec![1.5]);
        assert!((playback.edge.rate - 1.5).abs() < 1e-9);
        assert!((playback.threat - 1.5).abs() < 1e-9);
        assert!(playback.retry.is_none());
        let mut feared = paced(1.0, 1.0, now);
        feared.edge.target = 0.15;
        feared.threat = 1.5;
        let wait = command_wait(&feared, 1.5, now);
        let drain = Duration::from_secs_f64((1.0 - 0.15) / 0.5);
        assert!(wait >= ACK_WINDOW);
        assert!(wait.saturating_add(RESTORE_BUDGET) <= drain);
    }

    #[test]
    fn a_rejecting_receiver_stops_at_the_refused_speed_up() {
        let now = Instant::now();
        let mut playback = paced(1.0, 1.0, now);
        playback.edge.target = 0.15;
        let sent = drive_rate(&mut playback, 1.5, now, RateAnswer::Refused).expect("refused");
        assert_eq!(sent, vec![1.5]);
        assert!(playback.unsupported);
        assert!(playback.retry.is_none());
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!((playback.threat - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_lost_speed_up_with_room_still_queues_the_restore() {
        let now = Instant::now();
        let mut playback = paced(1.0, 1.0, now);
        playback.edge.target = 0.15;
        let sent = drive_rate(&mut playback, 1.5, now, RateAnswer::Unconfirmed).expect("restore");
        assert_eq!(sent, vec![1.5, 1.0]);
        assert!((playback.threat - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_pause_does_not_seed_the_next_buffer() {
        assert!(opening_from_status("PAUSED", Some(10.30), 10.0).is_none());
        let opening = opening_from_status("BUFFERING", Some(10.30), 10.0).expect("buffer");
        assert!((opening - 0.30).abs() < 1e-9);
        let edge = edge::Edge {
            target: 0.25,
            floor: 0.15,
            ..edge::Edge::default()
        };
        let plan = plan_trim(
            edge,
            edge::Sample {
                lag: 0.30,
                buffering_for: None,
                elapsed: Duration::from_millis(400),
            },
            false,
        );
        assert!(plan.edge.episode_lag.is_none());
        let next = plan.edge.step(edge::Sample {
            lag: 0.90,
            buffering_for: Some(Duration::from_millis(600)),
            elapsed: Duration::from_millis(600),
        });
        assert!((next.target - 0.25).abs() < 1e-9);
        assert!((next.episode_lag.expect("sample") - 0.90).abs() < 1e-9);
    }

    #[test]
    fn a_delayed_buffer_event_keeps_the_opening_lag() {
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        session.playback.set(playback);
        let event = QueuedEvent {
            event: crate::Event {
                namespace: NS_MEDIA.to_owned(),
                source: "transport-1".to_owned(),
                payload: serde_json::json!({
                    "type": "MEDIA_STATUS",
                    "requestId": 0,
                    "status": [{"mediaSessionId": 9, "playerState": "BUFFERING", "currentTime": 10.0}]
                }),
            },
            media_time: Some(10.30),
            received_at: Instant::now(),
        };
        assert!(session.follow(&event, "session-1", "transport-1", 9).unwrap());
        let edge = session.playback.get().edge;
        assert!((edge.episode_lag.expect("opening") - 0.30).abs() < 1e-9);
        let next = edge.step(edge::Sample {
            lag: 0.90,
            buffering_for: Some(Duration::from_millis(600)),
            elapsed: Duration::from_millis(600),
        });
        assert!((next.target - 0.30).abs() < 1e-9);
        assert!((next.floor - 0.30).abs() < 1e-9);
    }

    #[test]
    fn a_delayed_rate_reply_keeps_the_opening_lag() {
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        let status = MediaStatus {
            media_session_id: 1,
            player_state: "BUFFERING".to_owned(),
            idle_reason: None,
            current_time: 10.0,
        };
        session.absorb_rate_status(&mut playback, &status, Instant::now(), Some(10.30));
        assert!((playback.edge.episode_lag.expect("opening") - 0.30).abs() < 1e-9);
        let next = playback.edge.step(edge::Sample {
            lag: 0.90,
            buffering_for: Some(Duration::from_millis(600)),
            elapsed: Duration::from_millis(600),
        });
        assert!((next.target - 0.30).abs() < 1e-9);
        assert!((next.floor - 0.30).abs() < 1e-9);
    }

    fn media_notice(player_state: &str, current_time: f64, media_time: Option<f64>, at: Instant) -> QueuedEvent {
        QueuedEvent {
            event: crate::Event {
                namespace: NS_MEDIA.to_owned(),
                source: "transport-1".to_owned(),
                payload: serde_json::json!({
                    "type": "MEDIA_STATUS",
                    "requestId": 0,
                    "status": [{"mediaSessionId": 9, "playerState": player_state, "currentTime": current_time}]
                }),
            },
            media_time,
            received_at: at,
        }
    }

    #[test]
    fn a_confirming_reply_does_not_use_the_discarded_event_stamp() {
        // A stale IDLE stamped at 10.30 s, then a confirming BUFFERING reply
        // at currentTime 11.0 s while the source is already at 12.0 s.
        let stale = note_open_lag(Some(10.30), 11.0).expect("stale");
        assert!(stale < 0.0, "{stale}");
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        session.playback.set(playback);
        let status = MediaStatus {
            media_session_id: 9,
            player_state: "BUFFERING".to_owned(),
            idle_reason: None,
            current_time: 11.0,
        };
        assert!(session.apply(&status, None, Some(12.0), Instant::now()).is_ok());
        let edge = session.playback.get().edge;
        assert!((edge.episode_lag.expect("fresh") - 1.0).abs() < 1e-9);
        let next = edge.step(edge::Sample {
            lag: 1.0,
            buffering_for: Some(Duration::from_millis(600)),
            elapsed: Duration::from_millis(600),
        });
        assert!((next.target - 0.25).abs() < 1e-9);
        assert!((next.floor - 0.15).abs() < 1e-9);
    }

    #[test]
    fn queued_buffer_transitions_keep_the_time_between_them() {
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        session.playback.set(playback);
        let opened = Instant::now();
        let buffering = media_notice("BUFFERING", 10.0, Some(10.30), opened);
        let playing = media_notice("PLAYING", 10.0, Some(10.90), opened + Duration::from_millis(600));
        assert!(session.follow(&buffering, "session-1", "transport-1", 9).unwrap());
        assert!(session.follow(&playing, "session-1", "transport-1", 9).unwrap());
        let edge = session.playback.get().edge;
        assert!((edge.target - 0.30).abs() < 1e-9);
        assert!((edge.floor - 0.30).abs() < 1e-9);
        assert!(edge.episode_lag.is_none());
    }

    #[test]
    fn a_short_headroom_trims_more_slowly_instead_of_stopping() {
        let now = Instant::now();
        let plan = plan_trim(
            edge::Edge::default(),
            edge::Sample {
                lag: 0.41,
                buffering_for: None,
                elapsed: Duration::from_secs(4),
            },
            true,
        );
        assert!((plan.edge.target - 0.35).abs() < 1e-9);
        let asked = plan.command.expect("trim");
        assert!((asked - 1.08).abs() < 1e-9);
        let mut playback = paced(1.0, 0.41, now);
        playback.edge = plan.edge;
        assert!(begin_rate(&mut playback, asked, now).is_none());
        let slower = admit_rate(&playback, asked, now).expect("slower trim");
        assert!(slower > 1.0 + 1e-3 && slower < asked, "{slower}");
        let sent = drive_rate(&mut playback, slower, now, RateAnswer::Applied).expect("applied");
        assert_eq!(sent.len(), 1);
        assert!((sent[0] - slower).abs() < 1e-9);
        assert!((playback.edge.rate - slower).abs() < 1e-9);
        assert!(playback.retry.is_none());
        let arrived = playback.edge.step(edge::Sample {
            lag: playback.edge.target + 0.01,
            buffering_for: None,
            elapsed: Duration::from_millis(400),
        });
        assert!((arrived.rate - 1.0).abs() < 1e-9);
        let mut wide = paced(1.0, 1.0, now);
        wide.edge.target = 0.15;
        let kept = admit_rate(&wide, 1.5, now).expect("room");
        assert!((kept - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_playing_poll_between_queued_notices_keeps_a_short_episode() {
        // The loop has already applied BUFFERING. PLAYING is still queued.
        // A poll that returns in between must not close the episode at the
        // poll's later timestamp.
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        session.playback.set(playback);
        let opened = Instant::now();
        let buffering = media_notice("BUFFERING", 10.0, Some(10.30), opened + Duration::from_millis(50));
        let playing = media_notice("PLAYING", 10.05, Some(10.35), opened + Duration::from_millis(100));
        assert!(session.follow(&buffering, "session-1", "transport-1", 9).unwrap());
        let poll = MediaStatus {
            media_session_id: 9,
            player_state: "PLAYING".to_owned(),
            idle_reason: None,
            current_time: 10.2,
        };
        assert!(
            session
                .judge_polled_status(
                    &CastMedia {
                        session: "session-1",
                        transport: "transport-1",
                        media_session: 9,
                    },
                    &[playing],
                    &poll,
                    None,
                    opened + Duration::from_millis(720),
                )
                .unwrap()
        );
        let edge = session.playback.get().edge;
        assert!((edge.target - 0.25).abs() < 1e-9, "{}", edge.target);
        assert!((edge.floor - 0.15).abs() < 1e-9, "{}", edge.floor);
        assert!(session.buffering_since.get().is_none());
        assert!(edge.episode_lag.is_none());
    }

    #[test]
    fn a_buffering_poll_does_not_hide_an_earlier_queued_stall() {
        // Both notices are still queued when the poll returns. Applying the
        // BUFFERING poll first would open the timer at the poll and ignore
        // the earlier notice, so the 550 ms stall would miss backoff.
        let session = test_session();
        let mut playback = Playback::new();
        playback.edge.target = 0.25;
        playback.edge.floor = 0.15;
        session.playback.set(playback);
        let opened = Instant::now();
        let buffering = media_notice("BUFFERING", 10.0, Some(10.30), opened + Duration::from_millis(50));
        let playing = media_notice("PLAYING", 10.4, Some(10.9), opened + Duration::from_millis(600));
        let poll = MediaStatus {
            media_session_id: 9,
            player_state: "BUFFERING".to_owned(),
            idle_reason: None,
            current_time: 11.0,
        };
        assert!(
            session
                .judge_polled_status(
                    &CastMedia {
                        session: "session-1",
                        transport: "transport-1",
                        media_session: 9,
                    },
                    &[buffering, playing],
                    &poll,
                    Some(12.0),
                    opened + Duration::from_millis(720),
                )
                .unwrap()
        );
        let edge = session.playback.get().edge;
        assert!((edge.target - 0.30).abs() < 1e-9, "{}", edge.target);
        assert!((edge.floor - 0.30).abs() < 1e-9, "{}", edge.floor);
        assert!(session.buffering_since.get().is_some());
    }

    #[test]
    fn a_rejected_fast_trim_returns_to_normal_speed() {
        // 0.26 s at 1.5× with a 0.15 s target asks for 1.08×, but the existing
        // 1.5× leaves no room for that trim or a slower one. Stay at 1.0.
        let now = Instant::now();
        let mut playback = paced(1.5, 0.26, now);
        playback.edge.target = 0.15;
        playback.edge.floor = 0.15;
        assert!(admit_rate(&playback, 1.08, now).is_none());
        let rate = command_after_admission(&playback, 1.08, now).expect("restore");
        assert!((rate - 1.0).abs() < 1e-9);
        let sent = drive_rate(&mut playback, rate, now, RateAnswer::Applied).expect("applied");
        assert_eq!(sent, vec![1.0]);
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!((playback.threat - 1.0).abs() < 1e-9);
        assert!(playback.retry.is_none());

        let mut parked = paced(1.0, 0.1505, now);
        parked.edge.target = 0.15;
        assert!(command_after_admission(&parked, 1.08, now).is_none());
        let mut wide = paced(1.0, 1.0, now);
        wide.edge.target = 0.15;
        let kept = command_after_admission(&wide, 1.5, now).expect("room");
        assert!((kept - 1.5).abs() < 1e-9);
        let mut near = paced(1.0, 0.41, now);
        near.edge.target = 0.35;
        let slower = command_after_admission(&near, 1.08, now).expect("slower");
        assert!(slower > 1.0 + 1e-3 && slower < 1.08, "{slower}");
    }
    #[test]
    fn calm_playback_converges_through_all_targets_with_safe_admission() {
        let mut playback = Playback::new();
        let mut lag = 0.41;
        let mut now = Instant::now();
        let tick = Duration::from_millis(100);
        let mut commanded = Vec::new();
        for _ in 0..1000 {
            let plan = plan_trim(
                playback.edge,
                edge::Sample {
                    lag,
                    buffering_for: None,
                    elapsed: tick,
                },
                true,
            );
            playback.edge = plan.edge;
            playback.lag = Some(lag);
            playback.lag_at = Some(now);
            let wanted = if fastest(&playback) > 1.0 && status_wait(&playback, now).is_none() {
                Some(1.0)
            } else {
                plan.command
            };
            if let Some(wanted) = wanted
                && let Some(rate) = command_after_admission(&playback, wanted, now)
            {
                let wait = begin_rate(&mut playback, rate, now).expect("admitted");
                assert!(wait >= ACK_WINDOW);
                assert!(!apply_rate_reply(&mut playback, rate, 0, RateAnswer::Applied, now));
                commanded.push(rate);
            }
            lag -= (playback.edge.rate - 1.0) * tick.as_secs_f64();
            now += tick;
        }
        assert!(
            (playback.edge.target - edge::LIVE_EDGE).abs() < 1e-9,
            "{:?}",
            playback.edge
        );
        assert!((edge::LIVE_EDGE..=edge::LIVE_EDGE + 0.05).contains(&lag), "{lag}");
        assert!((playback.edge.rate - 1.0).abs() < 1e-9);
        assert!(commanded.iter().any(|rate| *rate > 1.0 && *rate < 1.08));
    }
}
