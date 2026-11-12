//! The name lookup of one probe of the network, and the answers of a test.
//!
//! A caller that asks the system for the addresses of a name holds its thread for
//! as long as the resolver takes, because the resolver has no bound of its own.
//! The lookup therefore runs on a thread of its own and the caller reads the
//! answer with the time of its bound. A caller that has its own answers, such as a
//! test, implements [`NameLookup`] and reads no resolver of this computer.

use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs as _};
use std::sync::Arc;
use std::time::Duration;

/// Where one probe asks for the addresses of the name of a host.
pub trait NameLookup: Send + Sync + 'static {
    /// Every address of `host:port`, or why the name has none.
    ///
    /// # Errors
    /// The name has no address, or the resolver could not answer the question.
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
}

/// The resolver of this computer.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLookup;

impl NameLookup for SystemLookup {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        (host, port).to_socket_addrs().map(Iterator::collect)
    }
}

/// Why a name gave one probe no address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupFailure {
    /// The name has no address, or the resolver refused the question.
    Unresolved,
    /// No answer arrived inside the bound of the probe.
    TimedOut,
}

/// The socket addresses of `host:port`, within `bound`.
///
/// A literal address needs no lookup. A name is looked up on its own thread, and a
/// lookup that outlives the bound is left to finish alone: the caller answers at
/// its bound and no thread of it waits for the resolver.
///
/// # Errors
/// The name has no address, or no answer arrived inside `bound`.
pub fn resolve_within<N: NameLookup + ?Sized>(
    lookup: &Arc<N>,
    host: &str,
    port: u16,
    bound: Duration,
) -> Result<Vec<SocketAddr>, LookupFailure> {
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, port)]);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let name = host.to_owned();
    let lookup = Arc::clone(lookup);
    std::thread::Builder::new()
        .name("name-lookup".to_owned())
        .spawn(move || {
            let _ = sender.send(lookup.resolve(&name, port));
        })
        .map_err(|_| LookupFailure::Unresolved)?;
    match receiver.recv_timeout(bound) {
        Ok(Ok(addresses)) if !addresses.is_empty() => Ok(addresses),
        Ok(_) => Err(LookupFailure::Unresolved),
        Err(_) => Err(LookupFailure::TimedOut),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::{LookupFailure, NameLookup, resolve_within};

    /// The shortest bound that a test gives a lookup.
    const BOUND: Duration = Duration::from_millis(50);

    /// What a lookup of a test answers.
    #[derive(Clone, Copy)]
    enum Answer {
        /// These addresses.
        These(&'static [&'static str]),
        /// Nothing, as a port of this computer with no listener does not.
        Nothing,
        /// An empty list, as a resolver with no address for a name does not.
        Empty,
        /// Nothing inside the bound of the test.
        Hangs,
    }

    /// A lookup of a test: the resolver of the computer is never read.
    struct TestLookup {
        answer: Answer,
        asked: Mutex<Vec<(String, u16)>>,
    }

    impl TestLookup {
        fn new(answer: Answer) -> Self {
            Self {
                answer,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<(String, u16)> {
            self.asked.lock().expect("the questions of the test").clone()
        }
    }

    impl NameLookup for TestLookup {
        fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<std::net::SocketAddr>> {
            self.asked
                .lock()
                .expect("the questions of the test")
                .push((host.to_owned(), port));
            match self.answer {
                Answer::These(addresses) => Ok(addresses
                    .iter()
                    .map(|address| std::net::SocketAddr::new(address.parse().expect("an address of the test"), port))
                    .collect()),
                Answer::Nothing => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
                Answer::Empty => Ok(Vec::new()),
                // The caller stops waiting at its bound; this lookup outlives it.
                Answer::Hangs => {
                    std::thread::sleep(Duration::from_secs(5));
                    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                }
            }
        }
    }

    #[test]
    fn a_literal_address_asks_no_lookup() {
        let lookup = std::sync::Arc::new(TestLookup::new(Answer::Hangs));
        let addresses = resolve_within(&lookup, "127.0.0.1", 40913, BOUND).expect("a literal address");
        assert_eq!(
            addresses,
            [std::net::SocketAddr::from(([127, 0, 0, 1], 40913))],
            "an address of the record is no name"
        );
        assert!(lookup.asked().is_empty(), "an address of the record is no name");
    }

    #[test]
    fn a_name_that_answers_addresses_answers_them_with_its_port() {
        let lookup = std::sync::Arc::new(TestLookup::new(Answer::These(&["127.0.0.1", "::1"])));
        let addresses = resolve_within(&lookup, "a-service.example", 8443, BOUND).expect("two addresses");
        assert_eq!(addresses.len(), 2);
        assert!(addresses.iter().all(|address| address.port() == 8443));
        assert_eq!(lookup.asked(), [("a-service.example".to_owned(), 8443)]);
    }

    #[test]
    fn a_name_that_answers_nothing_is_unresolved() {
        for answer in [Answer::Nothing, Answer::Empty] {
            let lookup = std::sync::Arc::new(TestLookup::new(answer));
            assert_eq!(
                resolve_within(&lookup, "no-such-host.invalid", 443, BOUND),
                Err(LookupFailure::Unresolved)
            );
            assert_eq!(lookup.asked(), [("no-such-host.invalid".to_owned(), 443)]);
        }
    }

    #[test]
    fn a_lookup_that_does_not_answer_inside_the_bound_times_out() {
        let lookup = std::sync::Arc::new(TestLookup::new(Answer::Hangs));
        let started = Instant::now();
        assert_eq!(
            resolve_within(&lookup, "slow-resolver.example", 443, BOUND),
            Err(LookupFailure::TimedOut)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the caller answers at its bound, and the lookup of the test takes 5 s"
        );
    }
}
