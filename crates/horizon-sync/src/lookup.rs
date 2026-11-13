//! The name lookup of one link, with the bound that a resolver has not.
//!
//! The resolver of a computer waits for its own servers, and a name whose
//! resolver is gone holds the calling thread for that time. Every link of this
//! crate therefore asks through [`LinkLookup`], which runs the question on a
//! thread of its own and answers at the bound of the attempt. A test gives its own
//! [`NameLookup`] and reads no resolver of the computer.
//!
//! The seam itself is [`horizon_browser::lookup`], which the speech path of
//! `horizon-core` uses for the same reason.

use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};

use horizon_browser::lookup::{LookupFailure, NameLookup, SystemLookup, resolve_within};

/// Where one link asks for the addresses of a name.
#[derive(Clone)]
pub struct LinkLookup(Arc<dyn NameLookup>);

impl Default for LinkLookup {
    /// The lookup of this computer.
    fn default() -> Self {
        Self::system()
    }
}

impl LinkLookup {
    /// The lookup of this computer.
    #[must_use]
    pub fn system() -> Self {
        Self::new(Arc::new(SystemLookup))
    }

    /// The lookup that answers from `lookup`.
    #[must_use]
    pub fn new(lookup: Arc<dyn NameLookup>) -> Self {
        Self(lookup)
    }

    /// Every address of `host:port`, within `bound`.
    ///
    /// # Errors
    /// The name has no address, or no answer arrived inside `bound`.
    pub fn resolve(&self, host: &str, port: u16, bound: Duration) -> Result<Vec<SocketAddr>, LookupFailure> {
        resolve_within(&self.0, host, port, bound)
    }
}

impl fmt::Debug for LinkLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LinkLookup")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use horizon_browser::lookup::{LookupFailure, NameLookup};

    use super::LinkLookup;

    /// A lookup that keeps the question and answers it after `hold`.
    struct SlowLookup {
        hold: Duration,
        asked: Mutex<Vec<(String, u16)>>,
    }

    impl NameLookup for SlowLookup {
        fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
            self.asked
                .lock()
                .expect("the questions of the test")
                .push((host.to_owned(), port));
            std::thread::sleep(self.hold);
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
        }
    }

    /// B14: a lookup that outlives the bound of the attempt answers
    /// [`LookupFailure::TimedOut`] at that bound, so a name whose resolver is gone
    /// ends the attempt instead of holding it for the resolver's own time.
    #[test]
    fn a_name_that_resolves_too_late_answers_at_the_bound_of_the_attempt() {
        let lookup = LinkLookup::new(Arc::new(SlowLookup {
            hold: Duration::from_secs(5),
            asked: Mutex::new(Vec::new()),
        }));
        let bound = Duration::from_millis(50);
        let started = Instant::now();
        assert_eq!(
            lookup.resolve("slow-resolver.example", 7000, bound),
            Err(LookupFailure::TimedOut),
            "a name that answers after its bound answers nothing"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the caller waits for its own bound and not for the resolver: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_name_that_answers_inside_the_bound_answers_its_addresses() {
        let lookup = LinkLookup::new(Arc::new(SlowLookup {
            hold: Duration::ZERO,
            asked: Mutex::new(Vec::new()),
        }));
        assert_eq!(
            lookup.resolve("a-server.example", 7000, Duration::from_secs(1)),
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], 7000))])
        );
    }

    #[test]
    fn the_debug_of_a_lookup_carries_no_answer() {
        assert_eq!(format!("{:?}", LinkLookup::system()), "LinkLookup");
    }
}
