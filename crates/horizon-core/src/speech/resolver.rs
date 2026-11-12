//! The name lookup of one call of a transcription service.
//!
//! ureq asks a [`Resolver`] for the addresses of the host of the endpoint before
//! it connects. This module answers that question from the lookup seam of
//! [`horizon_browser::lookup`], which a test replaces with its own answers, so no
//! test of a name reads the resolver of this computer. A lookup that outlives the
//! bound of the call is left to finish on its own thread.
//!
//! The seam is the `unversioned` API of ureq, which does not follow semver: a
//! later minor version of ureq can change it, and the build then says so.

use std::fmt;
use std::sync::Arc;

use horizon_browser::lookup::{LookupFailure, NameLookup, SystemLookup, resolve_within};
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use ureq::{Agent, Error, Timeout};

/// The resolver of one call of a service, over the lookup seam.
pub struct SeamResolver {
    lookup: Arc<dyn NameLookup>,
}

impl SeamResolver {
    /// A resolver that answers from `lookup`.
    #[must_use]
    pub fn new(lookup: Arc<dyn NameLookup>) -> Self {
        Self { lookup }
    }

    /// The resolver of this computer.
    #[must_use]
    pub fn system() -> Self {
        Self::new(Arc::new(SystemLookup))
    }
}

impl fmt::Debug for SeamResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SeamResolver")
    }
}

impl Resolver for SeamResolver {
    fn resolve(&self, uri: &Uri, _config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, Error> {
        let (Some(host), Some(port)) = (uri.host(), uri.port_u16().or_else(|| default_port(uri.scheme_str()))) else {
            return Err(Error::BadUri(uri.to_string()));
        };
        // One bound for the lookup: the time that the call has left, which the
        // caller set, or no bound when it set none.
        let addresses = resolve_within(&self.lookup, host, port, *timeout.after).map_err(error_of)?;
        let mut resolved = self.empty();
        for address in addresses {
            // Sixteen addresses are what one call reads, so a name with more keeps
            // the first sixteen, as the resolver of ureq does.
            if resolved.try_push(address).is_err() {
                break;
            }
        }
        Ok(resolved)
    }
}

/// An agent of one call of a service, whose name lookup is `resolver`.
#[must_use]
pub fn agent(config: Config, resolver: SeamResolver) -> Agent {
    Agent::with_parts(config, DefaultConnector::default(), resolver)
}

/// The port that a scheme answers when the URL names none.
fn default_port(scheme: Option<&str>) -> Option<u16> {
    match scheme {
        Some("https") => Some(443),
        Some("http") => Some(80),
        _ => None,
    }
}

/// The failure of ureq for a lookup that gave no address.
fn error_of(failure: LookupFailure) -> Error {
    match failure {
        LookupFailure::Unresolved => Error::HostNotFound,
        LookupFailure::TimedOut => Error::Timeout(Timeout::Resolve),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ureq::config::Config;
    use ureq::http::Uri;
    use ureq::unversioned::resolver::Resolver;
    use ureq::unversioned::transport::NextTimeout;
    use ureq::unversioned::transport::time::Duration as UreqDuration;

    use super::{SeamResolver, default_port};
    use horizon_browser::lookup::NameLookup;

    /// The bound that a test gives one lookup.
    const BOUND: Duration = Duration::from_millis(50);

    /// A lookup of a test: it answers with `answers`, and it keeps the question.
    struct TestLookup {
        addresses: Option<Vec<SocketAddr>>,
        asked: Mutex<Vec<(String, u16)>>,
    }

    impl TestLookup {
        fn answering(addresses: &[SocketAddr]) -> Arc<Self> {
            Arc::new(Self {
                addresses: Some(addresses.to_vec()),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn answering_nothing() -> Arc<Self> {
            Arc::new(Self {
                addresses: None,
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<(String, u16)> {
            self.asked.lock().expect("the questions of the test").clone()
        }
    }

    impl NameLookup for TestLookup {
        fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
            self.asked
                .lock()
                .expect("the questions of the test")
                .push((host.to_owned(), port));
            match &self.addresses {
                Some(addresses) => Ok(addresses.clone()),
                None => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }
    }

    fn next_timeout() -> NextTimeout {
        NextTimeout {
            after: UreqDuration::Exact(BOUND),
            ..NextTimeout::default()
        }
    }

    fn resolve(resolver: &SeamResolver, url: &str) -> Result<Vec<SocketAddr>, ureq::Error> {
        let uri: Uri = url.parse().expect("the URI of the test");
        resolver
            .resolve(&uri, &Config::default(), next_timeout())
            .map(|addresses| addresses.to_vec())
    }

    #[test]
    fn a_name_that_answers_nothing_asks_for_no_connection() {
        let lookup = TestLookup::answering_nothing();
        let resolver = SeamResolver::new(lookup.clone());
        let failure = resolve(&resolver, "https://no-such-host.invalid/v1/audio/transcriptions")
            .expect_err("a name with no address");
        assert!(matches!(failure, ureq::Error::HostNotFound), "{failure:?}");
        assert_eq!(lookup.asked(), [("no-such-host.invalid".to_owned(), 443)]);
    }

    #[test]
    fn the_question_names_the_port_of_the_url_or_the_one_of_its_scheme() {
        for (url, expected) in [
            ("https://a-service.example/v1", 443),
            ("http://a-service.example/v1", 80),
            ("https://a-service.example:8443/v1", 8443),
        ] {
            let lookup = TestLookup::answering_nothing();
            let resolver = SeamResolver::new(lookup.clone());
            let _ = resolve(&resolver, url);
            assert_eq!(lookup.asked(), [("a-service.example".to_owned(), expected)], "{url}");
        }
        assert_eq!(default_port(Some("socks5")), None);
    }

    #[test]
    fn a_name_that_answers_addresses_answers_them() {
        let addresses = [SocketAddr::from(([127, 0, 0, 1], 40913))];
        let lookup = TestLookup::answering(&addresses);
        let resolver = SeamResolver::new(lookup.clone());
        assert_eq!(
            resolve(&resolver, "https://a-service.example/v1").expect("one address"),
            addresses
        );
    }

    #[test]
    fn a_literal_address_asks_no_lookup() {
        let lookup = TestLookup::answering_nothing();
        let resolver = SeamResolver::new(lookup.clone());
        assert_eq!(
            resolve(&resolver, "http://127.0.0.1:40913/v1").expect("a literal address"),
            [SocketAddr::from(([127, 0, 0, 1], 40913))]
        );
        assert!(lookup.asked().is_empty(), "an address of the record is no name");
    }
}
