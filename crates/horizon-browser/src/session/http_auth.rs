//! Chromium `CDP` HTTP Basic/Digest challenge handling.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::http_auth::{HttpAuthDecision, bind_http_auth_origin};
use crate::{BrowserControlAction, BrowserControlFailure, BrowserControlValue};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    pub(super) fn retire_http_auth_session(&mut self, link: &mut CdpLink) {
        if let Some(session) = self.session_id.as_deref()
            && let Err(error) = link.send_request("Fetch.disable", &json!({}), Some(session))
        {
            tracing::warn!(target: "browser", "Chromium could not retire authentication interception: {error}");
        }
        self.http_auth.reset_requests();
    }

    pub(super) fn http_auth_action(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        action: &BrowserControlAction,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let BrowserControlAction::HttpAuth {
            operation,
            username,
            password,
            origin,
        } = action
        else {
            return Err(BrowserControlFailure::new(
                "invalid_action_state",
                "HTTP auth action was not dispatched",
            ));
        };
        let session = self.session_id.clone().ok_or_else(|| {
            BrowserControlFailure::new("browser_unavailable", "the Chromium page session is not attached")
        })?;
        let origin = if matches!(*operation, crate::BrowserHttpAuthOperation::Set) {
            Some(bind_http_auth_origin(origin.as_deref(), &self.url)?)
        } else {
            None
        };
        self.http_auth
            .apply(*operation, username.as_deref(), password.as_ref(), origin.as_deref())?;
        let (method, params) = if self.http_auth.has_credentials() {
            ("Fetch.enable", fetch_enable_params())
        } else {
            ("Fetch.disable", json!({}))
        };
        if let Err(error) = self.call_and_ack(link, event_tx, frame_slot, method, &params, Some(&session)) {
            let _ = self
                .http_auth
                .apply(crate::BrowserHttpAuthOperation::Clear, None, None, None);
            return Err(BrowserControlFailure::new(
                "auth_protocol",
                format!("Chromium could not update authentication interception: {error}"),
            ));
        }
        Ok(BrowserControlValue::Accepted)
    }

    pub(super) fn continue_http_auth(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) {
        match event.method {
            "Fetch.requestPaused" => self.continue_paused_fetch(link, event_tx, frame_slot, event),
            "Fetch.authRequired" => self.continue_auth_challenge(link, event_tx, frame_slot, event),
            _ => {}
        }
    }

    fn continue_paused_fetch(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) {
        let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
            return;
        };
        if event
            .session_id
            .is_some_and(|session| Some(session) == self.session_id.as_deref())
            && let Some(network_id) = event.params.get("networkId").and_then(Value::as_str)
        {
            self.http_auth.note_network_id(request_id, network_id);
        }
        if let Err(error) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Fetch.continueRequest",
            &json!({ "requestId": request_id }),
            event.session_id,
        ) {
            tracing::warn!(target: "browser", "Chromium Fetch.continueRequest failed: {error}");
        }
    }

    fn continue_auth_challenge(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) {
        let Some(request_id) = event.params.get("requestId").and_then(Value::as_str) else {
            return;
        };
        let decision = self.http_auth_challenge_decision(event, request_id);
        let params = continue_with_auth_params(request_id, &decision);
        if let Err(error) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Fetch.continueWithAuth",
            &params,
            event.session_id,
        ) {
            tracing::warn!(target: "browser", "Chromium HTTP auth continue failed: {error}");
        }
    }

    fn http_auth_challenge_decision(&mut self, event: &CdpEvent<'_>, request_id: &str) -> HttpAuthDecision {
        // A replaced session can still deliver paused requests. Release them
        // without credentials or mutations to the current session's retry guard.
        if event
            .session_id
            .is_none_or(|session| Some(session) != self.session_id.as_deref())
        {
            return HttpAuthDecision::Cancel;
        }
        if let Some(network_id) = event.params.get("networkId").and_then(Value::as_str) {
            self.http_auth.note_network_id(request_id, network_id);
        }
        let url = event
            .params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scheme = event.params.pointer("/authChallenge/scheme").and_then(Value::as_str);
        let is_proxy = event
            .params
            .pointer("/authChallenge/source")
            .and_then(Value::as_str)
            .is_some_and(|source| source.eq_ignore_ascii_case("Proxy"));
        self.http_auth.decide(request_id, url, scheme, is_proxy)
    }

    pub(super) fn forget_completed_http_auth(&mut self, event: &CdpEvent<'_>) {
        if matches!(event.method, "Network.loadingFinished" | "Network.loadingFailed")
            && let Some(request_id) = event.params.get("requestId").and_then(Value::as_str)
        {
            self.http_auth.forget_request(request_id);
        }
    }
}

fn continue_with_auth_params(request_id: &str, decision: &HttpAuthDecision) -> Value {
    match decision {
        HttpAuthDecision::Provide { username, password } => json!({
            "requestId": request_id,
            "authChallengeResponse": {
                "response": "ProvideCredentials",
                "username": username,
                "password": password,
            }
        }),
        HttpAuthDecision::Cancel => json!({
            "requestId": request_id,
            "authChallengeResponse": { "response": "CancelAuth" }
        }),
    }
}

pub(super) fn fetch_enable_params() -> Value {
    // Chromium rejects handleAuthRequests without patterns, and an omitted
    // pattern list pauses every request. Match all URLs and continue each
    // paused request unchanged so only authRequired is interpreted.
    json!({
        "handleAuthRequests": true,
        "patterns": [{ "urlPattern": "*" }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_session_challenges_do_not_use_credentials_or_consume_current_request_ids() {
        let config = super::super::BrowserSessionConfig {
            browser: crate::BrowserConfig::default(),
            panel_local_id: "auth-test".into(),
            initial_url: None,
            width: 800,
            height: 600,
            frame_slot: Arc::default(),
            coordination: None,
            capture_directory: None,
            video: Arc::default(),
            remote: None,
        };
        let mut state = DriverState::new(
            &config,
            "ws://127.0.0.1/test",
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        state.session_id = Some("current".into());
        state
            .http_auth
            .apply(
                crate::BrowserHttpAuthOperation::Set,
                Some("user"),
                Some(&crate::SecretString::new("password")),
                Some("https://example.test"),
            )
            .expect("set auth");
        let params = json!({
            "requestId": "request",
            "networkId": "network",
            "request": { "url": "https://example.test/basic" },
            "authChallenge": { "scheme": "basic", "source": "Server" },
        });
        let mut event = CdpEvent {
            method: "Fetch.authRequired",
            params: &params,
            session_id: Some("old"),
        };
        assert_eq!(
            state.http_auth_challenge_decision(&event, "request"),
            HttpAuthDecision::Cancel
        );
        event.session_id = Some("current");
        assert!(matches!(
            state.http_auth_challenge_decision(&event, "request"),
            HttpAuthDecision::Provide { .. }
        ));
        event.session_id = Some("old");
        assert_eq!(
            state.http_auth_challenge_decision(&event, "request"),
            HttpAuthDecision::Cancel
        );
        event.session_id = Some("current");
        assert_eq!(
            state.http_auth_challenge_decision(&event, "request"),
            HttpAuthDecision::Cancel
        );
    }

    #[test]
    fn chromium_continue_payloads_cover_provide_and_cancel() {
        let provide = continue_with_auth_params(
            "req-1",
            &HttpAuthDecision::Provide {
                username: "smoke-user".into(),
                password: crate::SecretString::new("smoke-pass-zephyr"),
            },
        );
        assert_eq!(provide["authChallengeResponse"]["response"], "ProvideCredentials");
        assert_eq!(provide["authChallengeResponse"]["username"], "smoke-user");
        assert_eq!(provide["authChallengeResponse"]["password"], "smoke-pass-zephyr");
        let cancel = continue_with_auth_params("req-1", &HttpAuthDecision::Cancel);
        assert_eq!(cancel["authChallengeResponse"]["response"], "CancelAuth");
        assert!(cancel["authChallengeResponse"].get("password").is_none());
        assert_eq!(fetch_enable_params()["handleAuthRequests"], true);
        assert_eq!(fetch_enable_params()["patterns"][0]["urlPattern"], "*");
    }
}
