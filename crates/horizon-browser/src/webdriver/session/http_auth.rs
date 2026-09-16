//! Firefox `BiDi` HTTP Basic/Digest challenge handling.

use serde_json::{Value, json};

use crate::http_auth::{HttpAuthDecision, http_auth_refusal, scheme_is_basic_or_digest};
use crate::session::BrowserEventSender;
use crate::{BrowserControlAction, BrowserControlFailure, BrowserControlValue};

use super::Driver;

impl Driver {
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
        if let Some(refusal) = http_auth_refusal(self.host.is_remote(), self.config.browser.backend) {
            return Err(refusal);
        }
        self.http_auth
            .apply(*operation, username.as_deref(), password.as_ref(), origin.as_deref())?;
        Ok(BrowserControlValue::Accepted)
    }

    pub(super) fn continue_http_auth(&mut self, event: &Value, event_tx: &BrowserEventSender) {
        if event.get("method").and_then(Value::as_str) != Some("network.authRequired") {
            return;
        }
        let params = event.get("params").unwrap_or(&Value::Null);
        if params.get("isBlocked").and_then(Value::as_bool) == Some(false) {
            return;
        }
        let Some(request_id) = params.pointer("/request/request").and_then(Value::as_str) else {
            return;
        };
        let url = params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (scheme, is_proxy) = select_server_auth_challenge(params);
        let decision = self.http_auth.decide(request_id, url, scheme, is_proxy);
        let payload = continue_with_auth_params(request_id, &decision);
        if let Err(error) = self.call_bidi("network.continueWithAuth", &payload, event_tx) {
            tracing::warn!(target: "browser", "Firefox HTTP auth continue failed: {error}");
        }
    }

    pub(super) fn forget_completed_http_auth(&mut self, event: &Value) {
        if matches!(
            event.get("method").and_then(Value::as_str),
            Some("network.responseCompleted" | "network.fetchError")
        ) && let Some(request_id) = event.pointer("/params/request/request").and_then(Value::as_str)
        {
            self.http_auth.forget_request(request_id);
        }
    }
}

fn select_server_auth_challenge(params: &Value) -> (Option<&str>, bool) {
    if request_is_proxy(params) {
        return (None, true);
    }
    let Some(challenges) = params.pointer("/response/authChallenges").and_then(Value::as_array) else {
        return (None, false);
    };
    if challenges.iter().any(challenge_is_proxy) {
        return (None, true);
    }
    let scheme = challenges.iter().find_map(|challenge| {
        let scheme = challenge.get("scheme").and_then(Value::as_str)?;
        scheme_is_basic_or_digest(Some(scheme)).then_some(scheme)
    });
    (scheme, false)
}

fn challenge_is_proxy(challenge: &Value) -> bool {
    challenge
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.eq_ignore_ascii_case("proxy"))
}

fn request_is_proxy(params: &Value) -> bool {
    params
        .pointer("/request/method")
        .and_then(Value::as_str)
        .is_some_and(|method| method.eq_ignore_ascii_case("CONNECT"))
}

fn continue_with_auth_params(request_id: &str, decision: &HttpAuthDecision) -> Value {
    match decision {
        HttpAuthDecision::Provide { username, password } => json!({
            "request": request_id,
            "action": "provideCredentials",
            "credentials": {
                "type": "password",
                "username": username,
                "password": password,
            }
        }),
        HttpAuthDecision::Cancel => json!({
            "request": request_id,
            "action": "cancel"
        }),
    }
}

pub(super) fn firefox_http_auth_intercept_params(context: &str) -> Value {
    json!({
        "phases": ["authRequired"],
        "contexts": [context],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretString;

    #[test]
    fn firefox_continue_payloads_cover_provide_and_cancel() {
        let provide = continue_with_auth_params(
            "req-1",
            &HttpAuthDecision::Provide {
                username: "smoke-user".into(),
                password: SecretString::new("smoke-pass-zephyr"),
            },
        );
        assert_eq!(provide["action"], "provideCredentials");
        assert_eq!(provide["credentials"]["type"], "password");
        assert_eq!(provide["credentials"]["password"], "smoke-pass-zephyr");
        let cancel = continue_with_auth_params("req-1", &HttpAuthDecision::Cancel);
        assert_eq!(cancel["action"], "cancel");
        assert!(cancel.get("credentials").is_none());
        let intercept = firefox_http_auth_intercept_params("ctx");
        assert_eq!(intercept["phases"], json!(["authRequired"]));
        assert_eq!(intercept["contexts"], json!(["ctx"]));
    }

    #[test]
    fn firefox_picks_the_first_supported_server_scheme() {
        let params = json!({
            "request": { "method": "GET", "url": "http://example.test/basic" },
            "response": {
                "authChallenges": [
                    { "scheme": "Negotiate" },
                    { "scheme": "Basic", "realm": "files" }
                ]
            }
        });
        assert_eq!(select_server_auth_challenge(&params), (Some("Basic"), false));
    }

    #[test]
    fn firefox_treats_proxy_challenges_and_connect_as_proxy() {
        let proxy_challenge = json!({
            "request": { "method": "GET" },
            "response": { "authChallenges": [{ "scheme": "Basic", "source": "proxy" }] }
        });
        assert_eq!(select_server_auth_challenge(&proxy_challenge), (None, true));
        let connect = json!({
            "request": { "method": "CONNECT", "url": "https://example.test:443" },
            "response": { "authChallenges": [{ "scheme": "Basic" }] }
        });
        assert_eq!(select_server_auth_challenge(&connect), (None, true));
    }
}
