//! Where a browser profile sends its traffic.
//!
//! A browser profile names its egress once, and the engine reads that record
//! instead of a string inside the browser arguments, so a later change can answer
//! for a proxy that does not answer and can report which egress a page uses.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize, de};
use url::Url;

/// Longest proxy address, in bytes.
pub const MAX_PROXY_BYTES: usize = 512;

/// Longest time that a dispatch waits for the listening port of the proxy before
/// it reports that the egress does not answer.
pub const MAX_PROBE: Duration = Duration::from_secs(2);

/// The proxy schemes that a browser profile accepts. Chromium reads these three.
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
    /// This waits on the network, so it runs on the driver thread that a session
    /// starts (`browser-driver` in `session.rs`) and never inside a frame.
    ///
    /// # Errors
    /// The address cannot be read, its name does not resolve, or no address of it
    /// answers within the bound.
    pub fn probe(&self, bound: Duration) -> Result<(), EgressProbeError> {
        self.probe_with(SystemNetwork, bound)
    }

    /// [`Egress::probe`] with the clock and the answers of a caller, so that a
    /// test of this module reads neither the resolver nor the routes of the
    /// computer.
    fn probe_with<N: ProbeNetwork + Send + Sync + 'static>(
        &self,
        network: N,
        bound: Duration,
    ) -> Result<(), EgressProbeError> {
        let Some((host, port)) = self.address() else {
            return Err(EgressProbeError::Address);
        };
        // One handle, so that a lookup which outlives the bound finishes on its
        // own thread after this probe has answered.
        let network = Arc::new(network);
        // One deadline for the whole probe, so a name with several addresses can
        // not take several bounds.
        let deadline = network.now() + bound;
        let addresses = resolve_within(&network, &host, port, bound)?;
        if addresses.is_empty() {
            return Err(EgressProbeError::Unresolved);
        }
        let mut failure = EgressProbeError::Refused;
        for address in addresses {
            let remaining = deadline.saturating_duration_since(network.now());
            if remaining.is_zero() {
                return Err(timed_out(bound));
            }
            match network.connect(address, remaining) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::TimedOut => failure = timed_out(bound),
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

/// What a probe asks of the system: the time that bounds it, the addresses of
/// the name of the proxy, and one connection to them.
///
/// A test answers all three, so no test of this module reads the resolver or the
/// routes of the computer.
trait ProbeNetwork {
    /// The time of this probe, which bounds every attempt of it.
    fn now(&self) -> Instant;

    /// Every address of `host:port`, or why the name has none.
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;

    /// One connection attempt to `address`, waiting at most `bound`.
    fn connect(&self, address: SocketAddr, bound: Duration) -> io::Result<()>;
}

/// The resolver, the routes and the clock of the computer.
struct SystemNetwork;

impl ProbeNetwork for SystemNetwork {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        (host, port).to_socket_addrs().map(Iterator::collect)
    }

    fn connect(&self, address: SocketAddr, bound: Duration) -> io::Result<()> {
        TcpStream::connect_timeout(&address, bound).map(|_| ())
    }
}

/// The socket addresses of one proxy host and port, within `bound`.
///
/// A literal address needs no lookup. A name is looked up on its own thread,
/// because the resolver has no bound of its own: a probe that called it here
/// would hold this thread for as long as the resolver takes, whatever the bound
/// says. The probe reads the answer with the remaining time, and a lookup that
/// outlives it is left to finish alone.
fn resolve_within<N: ProbeNetwork + Send + Sync + 'static>(
    network: &Arc<N>,
    host: &str,
    port: u16,
    bound: Duration,
) -> Result<Vec<SocketAddr>, EgressProbeError> {
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, port)]);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let name = host.to_string();
    let lookup = Arc::clone(network);
    std::thread::Builder::new()
        .name("egress-probe-resolve".into())
        .spawn(move || {
            let _ = sender.send(lookup.resolve(&name, port));
        })
        .map_err(|_| EgressProbeError::Unresolved)?;
    match receiver.recv_timeout(bound) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(_)) => Err(EgressProbeError::Unresolved),
        Err(_) => Err(timed_out(bound)),
    }
}

/// The timeout of a probe of `bound`, in the milliseconds that a person reads.
fn timed_out(bound: Duration) -> EgressProbeError {
    EgressProbeError::TimedOut {
        millis: u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

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
    }

    /// What a lookup of a test answers for the name of a proxy.
    #[derive(Clone, Copy)]
    enum Lookup {
        /// The name answers these addresses.
        These(&'static [&'static str]),
        /// The name answers nothing.
        Nothing,
        /// The resolver of the test does not answer inside the bound of the probe.
        Hangs,
    }

    /// What a connection of a test answers.
    #[derive(Clone, Copy)]
    enum Connection {
        /// A listener accepts the connection.
        Answers,
        /// The port refuses it, as a port that nothing holds does.
        Refused,
        /// No answer arrives inside the bound.
        NoAnswer,
    }

    /// The clock and the answers of a test: the resolver, the routes and the
    /// clock of the computer are never read.
    #[derive(Clone)]
    struct TestNetwork {
        state: Arc<Mutex<State>>,
        lookup: Lookup,
        connection: Connection,
    }

    struct State {
        now: Instant,
        tick: Duration,
        lookups: usize,
        attempts: Vec<(SocketAddr, Duration)>,
    }

    impl TestNetwork {
        fn new(lookup: Lookup, connection: Connection) -> Self {
            Self {
                state: Arc::new(Mutex::new(State {
                    now: Instant::now(),
                    tick: Duration::ZERO,
                    lookups: 0,
                    attempts: Vec::new(),
                })),
                lookup,
                connection,
            }
        }

        /// The time that each connection attempt of this test spends.
        fn ticking(mut self, tick: Duration) -> Self {
            let state = Arc::get_mut(&mut self.state).expect("the state of the test");
            state.get_mut().expect("the clock of the test").tick = tick;
            self
        }

        fn lookups(&self) -> usize {
            self.state.lock().expect("the state of the test").lookups
        }

        fn attempts(&self) -> Vec<(SocketAddr, Duration)> {
            self.state.lock().expect("the state of the test").attempts.clone()
        }
    }

    impl ProbeNetwork for TestNetwork {
        fn now(&self) -> Instant {
            self.state.lock().expect("the state of the test").now
        }

        fn resolve(&self, _host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
            {
                let mut state = self.state.lock().expect("the state of the test");
                state.lookups += 1;
            }
            match self.lookup {
                Lookup::These(addresses) => Ok(addresses
                    .iter()
                    .map(|address| SocketAddr::new(address.parse().expect("an address of the test"), port))
                    .collect()),
                Lookup::Nothing => Err(io::Error::from(io::ErrorKind::NotFound)),
                // The probe stops waiting at its bound; this lookup outlives it.
                Lookup::Hangs => {
                    std::thread::sleep(Duration::from_secs(5));
                    Err(io::Error::from(io::ErrorKind::NotFound))
                }
            }
        }

        fn connect(&self, address: SocketAddr, bound: Duration) -> io::Result<()> {
            {
                let mut state = self.state.lock().expect("the state of the test");
                state.attempts.push((address, bound));
                let tick = state.tick;
                state.now += tick;
            }
            match self.connection {
                Connection::Answers => Ok(()),
                Connection::Refused => Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
                Connection::NoAnswer => Err(io::Error::from(io::ErrorKind::TimedOut)),
            }
        }
    }

    /// The shortest bound that a test gives the probe.
    const BOUND: Duration = Duration::from_millis(50);

    fn proxy(host_port: &str) -> Egress {
        Egress::parse(&format!("socks5://{host_port}")).expect("a proxy of the test")
    }

    #[test]
    fn a_name_that_answers_nothing_is_unresolved() {
        let network = TestNetwork::new(Lookup::Nothing, Connection::Answers);
        assert_eq!(
            proxy("no-such-host.invalid:1080").probe_with(network.clone(), BOUND),
            Err(EgressProbeError::Unresolved)
        );
        assert_eq!(network.lookups(), 1);
        assert!(
            network.attempts().is_empty(),
            "a name without an address has nothing to attempt"
        );
    }

    #[test]
    fn a_lookup_that_does_not_answer_inside_the_bound_times_out() {
        let network = TestNetwork::new(Lookup::Hangs, Connection::Answers);
        let started = Instant::now();
        assert_eq!(
            proxy("slow-resolver.example:1080").probe_with(network.clone(), BOUND),
            Err(EgressProbeError::TimedOut { millis: 50 })
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the probe stops waiting at its bound, and the lookup of the test takes 5 s"
        );
    }

    #[test]
    fn a_connection_that_is_refused_refuses_the_proxy() {
        let network = TestNetwork::new(Lookup::These(&["127.0.0.1"]), Connection::Refused);
        assert_eq!(
            proxy("closed.example:1080").probe_with(network.clone(), BOUND),
            Err(EgressProbeError::Refused)
        );
        assert_eq!(network.attempts().len(), 1);
    }

    #[test]
    fn a_connection_that_does_not_answer_times_out_with_the_bound() {
        let network = TestNetwork::new(Lookup::These(&["192.0.2.1"]), Connection::NoAnswer);
        assert_eq!(
            proxy("black-hole.example:1080").probe_with(network.clone(), BOUND),
            Err(EgressProbeError::TimedOut { millis: 50 })
        );
    }

    #[test]
    fn the_whole_probe_reads_one_deadline_of_the_clock_of_the_test() {
        // Every attempt spends 200 ms of a 300 ms probe, so the third address of
        // the name is not attempted at all.
        let network = TestNetwork::new(
            Lookup::These(&["127.0.0.1", "127.0.0.2", "127.0.0.3"]),
            Connection::Refused,
        )
        .ticking(Duration::from_millis(200));
        let bound = Duration::from_millis(300);
        assert_eq!(
            proxy("three-addresses.example:1080").probe_with(network.clone(), bound),
            Err(EgressProbeError::TimedOut { millis: 300 })
        );
        let attempts = network.attempts();
        assert_eq!(attempts.len(), 2, "the bound of the probe cut the third address");
        assert_eq!(attempts[0].1, bound, "the first attempt gets the whole bound");
        assert_eq!(
            attempts[1].1,
            Duration::from_millis(100),
            "the second attempt gets what the first one left"
        );
        assert!(attempts.iter().all(|(_, remaining)| *remaining <= bound));
    }

    #[test]
    fn a_literal_address_asks_no_lookup() {
        let network = TestNetwork::new(Lookup::Hangs, Connection::Answers);
        assert_eq!(proxy("127.0.0.1:1080").probe_with(network.clone(), BOUND), Ok(()));
        assert_eq!(network.lookups(), 0, "an address of the record is no name");
        assert_eq!(network.attempts(), [(SocketAddr::from(([127, 0, 0, 1], 1080)), BOUND)]);
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
