//! Chromium `CDP` HTTP Basic/Digest challenge handling.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::http_auth::HttpAuthDecision;
use crate::{BrowserControlAction, BrowserControlFailure, BrowserControlValue};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    pub(super) fn http_auth_action(
        &mut self,
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
        self.http_auth
            .apply(*operation, username.as_deref(), password.as_ref(), origin.as_deref())?;
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
        let decision = self.http_auth.decide(request_id, url, scheme, is_proxy);
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
    fn chromium_continue_payloads_cover_provide_and_cancel() {
        let provide = continue_with_auth_params(
            "req-1",
            &HttpAuthDecision::Provide {
                username: "smoke-user".into(),
                password: "smoke-pass-zephyr".into(),
            },
        );
        assert_eq!(provide["authChallengeResponse"]["response"], "ProvideCredentials");
        assert_eq!(provide["authChallengeResponse"]["username"], "smoke-user");
        let cancel = continue_with_auth_params("req-1", &HttpAuthDecision::Cancel);
        assert_eq!(cancel["authChallengeResponse"]["response"], "CancelAuth");
        assert!(cancel["authChallengeResponse"].get("password").is_none());
        assert_eq!(fetch_enable_params()["handleAuthRequests"], true);
        assert_eq!(fetch_enable_params()["patterns"][0]["urlPattern"], "*");
    }
}
