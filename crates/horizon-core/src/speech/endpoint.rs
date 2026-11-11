//! The endpoint of a transcription service, and the rule that keeps one take off
//! a plain-text link.
//!
//! A person types the endpoint in the Settings tab "Speech", so it is untrusted
//! text. It must be one absolute URL: `https`, or `http` on a loopback address for
//! a stand-in service and a test. The recording and the key of the take travel to
//! the host of this URL, so an endpoint of another host is the decision of the
//! person and the documents say so.

use std::fmt;

use url::{Host, Url};

/// One endpoint of a transcription service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    url: Url,
}

/// Why a text is not an endpoint of a transcription service.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    #[error("the endpoint is empty")]
    Empty,
    #[error("the endpoint is not an absolute URL")]
    Malformed,
    #[error("the endpoint uses {0}, and a take leaves this computer over https")]
    Scheme(String),
    #[error("the endpoint uses http on {0}, and http is only for an address of this computer")]
    PlainTextNotLoopback(String),
    #[error("the endpoint holds a user name or a password, which a take never sends")]
    Credentials,
}

impl Endpoint {
    /// `text` as the endpoint of one transcription service.
    ///
    /// # Errors
    /// Reports an empty text, a text that is not one absolute URL, a scheme other
    /// than `https` or `http`, `http` on a host that is not a loopback address, and
    /// a URL that holds a user name or a password.
    pub fn parse(text: &str) -> Result<Self, EndpointError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(EndpointError::Empty);
        }
        let url = Url::parse(trimmed).map_err(|_| EndpointError::Malformed)?;
        match url.scheme() {
            "https" => {}
            "http" => {
                let host = url.host().ok_or(EndpointError::Malformed)?;
                if !is_loopback(&host) {
                    return Err(EndpointError::PlainTextNotLoopback(host.to_string()));
                }
            }
            scheme => return Err(EndpointError::Scheme(scheme.to_owned())),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::Credentials);
        }
        Ok(Self { url })
    }

    /// The URL without its `#fragment`, as the request sends it.
    #[must_use]
    pub fn request_url(&self) -> String {
        let mut url = self.url.clone();
        url.set_fragment(None);
        url.to_string()
    }

    /// The host of the endpoint, for the line of the Settings tab.
    #[must_use]
    pub fn host(&self) -> String {
        self.url.host_str().unwrap_or_default().to_owned()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.request_url().as_str())
    }
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, EndpointError};

    #[test]
    fn the_default_service_is_one_https_endpoint() {
        let endpoint = Endpoint::parse("https://api.openai.com/v1/audio/transcriptions").expect("an endpoint");
        assert_eq!(endpoint.host(), "api.openai.com");
        assert_eq!(endpoint.request_url(), "https://api.openai.com/v1/audio/transcriptions");
    }

    #[test]
    fn http_is_accepted_on_a_loopback_address_only() {
        for loopback in [
            "http://127.0.0.1:8080/v1/audio/transcriptions",
            "http://localhost:8080/v1",
            "http://[::1]:8080/v1",
            "http://127.1.2.3/v1",
        ] {
            Endpoint::parse(loopback).unwrap_or_else(|error| panic!("{loopback}: {error}"));
        }

        for outside in [
            "http://api.openai.com/v1",
            "http://192.168.1.4/v1",
            "http://example.test/v1",
            "http://0.0.0.0/v1",
        ] {
            let error = Endpoint::parse(outside).expect_err("a plain link leaves this computer");
            assert!(
                matches!(error, EndpointError::PlainTextNotLoopback(_)),
                "{outside}: {error}"
            );
        }
    }

    #[test]
    fn another_scheme_and_a_loose_text_are_refused() {
        assert_eq!(Endpoint::parse(""), Err(EndpointError::Empty));
        assert_eq!(Endpoint::parse("   "), Err(EndpointError::Empty));
        assert_eq!(Endpoint::parse("api.openai.com/v1"), Err(EndpointError::Malformed));
        assert_eq!(
            Endpoint::parse("ftp://api.openai.com/v1"),
            Err(EndpointError::Scheme("ftp".to_owned()))
        );
        assert_eq!(
            Endpoint::parse("file:///etc/passwd"),
            Err(EndpointError::Scheme("file".to_owned()))
        );
    }

    #[test]
    fn an_endpoint_that_holds_a_credential_is_refused() {
        assert_eq!(
            Endpoint::parse("https://a-person:a-value@api.openai.com/v1"),
            Err(EndpointError::Credentials)
        );
        assert_eq!(
            Endpoint::parse("https://sk-a-value@api.openai.com/v1"),
            Err(EndpointError::Credentials)
        );
    }

    #[test]
    fn the_fragment_never_reaches_the_request() {
        let endpoint = Endpoint::parse("https://api.openai.com/v1#section").expect("an endpoint");
        assert_eq!(endpoint.request_url(), "https://api.openai.com/v1");
    }
}
