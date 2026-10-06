//! Live H.264 casting: the host pushes encoded access units, this module
//! serves them on the LAN and keeps the Default Media Receiver playing them,
//! either as one progressive fragmented MP4 stream (low latency) or as HLS.
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
    StreamType, client::NS_CONNECTION, media::NS_MEDIA, receiver::NS_RECEIVER,
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
/// Progressive playback starts a few seconds behind; above this the session
/// speeds playback up until it is back near `TARGET_LAG`.
const CATCH_UP_ABOVE: f64 = 1.0;
const TARGET_LAG: f64 = 0.4;
const CATCH_UP_RATE: f64 = 1.5;
const LAG_CHECK: Duration = Duration::from_secs(2);
/// Attempts to return to normal speed before the session fails, so a receiver
/// is never left running fast.
const RESTORE_ATTEMPTS: u8 = 3;
const RESTORE_RETRY: Duration = Duration::from_millis(500);
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_RETRY: Duration = Duration::from_secs(2);

/// How the stream reaches the receiver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    /// One endless fragmented MP4 response; about half a second behind live
    /// once the session has caught up.
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
                catch_up: Cell::new(CatchUp::Watching),
                next_lag_check: Cell::new(Instant::now()),
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

    /// Adds one Annex B access unit with its presentation time. Input must be
    /// in presentation order (no B-frames): units whose timestamp goes
    /// backwards are dropped, since segments carry no separate decode time.
    /// Adds one raw AAC-LC frame (no ADTS header) for the audio track declared
    /// in [`LiveOptions::audio`], timed on the same clock as the video.
    /// Ignored without a declared audio track.
    pub fn push_aac(&self, frame: &[u8], pts: Duration) {
        if let Sink::Progressive(stream) = &self.sink {
            stream.push_audio(frame, pts);
        }
    }

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
    catch_up: Cell<CatchUp>,
    next_lag_check: Cell<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CatchUp {
    Watching,
    /// Playing fast until this instant; then returning to normal speed, with
    /// the failed attempts so far.
    Until(Instant, u8),
    /// The receiver refused a playback rate; leave it at normal speed.
    Unsupported,
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
    /// far playback trails the newest frame and briefly play faster to trim it.
    /// # Errors
    /// Fails when playback cannot be returned to normal speed.
    fn keep_up(&self, media: &MediaController<'_>) -> Result<()> {
        let Sink::Progressive(stream) = &self.sink else {
            return Ok(());
        };
        let now = Instant::now();
        match self.catch_up.get() {
            CatchUp::Until(until, failures) if now >= until => {
                let restored = media.set_playback_rate(1.0);
                let Some(next) = after_restore(restored.is_ok(), failures, now) else {
                    return restored.map(|_| ());
                };
                self.catch_up.set(next);
                self.next_lag_check.set(now + LAG_CHECK);
                return Ok(());
            }
            CatchUp::Until(..) | CatchUp::Unsupported => return Ok(()),
            CatchUp::Watching => {}
        }
        if !self.started.get() || now < self.next_lag_check.get() || *lock(&self.state) != LiveState::Playing {
            return Ok(());
        }
        self.next_lag_check.set(now + LAG_CHECK);
        let (Some(newest), Ok(Some(status))) = (stream.newest_media_time(), media.status()) else {
            return Ok(());
        };
        let lag = newest - status.current_time;
        if lag > CATCH_UP_ABOVE && status.player_state == "PLAYING" {
            // The receiver reports the time: ignore one too far off to represent.
            let Some(until) = Duration::try_from_secs_f64((lag - TARGET_LAG) / (CATCH_UP_RATE - 1.0))
                .ok()
                .and_then(|catch_up| now.checked_add(catch_up))
            else {
                return Ok(());
            };
            tracing::debug!(lag, "speeding up live playback to catch up");
            self.catch_up.set(match media.set_playback_rate(CATCH_UP_RATE) {
                Ok(_) => CatchUp::Until(until, 0),
                // Only an explicit refusal proves the rate was not applied.
                Err(Error::Rejected { .. }) => CatchUp::Unsupported,
                // The rate may have applied before the reply was lost: restore now.
                Err(_) => CatchUp::Until(now, 0),
            });
        }
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
        catch_up: Cell::new(CatchUp::Watching),
        next_lag_check: Cell::new(Instant::now()),
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
}
