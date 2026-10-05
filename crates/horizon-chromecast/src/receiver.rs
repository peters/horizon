//! Platform receiver namespace: application launch, status and volume.
use crate::{
    CastClient, Error, Result,
    client::{PLATFORM_RECEIVER, rejected},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(crate) const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";

/// Application id of the Default Media Receiver, which plays a URL it is given.
pub const DEFAULT_MEDIA_RECEIVER: &str = "CC1AD845";

/// TV platforms can take well over ten seconds to start a web receiver from
/// their idle screen, and may first ask the viewer to allow the cast.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(45);
const LAUNCH_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverStatus {
    #[serde(default)]
    pub applications: Vec<Application>,
    pub volume: Option<Volume>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub app_id: String,
    #[serde(default)]
    pub display_name: String,
    pub session_id: String,
    pub transport_id: String,
    #[serde(default)]
    namespaces: Vec<Namespace>,
}

#[derive(Clone, Debug, Deserialize)]
struct Namespace {
    name: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Volume {
    pub level: Option<f64>,
    pub muted: Option<bool>,
}

impl Application {
    #[must_use]
    pub fn supports(&self, namespace: &str) -> bool {
        self.namespaces.iter().any(|entry| entry.name == namespace)
    }
}

impl ReceiverStatus {
    fn from_reply(reply: &Value) -> Result<Self> {
        if reply.get("type").and_then(Value::as_str) != Some("RECEIVER_STATUS") {
            return Err(rejected(reply));
        }
        let status = reply
            .get("status")
            .ok_or(Error::Protocol("receiver status without body"))?;
        Ok(Self::deserialize(status)?)
    }

    fn application(&self, app_id: &str) -> Option<&Application> {
        self.applications.iter().find(|app| app.app_id == app_id)
    }
}

impl CastClient {
    /// # Errors
    /// Returns an error if the receiver does not answer with its status.
    pub fn receiver_status(&self) -> Result<ReceiverStatus> {
        let reply = self.request(PLATFORM_RECEIVER, NS_RECEIVER, json!({"type": "GET_STATUS"}))?;
        ReceiverStatus::from_reply(&reply)
    }

    /// Launches `app_id`, or joins it if it is already running, and opens a
    /// virtual connection to its transport.
    /// # Errors
    /// Returns [`Error::Rejected`] if the receiver refuses the launch.
    pub fn launch(&self, app_id: &str) -> Result<Application> {
        // Some TV receivers restart an application on a repeated LAUNCH, which
        // ends the session the reply names. Join a running one instead.
        if let Some(app) = self.receiver_status()?.application(app_id).cloned() {
            self.connect_virtual(&app.transport_id)?;
            return Ok(app);
        }
        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        let reply = self.request_within(
            PLATFORM_RECEIVER,
            NS_RECEIVER,
            json!({"type": "LAUNCH", "appId": app_id}),
            LAUNCH_TIMEOUT,
        )?;
        let mut status = ReceiverStatus::from_reply(&reply)?;
        // Some receivers acknowledge LAUNCH before the application reports a
        // transport; keep polling within the same launch allowance.
        let app = loop {
            if let Some(app) = status.application(app_id) {
                break app.clone();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout("LAUNCH".to_owned()));
            }
            std::thread::sleep(LAUNCH_POLL_INTERVAL.min(remaining));
            let reply = self.request_within(
                PLATFORM_RECEIVER,
                NS_RECEIVER,
                json!({"type": "GET_STATUS"}),
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(LAUNCH_POLL_INTERVAL),
            )?;
            status = ReceiverStatus::from_reply(&reply)?;
        };
        self.connect_virtual(&app.transport_id)?;
        Ok(app)
    }

    /// Stops the application session `session_id`.
    /// # Errors
    /// Returns an error if the receiver refuses or does not answer.
    pub fn stop_application(&self, session_id: &str) -> Result<ReceiverStatus> {
        let reply = self.request(
            PLATFORM_RECEIVER,
            NS_RECEIVER,
            json!({"type": "STOP", "sessionId": session_id}),
        )?;
        ReceiverStatus::from_reply(&reply)
    }

    /// Sets the receiver volume, clamped to `0.0..=1.0`.
    /// # Errors
    /// Returns a protocol error for `NaN`, or an error if the receiver refuses
    /// or does not answer.
    pub fn set_volume(&self, level: f64) -> Result<ReceiverStatus> {
        if level.is_nan() {
            return Err(Error::Protocol("volume level is not a number"));
        }
        let reply = self.request(
            PLATFORM_RECEIVER,
            NS_RECEIVER,
            json!({"type": "SET_VOLUME", "volume": {"level": level.clamp(0.0, 1.0)}}),
        )?;
        ReceiverStatus::from_reply(&reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_rejections() {
        let reply = json!({
            "type": "RECEIVER_STATUS",
            "requestId": 3,
            "status": {
                "applications": [{
                    "appId": "CC1AD845",
                    "displayName": "Default Media Receiver",
                    "sessionId": "s-1",
                    "transportId": "t-1",
                    "namespaces": [{"name": "urn:x-cast:com.google.cast.media"}]
                }],
                "volume": {"level": 0.25, "muted": false}
            }
        });
        let status = ReceiverStatus::from_reply(&reply).unwrap();
        let app = status.application(DEFAULT_MEDIA_RECEIVER).unwrap();
        assert_eq!(app.transport_id, "t-1");
        assert!(app.supports("urn:x-cast:com.google.cast.media"));
        assert_eq!(status.volume.and_then(|v| v.level), Some(0.25));

        let error = ReceiverStatus::from_reply(&json!({"type": "LAUNCH_ERROR", "reason": "NOT_FOUND"}));
        assert!(matches!(
            error,
            Err(Error::Rejected { kind, reason: Some(reason) }) if kind == "LAUNCH_ERROR" && reason == "NOT_FOUND"
        ));
    }
}
