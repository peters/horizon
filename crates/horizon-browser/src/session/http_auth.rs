//! Chromium `CDP` HTTP Basic/Digest challenge handling.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::http_auth::{HttpAuthDecision, bind_http_auth_origin};
use crate::{BrowserControlAction, BrowserControlFailure, BrowserControlValue};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    fn http_auth_sessions(&self) -> Vec<String> {
        self.session_id
            .iter()
            .chain(self.clipboard.iframe_sessions.iter())
            .cloned()
            .collect()
    }

    fn is_http_auth_session(&self, session: &str) -> bool {
        self.session_id.is_some()
            && (self.session_id.as_deref() == Some(session) || self.clipboard.iframe_sessions.contains(session))
    }

    fn disable_http_auth_interception(&self, link: &mut CdpLink) {
        for session in self.http_auth_sessions() {
            if let Err(error) = link.send_request("Fetch.disable", &json!({}), Some(&session)) {
                tracing::warn!(target: "browser", "Chromium could not retire authentication interception: {error}");
            }
        }
    }

    pub(super) fn retire_http_auth_session(&mut self, link: &mut CdpLink) {
        self.disable_http_auth_interception(link);
        self.http_auth.reset_requests();
    }

    pub(super) fn attach_http_auth_iframe(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) {
        let Some(session) = super::clipboard::target_event_session_id(event.params, event.session_id) else {
            return;
        };
        if !self.http_auth.should_intercept() || !self.is_http_auth_session(session) {
            return;
        }
        for (method, params) in [("Network.enable", json!({})), ("Fetch.enable", fetch_enable_params())] {
            if !self.is_http_auth_session(session) {
                break;
            }
            if let Err(error) = self.call_and_ack(link, event_tx, frame_slot, method, &params, Some(session)) {
                tracing::warn!(target: "browser", "Chromium iframe authentication setup failed: {error}");
                break;
            }
        }
    }

    pub(super) fn forget_http_auth_session(&mut self, session: &str) {
        self.http_auth.forget_requests_with_prefix(&request_scope(session));
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
        self.session_id.as_ref().ok_or_else(|| {
            BrowserControlFailure::new("browser_unavailable", "the Chromium page session is not attached")
        })?;
        let origin = if matches!(*operation, crate::BrowserHttpAuthOperation::Set) {
            Some(bind_http_auth_origin(origin.as_deref(), &self.url)?)
        } else {
            None
        };
        let previous = self.http_auth.configuration();
        self.http_auth
            .apply(*operation, username.as_deref(), password.as_ref(), origin.as_deref())?;
        if let Err(error) = self.reconcile_http_auth_sessions(link, event_tx, frame_slot) {
            self.http_auth.restore_configuration(previous);
            if let Err(rollback_error) = self.reconcile_http_auth_sessions(link, event_tx, frame_slot) {
                let _ = self
                    .http_auth
                    .apply(crate::BrowserHttpAuthOperation::Clear, None, None, None);
                self.disable_http_auth_interception(link);
                return Err(BrowserControlFailure::new(
                    "auth_protocol",
                    format!(
                        "Chromium authentication setup failed: {error}; restoring interception also failed: {rollback_error}; credentials were cleared"
                    ),
                ));
            }
            return Err(BrowserControlFailure::new(
                "auth_protocol",
                format!("Chromium could not update authentication interception: {error}"),
            ));
        }
        Ok(BrowserControlValue::Accepted)
    }

    fn reconcile_http_auth_sessions(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) -> Result<(), crate::cdp::CdpError> {
        let (method, params) = if self.http_auth.should_intercept() {
            ("Fetch.enable", fetch_enable_params())
        } else {
            ("Fetch.disable", json!({}))
        };
        self.update_http_auth_sessions(link, event_tx, frame_slot, method, &params)
    }

    fn update_http_auth_sessions(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        method: &str,
        params: &Value,
    ) -> Result<(), crate::cdp::CdpError> {
        for session in self.http_auth_sessions() {
            let commands = [("Network.enable", json!({})), (method, params.clone())];
            for (command, arguments) in commands {
                if command == "Network.enable"
                    && (method == "Fetch.disable" || self.session_id.as_deref() == Some(&session))
                {
                    continue;
                }
                if !self.is_http_auth_session(&session) {
                    break;
                }
                if let Err(error) = self.call_and_ack(link, event_tx, frame_slot, command, &arguments, Some(&session))
                    && self.is_http_auth_session(&session)
                {
                    return Err(error);
                }
            }
        }
        Ok(())
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
        if let Some(session) = event.session_id.filter(|session| self.is_http_auth_session(session))
            && let Some(network_id) = event.params.get("networkId").and_then(Value::as_str)
            && !request_id.is_empty()
            && !network_id.is_empty()
        {
            self.http_auth.note_network_id(
                &scoped_request(session, request_id),
                &scoped_request(session, network_id),
            );
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
        let Some(session) = event.session_id.filter(|session| self.is_http_auth_session(session)) else {
            return HttpAuthDecision::Cancel;
        };
        if request_id.is_empty() {
            return HttpAuthDecision::Cancel;
        }
        let request_id = scoped_request(session, request_id);
        if let Some(network_id) = event
            .params
            .get("networkId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.http_auth
                .note_network_id(&request_id, &scoped_request(session, network_id));
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
        self.http_auth.decide(&request_id, url, scheme, is_proxy)
    }

    pub(super) fn forget_completed_http_auth(&mut self, event: &CdpEvent<'_>) {
        if matches!(event.method, "Network.loadingFinished" | "Network.loadingFailed")
            && let Some(session) = event.session_id.filter(|session| self.is_http_auth_session(session))
            && let Some(request_id) = event.params.get("requestId").and_then(Value::as_str)
            && !request_id.is_empty()
        {
            self.http_auth.forget_request(&scoped_request(session, request_id));
        }
    }
}

fn request_scope(session: &str) -> String {
    format!("{}:{session}:", session.len())
}

fn scoped_request(session: &str, request: &str) -> String {
    format!("{}{request}", request_scope(session))
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

    fn authenticated_state() -> DriverState {
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
        state
    }

    #[test]
    fn failed_first_set_restores_disabled_interception() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .expect("timeout");
            let mut socket = tungstenite::accept(stream).expect("handshake");
            for (index, method) in ["Fetch.enable", "Fetch.disable"].into_iter().enumerate() {
                let message = socket.read().expect("command");
                let command: Value = serde_json::from_str(message.to_text().expect("text")).expect("JSON");
                assert_eq!(command["method"], method);
                assert_eq!(command["sessionId"], "current");
                let response = if index == 0 {
                    json!({ "id": command["id"], "error": {"code": -32000, "message": "mock failure"} })
                } else {
                    json!({ "id": command["id"], "result": {} })
                };
                socket
                    .send(tungstenite::Message::Text(response.to_string().into()))
                    .expect("respond");
            }
        });
        let mut state = authenticated_state();
        state.http_auth = crate::http_auth::HttpAuthState::default();
        let mut link = CdpLink::connect(&format!("ws://{address}")).expect("connect");
        let (tx, _rx) = std::sync::mpsc::channel();
        let events = BrowserEventSender {
            tx,
            wake: super::super::BrowserEventWake::default(),
            committed_url: super::super::CommittedUrl::default(),
        };
        let result = state.http_auth_action(
            &mut link,
            &events,
            &Arc::default(),
            &BrowserControlAction::HttpAuth {
                operation: crate::BrowserHttpAuthOperation::Set,
                username: Some("user".into()),
                password: Some("password".into()),
                origin: Some("https://example.test".into()),
            },
        );
        assert!(result.is_err());
        assert!(!state.http_auth.should_intercept());
        assert_eq!(
            state
                .http_auth
                .decide("new", "https://example.test", Some("basic"), false),
            HttpAuthDecision::Cancel
        );
        drop(link);
        server.join().expect("mock completed");
    }

    #[test]
    fn stale_session_challenges_do_not_use_credentials_or_consume_current_request_ids() {
        let mut state = authenticated_state();
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
    fn iframe_attempts_and_completion_are_scoped_to_their_session() {
        let mut state = authenticated_state();
        state.clipboard.iframe_sessions.insert("child".into());
        let params = json!({
            "networkId": "network",
            "request": { "url": "https://example.test/basic" },
            "authChallenge": { "scheme": "basic", "source": "Server" },
        });
        for session in ["current", "child"] {
            let event = CdpEvent {
                method: "Fetch.authRequired",
                params: &params,
                session_id: Some(session),
            };
            assert_eq!(state.http_auth_challenge_decision(&event, ""), HttpAuthDecision::Cancel);
            assert!(matches!(
                state.http_auth_challenge_decision(&event, "request"),
                HttpAuthDecision::Provide { .. }
            ));
        }
        let completed = json!({ "requestId": "network" });
        state.forget_completed_http_auth(&CdpEvent {
            method: "Network.loadingFinished",
            params: &completed,
            session_id: Some("child"),
        });
        let child = CdpEvent {
            method: "Fetch.authRequired",
            params: &params,
            session_id: Some("child"),
        };
        assert!(matches!(
            state.http_auth_challenge_decision(&child, "request"),
            HttpAuthDecision::Provide { .. }
        ));
        state.forget_http_auth_session("child");
        state.clipboard.iframe_sessions.remove("child");
        assert_eq!(
            state.http_auth_challenge_decision(&child, "request"),
            HttpAuthDecision::Cancel
        );
        state.clipboard.iframe_sessions.insert("child".into());
        assert!(matches!(
            state.http_auth_challenge_decision(&child, "request"),
            HttpAuthDecision::Provide { .. }
        ));
        let parent = CdpEvent {
            session_id: Some("current"),
            ..child
        };
        assert_eq!(
            state.http_auth_challenge_decision(&parent, "request"),
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
