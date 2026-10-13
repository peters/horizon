//! Host-side origin and relying-party rules for the `WebAuthn` bridge.
//!
//! The origin always comes from the browser's record of the calling frame
//! (CDP frame tree, `BiDi` realm), never from the page message.

use super::{Operation, PageError};

/// The origin of a calling frame that passed the bridge's origin rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallerOrigin {
    serialized: String,
    host: String,
}

impl CallerOrigin {
    /// Accept `https://<domain>[:port]` and `http://localhost[:port]` only.
    /// IP literals, opaque origins and every other scheme are refused.
    pub(crate) fn parse(origin: &str) -> Result<Self, PageError> {
        let (scheme, authority) = origin.split_once("://").ok_or_else(PageError::insecure_origin)?;
        if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
            return Err(PageError::insecure_origin());
        }
        let host = match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
            Some(_) => return Err(PageError::insecure_origin()),
            None => authority,
        };
        if !is_domain(host) || is_ip_literal(host) {
            return Err(PageError::insecure_origin());
        }
        match scheme {
            "https" => {}
            "http" if host == "localhost" => {}
            _ => return Err(PageError::insecure_origin()),
        }
        Ok(Self {
            serialized: origin.to_string(),
            host: host.to_string(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.serialized
    }

    /// The relying-party ID: the origin's host by default, or a requested
    /// ID that equals the host or is a parent domain of it. A single-label
    /// parent (`com`) is refused. Horizon has no public suffix list, so a
    /// multi-label public suffix such as `co.uk` is not detected here.
    pub(crate) fn rp_id(&self, requested: Option<&str>) -> Result<String, PageError> {
        let Some(requested) = requested else {
            return Ok(self.host.clone());
        };
        let rp_id = requested.to_ascii_lowercase();
        if rp_id == self.host {
            return Ok(rp_id);
        }
        let is_parent = is_domain(&rp_id)
            && !is_ip_literal(&rp_id)
            && rp_id.contains('.')
            && self
                .host
                .strip_suffix(rp_id.as_str())
                .is_some_and(|prefix| prefix.ends_with('.'));
        if is_parent {
            Ok(rp_id)
        } else {
            Err(PageError::foreign_rp_id())
        }
    }
}

/// Build `clientDataJSON` in the member order that the `WebAuthn`
/// specification serializes. Cross-origin callers are refused before this
/// point, so `crossOrigin` is always `false`.
pub(crate) fn client_data_json(operation: Operation, challenge: &str, origin: &CallerOrigin) -> Vec<u8> {
    let kind = match operation {
        Operation::Get => "webauthn.get",
        Operation::Create => "webauthn.create",
    };
    let origin = serde_json::Value::String(origin.serialized.clone());
    format!(r#"{{"type":"{kind}","challenge":"{challenge}","origin":{origin},"crossOrigin":false}}"#).into_bytes()
}

fn is_domain(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// A numeric last label marks an IPv4 address; no valid domain has one.
fn is_ip_literal(host: &str) -> bool {
    host.rsplit('.')
        .next()
        .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_domains_and_http_localhost_are_accepted() {
        for accepted in [
            "https://example.com",
            "https://login.example.com:8443",
            "http://localhost",
            "http://localhost:3000",
            "https://localhost:8443",
        ] {
            assert!(CallerOrigin::parse(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "http://example.com",
            "http://127.0.0.1:3000",
            "http://sub.localhost",
            "https://127.0.0.1",
            "https://[::1]:8443",
            "https://10.0.0.5",
            "null",
            "://",
            "file://",
            "chrome-extension://abcdef",
            "https://example.com/path",
            "https://user@example.com",
            "https://example.com:",
            "https://example.com:abc",
            "https://EXAMPLE.com",
            "https://-bad.example.com",
        ] {
            let error = CallerOrigin::parse(refused).expect_err(refused);
            assert_eq!(error.name, "SecurityError", "{refused}");
        }
    }

    #[test]
    fn rp_id_must_be_the_host_or_a_multi_label_parent_domain() {
        let origin = CallerOrigin::parse("https://login.example.com").expect("origin");
        assert_eq!(origin.rp_id(None).ok().as_deref(), Some("login.example.com"));
        assert_eq!(
            origin.rp_id(Some("login.example.com")).ok().as_deref(),
            Some("login.example.com")
        );
        assert_eq!(origin.rp_id(Some("Example.COM")).ok().as_deref(), Some("example.com"));
        for foreign in [
            "com",
            "example.org",
            "other.example.com",
            "ample.com",
            "xlogin.example.com",
            ".example.com",
            "example.com.",
            "",
            "evil.com/example.com",
        ] {
            let error = origin.rp_id(Some(foreign)).expect_err(foreign);
            assert_eq!(error.name, "SecurityError", "{foreign}");
        }
        let localhost = CallerOrigin::parse("http://localhost:3000").expect("localhost");
        assert_eq!(localhost.rp_id(None).ok().as_deref(), Some("localhost"));
        assert!(localhost.rp_id(Some("example.com")).is_err());
    }

    #[test]
    fn client_data_uses_the_host_origin_in_specification_order() {
        let origin = CallerOrigin::parse("https://example.com:8443").expect("origin");
        let json = client_data_json(Operation::Get, "AAEC", &origin);
        assert_eq!(
            String::from_utf8(json).expect("utf8"),
            r#"{"type":"webauthn.get","challenge":"AAEC","origin":"https://example.com:8443","crossOrigin":false}"#
        );
        let created = client_data_json(Operation::Create, "AAEC", &origin);
        assert!(created.starts_with(br#"{"type":"webauthn.create""#));
    }
}
