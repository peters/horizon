//! Session HTTP Basic/Digest credentials and challenge decisions.

use std::collections::{HashMap, HashSet};

use horizon_browser_protocol::{
    parse_http_auth_origin, request_origin, validate_http_auth_password, validate_http_auth_username,
};

use crate::{BackendKind, BrowserControlFailure, BrowserHttpAuthOperation, SecretString};

const MAX_ATTEMPTED_REQUESTS: usize = 256;

#[derive(Clone, Debug, Default)]
pub(crate) struct HttpAuthState {
    credentials: Option<HttpAuthCredentials>,
    attempted_requests: HashSet<String>,
    fetch_network_ids: HashMap<String, String>,
    network_fetch_ids: HashMap<String, String>,
}

#[derive(Clone, Debug)]
struct HttpAuthCredentials {
    username: SecretString,
    password: SecretString,
    origin: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HttpAuthDecision {
    Provide {
        username: SecretString,
        password: SecretString,
    },
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
                    .ok_or_else(|| BrowserControlFailure::new("invalid_action", "HTTP auth set requires origin"))?;
                let origin = parse_http_auth_origin(origin)
                    .map_err(|message| BrowserControlFailure::new("invalid_action", message))?;
                self.credentials = Some(HttpAuthCredentials {
                    username: SecretString::new(username),
                    password: password.clone(),
                    origin: origin.clone(),
                });
                self.clear_attempts();
                Ok((true, Some(origin)))
            }
            BrowserHttpAuthOperation::Clear => {
                if username.is_some() || password.is_some() || origin.is_some() {
                    return Err(BrowserControlFailure::new(
                        "invalid_action",
                        "HTTP auth clear does not accept credentials",
                    ));
                }
                self.credentials = None;
                self.clear_attempts();
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
        if self.attempted_requests.contains(request_id) || self.attempted_requests.len() >= MAX_ATTEMPTED_REQUESTS {
            return HttpAuthDecision::Cancel;
        }
        let Some(credentials) = &self.credentials else {
            return HttpAuthDecision::Cancel;
        };
        let Some(origin) = request_origin(url) else {
            return HttpAuthDecision::Cancel;
        };
        if credentials.origin != origin {
            return HttpAuthDecision::Cancel;
        }
        let username = credentials.username.clone();
        let password = credentials.password.clone();
        self.remember_attempt(request_id);
        HttpAuthDecision::Provide { username, password }
    }

    pub(crate) fn note_network_id(&mut self, fetch_id: &str, network_id: &str) {
        if fetch_id.is_empty() || network_id.is_empty() {
            return;
        }
        if let Some(previous) = self
            .fetch_network_ids
            .insert(fetch_id.to_string(), network_id.to_string())
        {
            self.network_fetch_ids.remove(&previous);
        }
        if let Some(previous) = self
            .network_fetch_ids
            .insert(network_id.to_string(), fetch_id.to_string())
            && previous != fetch_id
        {
            self.fetch_network_ids.remove(&previous);
        }
    }

    pub(crate) fn forget_request(&mut self, request_id: &str) {
        let mut related = vec![request_id.to_string()];
        if let Some(network_id) = self.fetch_network_ids.remove(request_id) {
            self.network_fetch_ids.remove(&network_id);
            related.push(network_id);
        }
        if let Some(fetch_id) = self.network_fetch_ids.remove(request_id) {
            self.fetch_network_ids.remove(&fetch_id);
            related.push(fetch_id);
        }
        for id in related {
            self.attempted_requests.remove(&id);
        }
    }

    fn remember_attempt(&mut self, request_id: &str) {
        self.attempted_requests.insert(request_id.to_string());
    }

    fn clear_attempts(&mut self) {
        self.attempted_requests.clear();
        self.fetch_network_ids.clear();
        self.network_fetch_ids.clear();
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

pub(crate) fn bind_http_auth_origin(origin: Option<&str>, page_url: &str) -> Result<String, BrowserControlFailure> {
    match origin {
        Some(origin) => {
            parse_http_auth_origin(origin).map_err(|message| BrowserControlFailure::new("invalid_action", message))
        }
        None => request_origin(page_url).ok_or_else(|| {
            BrowserControlFailure::new(
                "invalid_action",
                "HTTP auth set requires origin when the current page has none",
            )
        }),
    }
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

    fn provide(username: &str, password: &str) -> HttpAuthDecision {
        HttpAuthDecision::Provide {
            username: SecretString::new(username),
            password: SecretString::new(password),
        }
    }

    #[test]
    fn set_without_origin_is_rejected() {
        let mut state = HttpAuthState::default();
        assert!(
            state
                .apply(
                    BrowserHttpAuthOperation::Set,
                    Some("smoke-user"),
                    Some(&SecretString::new("smoke-pass-zephyr")),
                    None,
                )
                .is_err()
        );
        assert_eq!(
            bind_http_auth_origin(None, "about:blank").expect_err("blank").code,
            "invalid_action"
        );
        assert_eq!(
            bind_http_auth_origin(None, "http://127.0.0.1:8080/basic-auth").expect("page"),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn matching_basic_challenge_is_provided_once_per_request() {
        let mut state = HttpAuthState::default();
        set(&mut state, Some("http://127.0.0.1:8080"));
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            provide("smoke-user", "smoke-pass-zephyr")
        );
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            HttpAuthDecision::Cancel
        );
        state.forget_request("req-1");
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            provide("smoke-user", "smoke-pass-zephyr")
        );
        state.note_network_id("req-1", "net-1");
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            HttpAuthDecision::Cancel
        );
        state.forget_request("net-1");
        assert_eq!(
            state.decide("req-1", "http://127.0.0.1:8080/basic-auth", Some("Basic"), false),
            provide("smoke-user", "smoke-pass-zephyr")
        );
    }

    #[test]
    fn active_attempt_cap_cancels_until_a_request_completes() {
        let mut state = HttpAuthState::default();
        set(&mut state, Some("http://example.test"));
        for index in 0..MAX_ATTEMPTED_REQUESTS {
            let request_id = format!("req-{index}");
            assert!(matches!(
                state.decide(&request_id, "http://example.test/basic", Some("basic"), false),
                HttpAuthDecision::Provide { .. }
            ));
        }
        assert_eq!(
            state.decide("req-0", "http://example.test/basic", Some("basic"), false),
            HttpAuthDecision::Cancel
        );
        assert_eq!(
            state.decide("req-cap", "http://example.test/basic", Some("basic"), false),
            HttpAuthDecision::Cancel
        );
        assert_eq!(
            state.decide("req-0", "http://example.test/basic", Some("basic"), false),
            HttpAuthDecision::Cancel
        );
        state.forget_request("req-0");
        assert!(matches!(
            state.decide("req-cap", "http://example.test/basic", Some("basic"), false),
            HttpAuthDecision::Provide { .. }
        ));
    }

    #[test]
    fn debug_does_not_echo_the_password() {
        let mut state = HttpAuthState::default();
        set(&mut state, Some("https://files.test"));
        let decision = state.decide("req-debug", "https://files.test/digest", Some("digest"), false);
        let rendered = format!("{state:?}{decision:?}");
        assert!(!rendered.contains("smoke-pass-zephyr"), "{rendered}");
        assert!(!rendered.contains("smoke-user"), "{rendered}");
        assert!(rendered.contains("SecretString(<redacted>)"), "{rendered}");
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
    fn credentials_are_not_provided_to_a_different_origin() {
        let mut state = HttpAuthState::default();
        set(&mut state, Some("http://127.0.0.1:8080"));
        assert_eq!(
            state.decide("req-5", "https://files.test/digest", Some("digest"), false),
            HttpAuthDecision::Cancel
        );
        assert!(matches!(
            state.decide("req-6", "http://127.0.0.1:8080/digest", Some("digest"), false),
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
