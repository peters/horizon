use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::RoutineError;

/// Exact browsing-context origin: scheme, host, and optional port.
///
/// Paths, userinfo, query, and fragment are not part of an origin. HTTPS is
/// required except for loopback HTTP fixtures.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Origin(String);

impl Origin {
    /// Parse a URL or origin string into a canonical origin.
    ///
    /// # Errors
    /// Returns [`RoutineError::InvalidOrigin`] when the input is not an
    /// allowed scheme/host, contains userinfo, or has no host.
    pub fn parse(input: &str) -> Result<Self, RoutineError> {
        let url = Url::parse(input.trim()).map_err(|_| RoutineError::InvalidOrigin)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RoutineError::InvalidOrigin);
        }
        let host = url.host_str().ok_or(RoutineError::InvalidOrigin)?;
        let scheme = url.scheme();
        let loopback = is_loopback_host(host);
        match scheme {
            "https" => {}
            "http" if loopback => {}
            _ => return Err(RoutineError::InvalidOrigin),
        }
        let host_part = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let mut encoded = format!("{scheme}://{host_part}");
        if let Some(port) = url.port() {
            encoded.push(':');
            encoded.push_str(&port.to_string());
        }
        Ok(Self(encoded))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Origin {
    type Err = RoutineError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl TryFrom<String> for Origin {
    type Error = RoutineError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Origin> for String {
    fn from(origin: Origin) -> Self {
        origin.0
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

#[cfg(test)]
mod tests {
    use super::Origin;
    use crate::RoutineError;

    #[test]
    fn https_origin_strips_path_and_query() {
        let origin = Origin::parse("https://reports.example/path?q=1#frag").expect("https");
        assert_eq!(origin.as_str(), "https://reports.example");
    }

    #[test]
    fn loopback_http_is_allowed_for_fixtures() {
        assert_eq!(
            Origin::parse("http://127.0.0.1:8080/login").expect("loopback").as_str(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            Origin::parse("http://localhost/").expect("localhost").as_str(),
            "http://localhost"
        );
    }

    #[test]
    fn remote_http_and_userinfo_are_rejected() {
        assert_eq!(Origin::parse("http://example.test"), Err(RoutineError::InvalidOrigin));
        assert_eq!(
            Origin::parse("https://user:name@example.test"),
            Err(RoutineError::InvalidOrigin)
        );
        assert_eq!(Origin::parse("javascript:alert(1)"), Err(RoutineError::InvalidOrigin));
    }

    #[test]
    fn origin_roundtrips_through_json() {
        let origin = Origin::parse("https://example.test:8443").expect("origin");
        let encoded = serde_json::to_string(&origin).expect("encode");
        assert_eq!(encoded, "\"https://example.test:8443\"");
        let decoded: Origin = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, origin);
    }
}
