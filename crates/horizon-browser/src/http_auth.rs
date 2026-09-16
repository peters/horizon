//! Session HTTP Basic/Digest credentials and challenge decisions.

use std::collections::HashSet;

use horizon_browser_protocol::{
    parse_http_auth_origin, request_origin, validate_http_auth_password, validate_http_auth_username,
};

use crate::{BackendKind, BrowserControlFailure, BrowserHttpAuthOperation, SecretString};

const MAX_ATTEMPTED_REQUESTS: usize = 256;

#[derive(Clone, Debug, Default)]
pub(crate) struct HttpAuthState {
    credentials: Option<HttpAuthCredentials>,
    attempted_requests: HashSet<String>,
}

#[derive(Clone, Debug)]
struct HttpAuthCredentials {
    username: String,
    password: String,
    origin: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HttpAuthDecision {
    Provide { username: String, password: String },
    Cancel,
}

impl HttpAuthState {
    pub(crate) fn apply(
        &mut self,
        operation: BrowserHttpAuthOperation,
        username: Option<&str>,
        password: Option<&SecretString>,
        origin: Option<&str>,
    ) -> Result<(bool, Option<String>), BrowserControlFailure> {
        match operation {
            BrowserHttpAuthOperation::Set => {
                let username = username
                    .ok_or_else(|| BrowserControlFailure::new("invalid_action", "HTTP auth set requires username"))?;
                let password = password
                    .ok_or_else(|| BrowserControlFailure::new("invalid_action", "HTTP auth set requires password"))?;
                validate_http_auth_username(username)
                    .map_err(|message| BrowserControlFailure::new("invalid_action", message))?;
                validate_http_auth_password(password.as_str())
                    .map_err(|message| BrowserControlFailure::new("invalid_action", message))?;
                let origin = origin
                    .map(parse_http_auth_origin)
                    .transpose()
                    .map_err(|message| BrowserControlFailure::new("invalid_action", message))?;
                self.credentials = Some(HttpAuthCredentials {
                    username: username.to_string(),
                    password: password.as_str().to_string(),
                    origin: origin.clone(),
                });
                self.attempted_requests.clear();
                Ok((true, origin))
            }
            BrowserHttpAuthOperation::Clear => {
                if username.is_some() || password.is_some() || origin.is_some() {
                    return Err(BrowserControlFailure::new(
                        "invalid_action",
                        "HTTP auth clear does not accept credentials",
                    ));
                }
                self.credentials = None;
                self.attempted_requests.clear();
                Ok((false, None))
            }
        }
    }

    pub(crate) fn decide(
        &mut self,
        request_id: &str,
        url: &str,
        scheme: Option<&str>,
        is_proxy: bool,
    ) -> HttpAuthDecision {
        if request_id.is_empty() || is_proxy || !scheme_is_basic_or_digest(scheme) {
            return HttpAuthDecision::Cancel;
        }
        if self.attempted_requests.contains(request_id) {
            return HttpAuthDecision::Cancel;
        }
        let Some(credentials) = &self.credentials else {
            return HttpAuthDecision::Cancel;
        };
        let Some(origin) = request_origin(url) else {
            return HttpAuthDecision::Cancel;
        };
        if credentials.origin.as_ref().is_some_and(|expected| expected != &origin) {
            return HttpAuthDecision::Cancel;
        }
        if self.attempted_requests.len() >= MAX_ATTEMPTED_REQUESTS {
            self.attempted_requests.clear();
        }
        self.attempted_requests.insert(request_id.to_string());
        HttpAuthDecision::Provide {
            username: credentials.username.clone(),
            password: credentials.password.clone(),
        }
    }
}

pub(crate) fn http_auth_refusal(remote: bool, backend: BackendKind) -> Option<BrowserControlFailure> {
    if remote {
        return Some(BrowserControlFailure::new(
            "unsupported_backend",
            "HTTP authentication is unavailable for remote device sessions, which run on classic WebDriver",
        ));
    }
    if backend == BackendKind::SafariWebDriver {
        return Some(BrowserControlFailure::new(
            "unsupported_backend",
            "Safari does not expose HTTP authentication challenges through Horizon's current WebDriver transport",
        ));
    }
    None
}

pub(crate) fn scheme_is_basic_or_digest(scheme: Option<&str>) -> bool {
    scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic") || scheme.eq_ignore_ascii_case("digest"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(state: &mut HttpAuthState, origin: Option<&str>) {
        state
            .apply(
                BrowserHttpAuthOperation::Set,
                Some("smoke-user"),
                Some(&SecretString::new("smoke-pass-zephyr")),
                origin,
            )
            .expect("set");
    }

    #[test]
    fn matching_basic_challenge_is_provided_once_per_request() {
        let mut state = HttpAuthState::default();
        set(&mut state, Some("http://127.0.0.1:8080"));
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            HttpAuthDecision::Provide {
                username: "smoke-user".to_string(),
                password: "smoke-pass-zephyr".to_string(),
            }
        );
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            HttpAuthDecision::Cancel
        );
    }

    #[test]
    fn proxy_unknown_scheme_origin_mismatch_and_missing_credentials_cancel() {
        let mut state = HttpAuthState::default();
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("basic"), false),
            HttpAuthDecision::Cancel
        );
        set(&mut state, Some("http://127.0.0.1:8080"));
        assert_eq!(
            state.decide("req-2", "http://127.0.0.1:8080/basic-auth", Some("basic"), true),
            HttpAuthDecision::Cancel
        );
        assert_eq!(
            state.decide("req-3", "http://127.0.0.1:8080/basic-auth", Some("Negotiate"), false),
            HttpAuthDecision::Cancel
        );
        assert_eq!(
            state.decide("req-4", "http://example.test/basic-auth", Some("digest"), false),
            HttpAuthDecision::Cancel
        );
    }

    #[test]
    fn omitted_origin_matches_any_http_origin() {
        let mut state = HttpAuthState::default();
        set(&mut state, None);
        assert!(matches!(
            state.decide("req-5", "https://files.test/digest", Some("digest"), false),
            HttpAuthDecision::Provide { .. }
        ));
    }

    #[test]
    fn safari_and_remote_sessions_are_unsupported() {
        assert!(http_auth_refusal(true, BackendKind::FirefoxBidi).is_some());
        assert!(http_auth_refusal(false, BackendKind::SafariWebDriver).is_some());
        assert!(http_auth_refusal(false, BackendKind::ChromiumCdp).is_none());
        assert!(http_auth_refusal(false, BackendKind::FirefoxBidi).is_none());
    }
}
