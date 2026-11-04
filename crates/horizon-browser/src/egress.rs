//! Where a browser profile sends its traffic.
//!
//! A profile host names the egress of a profile once, and the engine reads that
//! record instead of a string inside the browser arguments, so a later change can
//! answer for a proxy that does not answer and can report which egress a page uses.

use serde::{Deserialize, Deserializer, Serialize, de};

/// Longest proxy address, in bytes.
pub const MAX_PROXY_BYTES: usize = 512;

/// The proxy schemes that a profile host accepts. Chromium reads these three.
/// `socks5h` is not one of them: Chromium drops a proxy it does not know, and
/// the profile would then reach its pages without the proxy.
const SCHEMES: [&str; 3] = ["socks5", "http", "https"];

/// Why an egress record is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("an egress proxy is 1 to {MAX_PROXY_BYTES} bytes with no space and no control character")]
    Shape,
    #[error("an egress proxy starts with `socks5://`, `http://` or `https://`")]
    Scheme,
    #[error("Chromium reads `socks5://`, not `socks5h://`; `socks5` resolves the name on the proxy")]
    Socks5h,
    #[error("an egress proxy holds no credential, because the host file holds none")]
    Credential,
    #[error("an egress proxy names an address only, without a path, a query or a fragment")]
    Suffix,
    #[error("an egress proxy names `host:port`, for example `socks5://127.0.0.1:1080`")]
    Address,
    #[error("an egress proxy port is a number between 1 and 65535")]
    Port,
}

/// The egress of one browser profile: the proxy that its traffic uses.
///
/// A record from any source is checked, not only one that a host file named: the
/// settings store and a saved panel read the same type, and a proxy that Chromium
/// would drop must not arrive that way either.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Egress {
    /// The proxy address, `scheme://host:port`, with no credential.
    pub proxy: String,
}

impl<'de> Deserialize<'de> for Egress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            proxy: String,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::parse(&raw.proxy).map_err(de::Error::custom)
    }
}

impl Egress {
    /// Check one proxy address.
    ///
    /// # Errors
    /// The address has no space for a scheme of the list, holds a credential,
    /// names more than an address, or has no host and port.
    pub fn parse(proxy: &str) -> Result<Self, EgressError> {
        if proxy.is_empty()
            || proxy.len() > MAX_PROXY_BYTES
            || proxy.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(EgressError::Shape);
        }
        let Some((scheme, rest)) = proxy.split_once("://") else {
            return Err(EgressError::Scheme);
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "socks5h" {
            return Err(EgressError::Socks5h);
        }
        if !SCHEMES.contains(&scheme.as_str()) {
            return Err(EgressError::Scheme);
        }
        if rest.contains('@') {
            return Err(EgressError::Credential);
        }
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(EgressError::Suffix);
        }
        let Some((host, port)) = rest.rsplit_once(':') else {
            return Err(EgressError::Address);
        };
        // A host is a name, an IPv4 address, or an IPv6 address in brackets. A
        // bracket or a colon anywhere else is a malformed address, and Chromium
        // would then drop the proxy and reach the page without it.
        let host_ok = match host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            Some(inner) => inner.parse::<std::net::Ipv6Addr>().is_ok(),
            None => {
                !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            }
        };
        if !host_ok {
            return Err(EgressError::Address);
        }
        match port.parse::<u16>() {
            Ok(0) | Err(_) => return Err(EgressError::Port),
            Ok(_) => {}
        }
        Ok(Self {
            proxy: proxy.to_string(),
        })
    }

    /// The switches that select or bypass a proxy. A session that states an
    /// egress and also carries one of these would send its pages around the proxy,
    /// so the engine refuses the pair.
    #[must_use]
    pub fn names_proxy_switch(argument: &str) -> bool {
        let switch = argument.trim_start_matches(['-', '/']);
        let name = switch.split(['=', ':']).next().unwrap_or(switch);
        matches!(
            name,
            "proxy-server" | "no-proxy-server" | "proxy-pac-url" | "proxy-auto-detect" | "proxy-bypass-list"
        )
    }

    /// The Chromium switch of this proxy. A headed Chromium reads it at start, so
    /// the record and the traffic can not disagree.
    #[must_use]
    pub fn chromium_argument(&self) -> String {
        format!("--proxy-server={}", self.proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_proxy_is_carried_unchanged() {
        for proxy in [
            "socks5://127.0.0.1:1080",
            "socks5://100.64.0.10:1080",
            "http://proxy.example:3128",
            "HTTPS://proxy.example:8443",
            "socks5://[::1]:1080",
            "socks5://[::ffff:127.0.0.1]:1080",
        ] {
            let egress = Egress::parse(proxy).expect(proxy);
            assert_eq!(egress.proxy, proxy);
        }
    }

    #[test]
    fn a_proxy_that_a_host_file_must_not_hold_is_refused() {
        for (proxy, expected) in [
            ("", EgressError::Shape),
            (" socks5://127.0.0.1:1080", EgressError::Shape),
            ("socks5://127.0.0.1:1080\n", EgressError::Shape),
            (&"a".repeat(MAX_PROXY_BYTES + 1), EgressError::Shape),
            ("127.0.0.1:1080", EgressError::Scheme),
            ("ftp://127.0.0.1:1080", EgressError::Scheme),
            ("socks5h://100.64.0.10:1080", EgressError::Socks5h),
            ("socks5://person:secret@127.0.0.1:1080", EgressError::Credential),
            ("socks5://127.0.0.1", EgressError::Address),
            ("socks5://127.0.0.1:0", EgressError::Port),
            ("socks5://127.0.0.1:1080/", EgressError::Suffix),
            ("socks5://127.0.0.1:1080?x=1", EgressError::Suffix),
            ("socks5://exa mple:1080", EgressError::Shape),
            ("socks5://exa$mple:1080", EgressError::Address),
            ("socks5://[::1:1080", EgressError::Address),
            ("socks5://a]b:1080", EgressError::Address),
            ("socks5://::1:1080", EgressError::Address),
            ("socks5://[]:1080", EgressError::Address),
            ("socks5://[1]:1080", EgressError::Address),
            ("socks5://[:]:1080", EgressError::Address),
            ("socks5://[abc]:1080", EgressError::Address),
        ] {
            assert_eq!(Egress::parse(proxy).unwrap_err(), expected, "{proxy:?}");
        }
    }

    #[test]
    fn the_chromium_switch_names_the_proxy() {
        let egress = Egress::parse("socks5://100.64.0.10:1080").unwrap();
        assert_eq!(egress.chromium_argument(), "--proxy-server=socks5://100.64.0.10:1080");
    }

    #[test]
    fn a_record_round_trips_through_serde() {
        let egress = Egress::parse("socks5://127.0.0.1:1080").unwrap();
        let text = serde_json::to_string(&egress).unwrap();
        assert_eq!(text, r#"{"proxy":"socks5://127.0.0.1:1080"}"#);
        assert_eq!(serde_json::from_str::<Egress>(&text).unwrap(), egress);
        assert!(serde_json::from_str::<Egress>(r#"{"proxy":"socks5://h:1","extra":1}"#).is_err());
    }

    #[test]
    fn a_record_from_the_settings_store_is_checked_like_a_host_file() {
        for text in [
            r#"{"proxy":"socks5h://127.0.0.1:1080"}"#,
            r#"{"proxy":"socks5://person:secret@127.0.0.1:1080"}"#,
            r#"{"proxy":"socks5://[abc]:1080"}"#,
            r#"{"proxy":"127.0.0.1:1080"}"#,
            r#"{"proxy":""}"#,
        ] {
            assert!(serde_json::from_str::<Egress>(text).is_err(), "{text}");
            assert!(
                serde_json::from_str::<crate::BrowserConfig>(&format!(
                    r#"{{"backend":"chromium_cdp","egress":{text}}}"#
                ))
                .is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_proxy_switch_is_recognized_in_both_spellings() {
        for argument in [
            "--proxy-server=socks5://127.0.0.1:1080",
            "-proxy-server=socks5://127.0.0.1:1080",
            "--no-proxy-server",
            "-no-proxy-server",
            "--proxy-pac-url=http://pac.example/p.pac",
            "--proxy-auto-detect",
            "--proxy-bypass-list=*",
            "/proxy-server=socks5://127.0.0.1:1080",
        ] {
            assert!(Egress::names_proxy_switch(argument), "{argument}");
        }
        for argument in [
            "--proxy-bypass-list-not",
            "--no-sandbox",
            "--host-resolver-rules=MAP * ~NOTFOUND",
        ] {
            assert!(!Egress::names_proxy_switch(argument), "{argument}");
        }
    }
}
