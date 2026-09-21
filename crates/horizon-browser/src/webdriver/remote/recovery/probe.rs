//! Exact-session evidence bound to the original provider and credential.
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{RemoteHttpClient, RemoteRecoveryStatus};
use crate::webdriver::transport::encode_path_segment;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(super) struct SessionProbe {
    transport: Arc<RemoteHttpClient>,
    session: String,
    report: Option<Arc<RemoteHttpClient>>,
}

impl SessionProbe {
    pub(super) fn new(
        transport: Arc<RemoteHttpClient>,
        session: String,
        report: Option<Arc<RemoteHttpClient>>,
    ) -> Self {
        Self {
            transport,
            session,
            report,
        }
    }

    pub(super) fn probe(&self) -> RemoteRecoveryStatus {
        let path = format!("/session/{}/url", self.session);
        let (status, body) = match response(&self.transport, &path, false) {
            Ok(response) => response,
            Err(status) => return status,
        };
        if status == 404 && body["value"]["error"] == "invalid session id" {
            return RemoteRecoveryStatus::Released;
        }
        if status == 200 && body["value"].is_string() {
            return RemoteRecoveryStatus::Active;
        }
        // Only the configured provider adapter may use reporting evidence.
        // Neither account-wide capacity nor generic hub errors prove absence.
        self.report
            .as_ref()
            .map_or(RemoteRecoveryStatus::UnsupportedResponse, |report| {
                self.probe_report(report)
            })
    }

    fn probe_report(&self, report: &RemoteHttpClient) -> RemoteRecoveryStatus {
        let segment = encode_path_segment(&self.session);
        if segment.is_empty() || self.session.contains(['/', '%', '\\']) {
            return RemoteRecoveryStatus::IdentityUnavailable;
        }
        let path = format!("/automate/sessions/{segment}.json");
        let (status, body) = match response(report, &path, true) {
            Ok(response) => response,
            Err(status) => return status,
        };
        if status != 200 {
            return RemoteRecoveryStatus::UnsupportedResponse;
        }
        let record = &body["automation_session"];
        if record["hashed_id"].as_str() != Some(self.session.as_str()) {
            return RemoteRecoveryStatus::UnsupportedResponse;
        }
        // `status` is user-editable test metadata. The provider's execution
        // status is separate; only its documented terminal values are proof.
        match record["browserstack_status"].as_str() {
            Some("done" | "timeout" | "error") => RemoteRecoveryStatus::Released,
            Some("running") => RemoteRecoveryStatus::Active,
            _ => RemoteRecoveryStatus::UnsupportedResponse,
        }
    }
}

fn response(transport: &RemoteHttpClient, path: &str, reporting: bool) -> Result<(u16, Value), RemoteRecoveryStatus> {
    let (status, bytes) = transport.request_bytes("GET", path, None, PROBE_TIMEOUT).map_err(|_| {
        tracing::warn!("remote recovery transport failed; capacity retained");
        RemoteRecoveryStatus::ProviderUnavailable
    })?;
    if matches!(status, 401 | 403) {
        return Err(RemoteRecoveryStatus::AuthenticationRequired);
    }
    if matches!(status, 502..=504) || (reporting && (500..=599).contains(&status)) {
        return Err(RemoteRecoveryStatus::ProviderUnavailable);
    }
    let body = serde_json::from_slice(&bytes).map_err(|_| {
        tracing::warn!(
            http_status = status,
            "remote recovery response was not JSON; capacity retained"
        );
        RemoteRecoveryStatus::UnsupportedResponse
    })?;
    // Raw URLs, provider identifiers, response bodies and error messages are
    // deliberately excluded from diagnostics.
    tracing::debug!(http_status = status, "remote recovery received a session response");
    Ok((status, body))
}
