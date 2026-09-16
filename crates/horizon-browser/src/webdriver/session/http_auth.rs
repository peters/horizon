//! Firefox `BiDi` HTTP Basic/Digest challenge handling.

use serde_json::{Value, json};

use crate::http_auth::{HttpAuthDecision, http_auth_refusal};
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
        let scheme = first_auth_scheme(params);
        let decision = self.http_auth.decide(request_id, url, scheme, false);
        let payload = continue_with_auth_params(request_id, &decision);
        if let Err(error) = self.call_bidi("network.continueWithAuth", &payload, event_tx) {
            tracing::warn!(target: "browser", "Firefox HTTP auth continue failed: {error}");
        }
    }
}

fn first_auth_scheme(params: &Value) -> Option<&str> {
    params
        .pointer("/response/authChallenges")
        .and_then(Value::as_array)
        .and_then(|challenges| challenges.first())
        .and_then(|challenge| challenge.get("scheme"))
        .and_then(Value::as_str)
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

    #[test]
    fn firefox_continue_payloads_cover_provide_and_cancel() {
        let provide = continue_with_auth_params(
            "req-1",
            &HttpAuthDecision::Provide {
                username: "smoke-user".into(),
                password: "smoke-pass-zephyr".into(),
            },
        );
        assert_eq!(provide["action"], "provideCredentials");
        assert_eq!(provide["credentials"]["type"], "password");
        let cancel = continue_with_auth_params("req-1", &HttpAuthDecision::Cancel);
        assert_eq!(cancel["action"], "cancel");
        assert!(cancel.get("credentials").is_none());
        let intercept = firefox_http_auth_intercept_params("ctx");
        assert_eq!(intercept["phases"], json!(["authRequired"]));
        assert_eq!(intercept["contexts"], json!(["ctx"]));
    }
}
