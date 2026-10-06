//! Media namespace of a launched application: load a URL and control playback.
use crate::{
    Application, CastClient, Error, Event, Result,
    client::{REQUEST_TIMEOUT, rejected},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::time::Duration;

pub(crate) const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamType {
    #[default]
    Buffered,
    Live,
}

impl StreamType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Buffered => "BUFFERED",
            Self::Live => "LIVE",
        }
    }
}

/// What to play. HLS segment formats are only needed when the playlist does
/// not let the receiver infer them.
#[derive(Clone, Debug, Default)]
pub struct MediaLoad {
    pub url: String,
    pub content_type: String,
    pub stream_type: StreamType,
    pub title: Option<String>,
    pub hls_segment_format: Option<String>,
    pub hls_video_segment_format: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatus {
    pub media_session_id: i64,
    #[serde(default)]
    pub player_state: String,
    pub idle_reason: Option<String>,
    #[serde(default)]
    pub current_time: f64,
}

impl MediaStatus {
    /// Media statuses carried by an unsolicited `MEDIA_STATUS` event.
    #[must_use]
    pub fn from_event(event: &Event) -> Vec<Self> {
        if event.namespace != NS_MEDIA || event.kind() != Some("MEDIA_STATUS") {
            return Vec::new();
        }
        statuses(&event.payload).unwrap_or_default()
    }
}

/// Controls media in one launched application.
pub struct MediaController<'a> {
    client: &'a CastClient,
    transport_id: String,
    media_session_id: Option<i64>,
}

impl CastClient {
    #[must_use]
    pub fn media(&self, app: &Application) -> MediaController<'_> {
        MediaController {
            client: self,
            transport_id: app.transport_id.clone(),
            media_session_id: None,
        }
    }
}

impl MediaController<'_> {
    /// Loads and autoplays `load`.
    /// # Errors
    /// Returns [`Error::Rejected`] with the receiver's reason if loading fails.
    pub fn load(&mut self, load: &MediaLoad) -> Result<MediaStatus> {
        let mut media = Map::new();
        media.insert("contentId".to_owned(), load.url.clone().into());
        media.insert("contentType".to_owned(), load.content_type.clone().into());
        media.insert("streamType".to_owned(), load.stream_type.as_str().into());
        if let Some(title) = &load.title {
            media.insert("metadata".to_owned(), json!({"metadataType": 0, "title": title}));
        }
        if let Some(format) = &load.hls_segment_format {
            media.insert("hlsSegmentFormat".to_owned(), format.clone().into());
        }
        if let Some(format) = &load.hls_video_segment_format {
            media.insert("hlsVideoSegmentFormat".to_owned(), format.clone().into());
        }
        let reply = self.request(json!({"type": "LOAD", "media": media, "autoplay": true}))?;
        let status = first_status(&reply)?;
        self.media_session_id = Some(status.media_session_id);
        Ok(status)
    }

    /// # Errors
    /// Returns an error if nothing is loaded or the receiver refuses.
    pub fn play(&self) -> Result<MediaStatus> {
        self.command("PLAY")
    }

    /// # Errors
    /// Returns an error if nothing is loaded or the receiver refuses.
    pub fn pause(&self) -> Result<MediaStatus> {
        self.command("PAUSE")
    }

    /// Stops playback; the receiver then reports an idle player.
    /// # Errors
    /// Returns an error if nothing is loaded or the receiver refuses.
    pub fn stop(&self) -> Result<MediaStatus> {
        self.command("STOP")
    }

    /// Plays faster or slower than real time; receivers accept about 0.5–2.
    /// # Errors
    /// Returns an error for a rate that is not positive and finite, if nothing
    /// is loaded, or if the receiver refuses.
    pub fn set_playback_rate(&self, rate: f64) -> Result<MediaStatus> {
        self.set_playback_rate_within(rate, REQUEST_TIMEOUT)
    }

    /// [`Self::set_playback_rate`] with a caller-chosen reply timeout.
    /// # Errors
    /// Returns an error for a rate that is not positive and finite, if nothing
    /// is loaded, if the receiver refuses, or if it does not answer in `timeout`.
    pub fn set_playback_rate_within(&self, rate: f64, timeout: Duration) -> Result<MediaStatus> {
        if !rate.is_finite() || rate <= 0.0 {
            return Err(Error::Protocol("playback rate must be positive and finite"));
        }
        let id = self
            .media_session_id
            .ok_or(Error::Protocol("no media session is loaded"))?;
        first_status(&self.request_within(
            json!({"type": "SET_PLAYBACK_RATE", "mediaSessionId": id, "playbackRate": rate}),
            timeout,
        )?)
    }

    /// Current media status, or `None` when nothing is loaded.
    /// # Errors
    /// Returns an error if the receiver does not answer.
    pub fn status(&self) -> Result<Option<MediaStatus>> {
        self.status_within(REQUEST_TIMEOUT)
    }

    /// [`Self::status`] with a caller-chosen reply timeout.
    /// # Errors
    /// Returns an error if the receiver does not answer in `timeout`.
    pub fn status_within(&self, timeout: Duration) -> Result<Option<MediaStatus>> {
        let reply = self.request_within(json!({"type": "GET_STATUS"}), timeout)?;
        Ok(statuses(&reply)?.into_iter().next())
    }

    fn command(&self, kind: &str) -> Result<MediaStatus> {
        let id = self
            .media_session_id
            .ok_or(Error::Protocol("no media session is loaded"))?;
        first_status(&self.request(json!({"type": kind, "mediaSessionId": id}))?)
    }

    fn request(&self, payload: Value) -> Result<Value> {
        self.request_within(payload, REQUEST_TIMEOUT)
    }

    fn request_within(&self, payload: Value, timeout: Duration) -> Result<Value> {
        self.client
            .request_within(&self.transport_id, NS_MEDIA, payload, timeout)
    }
}

fn statuses(reply: &Value) -> Result<Vec<MediaStatus>> {
    if reply.get("type").and_then(Value::as_str) != Some("MEDIA_STATUS") {
        return Err(rejected(reply));
    }
    let list = reply.get("status").cloned().unwrap_or_else(|| json!([]));
    Ok(Vec::<MediaStatus>::deserialize(list)?)
}

fn first_status(reply: &Value) -> Result<MediaStatus> {
    statuses(reply)?
        .into_iter()
        .next()
        .ok_or(Error::Protocol("media status without a session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_media_status_events() {
        let event = Event {
            namespace: NS_MEDIA.to_owned(),
            source: "t-1".to_owned(),
            payload: json!({
                "type": "MEDIA_STATUS",
                "requestId": 0,
                "status": [{"mediaSessionId": 7, "playerState": "PLAYING", "currentTime": 1.5}]
            }),
            media_time: None,
        };
        let status = MediaStatus::from_event(&event);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].media_session_id, 7);
        assert_eq!(status[0].player_state, "PLAYING");
    }

    #[test]
    fn maps_load_failures() {
        let failed = first_status(&json!({"type": "LOAD_FAILED", "detailedErrorCode": 104}));
        assert!(matches!(
            failed,
            Err(Error::Rejected { kind, reason: Some(reason) }) if kind == "LOAD_FAILED" && reason == "104"
        ));
        assert!(first_status(&json!({"type": "MEDIA_STATUS", "status": []})).is_err());
    }
}
