//! Where a browser profile sends its traffic.
//!
//! A profile host names the egress of a profile once, and the engine reads that
//! record instead of a string inside the browser arguments, so a later change can
//! answer for a proxy that does not answer and can report which egress a page uses.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs as _};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, de};
use url::Url;

/// Longest proxy address, in bytes.
pub const MAX_PROXY_BYTES: usize = 512;

/// Longest time that a dispatch waits for the listening port of the proxy before
/// it reports that the egress does not answer.
pub const MAX_PROBE: Duration = Duration::from_secs(2);

/// The proxy schemes that a profile host accepts. Chromium reads these three.
/// `socks5h` is not one of them: Chromium drops a proxy it does not know, and
/// the profile would then reach its pages without the proxy.
const SCHEMES: [&str; 3] = ["socks5", "http", "https"];

/// Why the proxy of an egress did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressProbeError {
    #[error("has no readable address")]
    Address,
    #[error("does not resolve its name")]
    Unresolved,
    #[error("refuses the connection")]
    Refused,
    #[error("did not answer within {millis} ms")]
    TimedOut { millis: u64 },
}

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
    #[error("an egress proxy holds no `=`, `;` or `,`: Chromium splits its proxy switch on them")]
    Delimiter,
    #[error("an egress proxy names `host:port`, for example `socks5://127.0.0.1:1080`")]
    Address,
    #[error("an egress proxy port is a number between 1 and 65535")]
    Port,
}

/// The egress of one browser profile: the proxy that its traffic uses.
///
/// The host and the port follow the URL standard, which is the parser that
/// Chromium reads a proxy with, so a record cannot state a proxy that the engine
/// would drop and then reach its pages directly. A hand-written address rule was
/// replaced by that parser after review found a shape that it missed in each of
/// five rounds.
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
        // Chromium splits `--proxy-server` on these, so a host that holds one turns
        // the switch into a rule that matches nothing and every page goes out direct.
        if proxy.contains(['=', ';', ',']) {
            return Err(EgressError::Delimiter);
        }
        // The host and the port are read by the parser that Chromium follows, under
        // a scheme where it reads a numeric host as an address and refuses one that
        // is not an address. A hand-written rule missed a shape in every round of
        // review that it was written for.
        let url = Url::parse(&format!("http://{rest}/")).map_err(|_| EgressError::Address)?;
        if url.host_str().is_none_or(str::is_empty) {
            return Err(EgressError::Address);
        }
        match url.port() {
            Some(port) if port > 0 => {}
            _ => return Err(EgressError::Port),
        }
        Ok(Self {
            proxy: proxy.to_string(),
        })
    }

    /// The host and port of the proxy, when the record could be read.
    #[must_use]
    pub fn address(&self) -> Option<(String, u16)> {
        let (_, rest) = self.proxy.split_once("://")?;
        let url = Url::parse(&format!("http://{rest}/")).ok()?;
        Some((url.host_str()?.to_string(), url.port()?))
    }

    /// Open one connection to the listening port of the proxy, within `bound`.
    ///
    /// A proxy that holds no route to its target still opens its port, so this
    /// probe answers for the machine and the listener, and a page that the proxy
    /// then cannot reach is what the navigation itself reports.
    ///
    /// # Errors
    /// The address cannot be read, its name does not resolve, or no address of it
    /// answers within the bound.
    pub fn probe(&self, bound: Duration) -> Result<(), EgressProbeError> {
        let Some((host, port)) = self.address() else {
            return Err(EgressProbeError::Address);
        };
        let started = std::time::Instant::now();
        let deadline = started + bound;
        let addresses = resolve(&host, port, bound)?;
        if addresses.is_empty() {
            return Err(EgressProbeError::Unresolved);
        }
        let mut failure = EgressProbeError::Refused;
        for address in addresses {
            // One deadline for the whole probe, so a name with several addresses
            // can not take several bounds.
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(EgressProbeError::TimedOut {
                    millis: u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
                });
            }
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    failure = EgressProbeError::TimedOut {
                        millis: u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
                    };
                }
                Err(_) => failure = EgressProbeError::Refused,
            }
        }
        Err(failure)
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

/// The socket addresses of one proxy host and port, within `bound`.
///
/// A literal address needs no lookup. A name is looked up on its own thread,
/// because the resolver has no bound of its own, and the probe reads the answer
/// with the remaining time; a lookup that outlives it is left to finish alone.
fn resolve(host: &str, port: u16, bound: Duration) -> Result<Vec<SocketAddr>, EgressProbeError> {
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        return Ok(vec![SocketAddr::new(address, port)]);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let name = host.to_string();
    std::thread::Builder::new()
        .name("egress-probe-resolve".into())
        .spawn(move || {
            let _ = sender.send((name.as_str(), port).to_socket_addrs().map(Iterator::collect::<Vec<_>>));
        })
        .map_err(|_| EgressProbeError::Unresolved)?;
    match receiver.recv_timeout(bound) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(_)) => Err(EgressProbeError::Unresolved),
        Err(_) => Err(EgressProbeError::TimedOut {
            millis: u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
        }),
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
            "socks5://127.0.0.1:1080",
            "socks5://proxy1.example:1080",
            // The standard reads a short IPv4 form as 1.2.0.3, as Chromium does.
            "socks5://1.2.3:1080",
            "socks5://0xzz.example:1080",
            "socks5://127.0.0.1.:1080",
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
            ("socks5://127.0.0.1", EgressError::Port),
            ("socks5://127.0.0.1:0", EgressError::Port),
            ("socks5://127.0.0.1:1080/", EgressError::Suffix),
            ("socks5://127.0.0.1:1080?x=1", EgressError::Suffix),
            ("socks5://exa mple:1080", EgressError::Shape),
            ("socks5://999.1.1.1:1080", EgressError::Address),
            ("socks5://256.0.0.1:1080", EgressError::Address),
            ("socks5://127.0.0.1:+1080", EgressError::Address),
            ("socks5://ftp=x:1080", EgressError::Delimiter),
            ("socks5://a;b:1080", EgressError::Delimiter),
            ("socks5://a,b:1080", EgressError::Delimiter),
            ("socks5://127.0.0.1:1080 ", EgressError::Shape),
            ("socks5://proxy.0x10:1080", EgressError::Address),
            ("socks5://10.0.0.0x100:1080", EgressError::Address),
            ("socks5://999.1.1.1.:1080", EgressError::Address),
            ("socks5://proxy.0x10.:1080", EgressError::Address),
            ("socks5://proxy.0x:1080", EgressError::Address),
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
    fn a_session_refuses_an_egress_that_it_would_not_apply() {
        let egress = Some(Egress::parse("socks5://100.64.0.10:1080").unwrap());
        for (config, expected) in [
            (
                crate::BrowserConfig {
                    egress: egress.clone(),
                    backend: crate::BackendKind::FirefoxBidi,
                    ..crate::BrowserConfig::default()
                },
                "applies to a chromium session",
            ),
            (
                crate::BrowserConfig {
                    egress: egress.clone(),
                    backend: crate::BackendKind::SafariWebDriver,
                    ..crate::BrowserConfig::default()
                },
                "applies to a chromium session",
            ),
            (
                crate::BrowserConfig {
                    egress: egress.clone(),
                    extra_args: vec!["--proxy-bypass-list=*".into()],
                    ..crate::BrowserConfig::default()
                },
                "--proxy-bypass-list=*",
            ),
            (
                crate::BrowserConfig {
                    egress: egress.clone(),
                    remote: serde_json::from_str(r#"{"targets":{"phone":{"provider":"example","browser_name":"chrome","platform_name":"android"}}}"#)
                        .unwrap(),
                    ..crate::BrowserConfig::default()
                },
                "remote target",
            ),
        ] {
            let error = config.check_egress().unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        // The record on its own backend passes, and so does a session that names
        // its proxy only in an argument, as before the record existed.
        assert!(
            crate::BrowserConfig {
                egress,
                ..crate::BrowserConfig::default()
            }
            .check_egress()
            .is_ok()
        );
        assert!(
            crate::BrowserConfig {
                extra_args: vec!["--proxy-bypass-list=*".into()],
                ..crate::BrowserConfig::default()
            }
            .check_egress()
            .is_ok()
        );
    }

    #[test]
    fn a_proxy_that_answers_passes_the_probe() {
        // The test opens a listener of its own, so no network is used.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let egress = Egress::parse(&format!("socks5://127.0.0.1:{port}")).unwrap();
        assert_eq!(egress.address(), Some(("127.0.0.1".to_string(), port)));
        assert_eq!(egress.probe(Duration::from_millis(500)), Ok(()));
        drop(listener);
        // With the listener gone the same address refuses, which is the same code
        // as an address that nothing routes to.
        let failure = egress.probe(Duration::from_millis(500)).unwrap_err();
        assert!(matches!(failure, EgressProbeError::Refused), "{failure:?}");
        let unroutable = Egress::parse("socks5://192.0.2.1:1080").unwrap();
        let failure = unroutable.probe(Duration::from_millis(300)).unwrap_err();
        assert!(
            matches!(failure, EgressProbeError::Refused | EgressProbeError::TimedOut { .. }),
            "{failure:?}"
        );
        // The whole probe stays inside its bound, also when the address does not
        // answer at all, because every attempt reads one deadline.
        let started = std::time::Instant::now();
        let unroutable = Egress::parse("socks5://192.0.2.1:1080").unwrap();
        let _ = unroutable.probe(Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "the probe took {:?}",
            started.elapsed()
        );
        let unresolved = Egress::parse("socks5://no-such-host.invalid:1080").unwrap();
        assert_eq!(
            unresolved.probe(Duration::from_millis(300)),
            Err(EgressProbeError::Unresolved)
        );
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
