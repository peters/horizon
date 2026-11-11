//! The link that joins a computer to a server, and the addresses that the link of
//! a server may use.
//!
//! Two kinds of text arrive here from outside Horizon:
//!
//! - The address of the listener, `host:port`, which a person writes in the
//!   settings or in `horizon --server --listen`.
//! - The join link, `horizon://<host>:<port>/join#<CODE>`, which a person pastes
//!   into the Settings window after a logged-in computer showed it as a QR code.
//!
//! Both are untrusted text. This module is the only parser of both, so one rule
//! decides what a server name is, what a port is, and what a code is. Everything
//! else is one typed refusal.
//!
//! The address rule belongs to the boundary of the link: Horizon encrypts nothing,
//! so the network must. The server refuses to listen on an address outside
//! loopback, `100.64.0.0/10` and `fd7a:115c:a1e0::/48`, and a computer checks the
//! same rule on every address that the name of a link resolves to, before it sends
//! one byte. Those three ranges are the addresses of a private overlay network,
//! which carries its own encryption.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

/// The scheme of a join link.
pub const SCHEME: &str = "horizon";

/// The one path of a join link.
pub const JOIN_PATH: &str = "join";

/// The characters of one invitation code: three groups of four.
pub const CODE_CHARS: usize = 12;

/// The most characters of a host of a link, as a DNS name takes.
pub const MAX_HOST_CHARS: usize = 253;

/// The most characters of a `host:port` entry, which holds a host and a port.
pub const MAX_ADDRESS_CHARS: usize = 255;

/// The 32 characters of Crockford base32. It leaves out `I`, `L`, `O` and `U`, so
/// a code has no character that a person reads as another one or as a word.
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The IPv4 range of the private overlay network, `100.64.0.0/10`.
const OVERLAY_V4: (Ipv4Addr, u32) = (Ipv4Addr::new(100, 64, 0, 0), 10);

/// The IPv6 range of the private overlay network, `fd7a:115c:a1e0::/48`.
const OVERLAY_V6: (Ipv6Addr, u32) = (Ipv6Addr::new(0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0, 0), 48);

/// Why a text is not one address, one link or one code.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// The text is not one link, one address or one code that this Horizon reads.
    #[error("{message}")]
    NotOne {
        /// One sentence for the person.
        message: String,
    },
    /// The address is outside the three ranges that a link may cross. Horizon
    /// does not encrypt its own link, so the network must.
    #[error(
        "The address {address} is not one of a private overlay network. Horizon sends a link only to a loopback address, to 100.64.0.0/10, or to fd7a:115c:a1e0::/48, because it does not encrypt the link itself."
    )]
    AddressNotAllowed {
        /// The address that was refused.
        address: IpAddr,
    },
}

impl LinkError {
    /// The stable code of this refusal, for a log line and a test.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotOne { .. } => "not_one",
            Self::AddressNotAllowed { .. } => "address_not_allowed",
        }
    }

    fn not_one(message: impl Into<String>) -> Self {
        Self::NotOne {
            message: message.into(),
        }
    }
}

/// Whether `ip` is an address that the link of a server may cross: loopback, or
/// inside the private overlay network of the two ranges above.
#[must_use]
pub fn allows_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || inside_v4(v4),
        IpAddr::V6(v6) => v6.is_loopback() || inside_v6(v6) || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Refuses an address that a link may not cross.
///
/// # Errors
/// Returns [`LinkError::AddressNotAllowed`] with one sentence for the person.
pub fn check_address(ip: IpAddr) -> Result<(), LinkError> {
    if allows_address(ip) {
        Ok(())
    } else {
        Err(LinkError::AddressNotAllowed { address: ip })
    }
}

/// The address of a server that a computer names, `host:port`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    host: String,
    port: u16,
}

impl ServerAddress {
    /// The address that `text` names, `host:port`, with the host rules of a join
    /// link.
    ///
    /// # Errors
    /// Refuses a text that is not one `host:port` of this Horizon.
    pub fn parse(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        if text.len() > MAX_ADDRESS_CHARS {
            return Err(LinkError::not_one(format!(
                "The server address is longer than the {MAX_ADDRESS_CHARS} characters of one address."
            )));
        }
        let (host, port) = split_host_port(text)?;
        Ok(Self { host, port })
    }

    /// The host of this address: an IP address without brackets, or a DNS name.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port of this address.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address as a host and a port of the system, when the host is an IP
    /// address and needs no name lookup.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, self.port))
    }

    /// This address as the text of a link, with brackets around an IPv6 address.
    #[must_use]
    pub fn authority(&self) -> String {
        authority(&self.host, self.port)
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.authority())
    }
}

/// The link that joins one computer to a server: the address of the listener and
/// one invitation code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinLink {
    host: String,
    port: u16,
    code: String,
}

impl JoinLink {
    /// The link that `text` holds.
    ///
    /// # Errors
    /// Refuses a text that is not exactly
    /// `horizon://<host>:<port>/join#<CODE>`.
    pub fn parse(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        let rest = text
            .strip_prefix("horizon://")
            .ok_or_else(|| LinkError::not_one("The link does not start with horizon://."))?;
        let (before, fragment) = rest
            .split_once('#')
            .ok_or_else(|| LinkError::not_one("The link holds no code after #."))?;
        if fragment.contains('#') {
            return Err(LinkError::not_one("The link holds more than one #."));
        }
        let code = normalize_code(fragment)?;
        let (authority, path) = before
            .split_once('/')
            .ok_or_else(|| LinkError::not_one("The link holds no /join path."))?;
        if path != JOIN_PATH {
            return Err(LinkError::not_one(format!("The path of the link is not /{JOIN_PATH}.")));
        }
        let (host, port) = split_host_port(authority)?;
        Ok(Self { host, port, code })
    }

    /// The link of `host`, `port` and `code`.
    ///
    /// # Errors
    /// Refuses a host, a port or a code that breaks its rule.
    pub fn new(host: &str, port: u16, code: &str) -> Result<Self, LinkError> {
        let (host, port) = split_host_port(&format!("{}:{port}", bracket_host(host)))?;
        let code = normalize_code(code)?;
        Ok(Self { host, port, code })
    }

    /// The host of the server: an IP address without brackets, or a DNS name.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port of the server.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The code of the invitation, in three groups of four.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The host and the port as one address, with brackets around an IPv6 address.
    #[must_use]
    pub fn authority(&self) -> String {
        authority(&self.host, self.port)
    }

    /// The address of the server, when its host is an IP address.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, self.port))
    }

    /// The link as the text that a person pastes.
    #[must_use]
    pub fn to_uri(&self) -> String {
        format!("{SCHEME}://{}/{JOIN_PATH}#{}", self.authority(), self.code)
    }
}

impl fmt::Display for JoinLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_uri())
    }
}

/// The code of an invitation in three groups of four, from `text`.
///
/// # Errors
/// Refuses a text that is not 12 characters of Crockford base32, with dashes
/// between the characters or without them, in either case.
pub fn normalize_code(text: &str) -> Result<String, LinkError> {
    let upper = text.trim().to_ascii_uppercase();
    let characters: Vec<u8> = upper.bytes().filter(|byte| *byte != b'-').collect();
    if characters.len() != CODE_CHARS || !characters.iter().all(|byte| CODE_ALPHABET.contains(byte)) {
        return Err(LinkError::not_one(format!(
            "The code is not {CODE_CHARS} characters of Crockford base32, in three groups of four."
        )));
    }
    let text = String::from_utf8(characters).unwrap_or_default();
    Ok(format!("{}-{}-{}", &text[0..4], &text[4..8], &text[8..12]))
}

/// Whether `text` is one code of an invitation, as a person writes it.
#[must_use]
pub fn check_code(text: &str) -> bool {
    normalize_code(text).is_ok()
}

/// Whether `host` is one host of a link: an IP address, or a DNS name of at most
/// [`MAX_HOST_CHARS`] characters of letters, digits, `.` and `-`.
#[must_use]
pub fn check_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    !host.is_empty() && host.len() <= MAX_HOST_CHARS && host.split('.').all(is_label)
}

/// Whether one label of a DNS name follows its rule: 1 to 63 characters of
/// letters, digits and `-`, with no `-` at either end.
fn is_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `text` as a host and a port. An IPv6 address is in brackets, as a URL writes
/// it: `[fd7a:115c:a1e0::1]:7000`.
fn split_host_port(text: &str) -> Result<(String, u16), LinkError> {
    if text.is_empty() {
        return Err(LinkError::not_one("The address is empty."));
    }
    if text
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ' || matches!(byte, b'@' | b'?' | b'#'))
    {
        return Err(LinkError::not_one(
            "The address holds a space, a control character, or one of @ ? #.",
        ));
    }
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (inside, after) = rest
            .split_once(']')
            .ok_or_else(|| LinkError::not_one("The address opens a bracket and does not close it."))?;
        if inside.parse::<IpAddr>().is_err() {
            return Err(LinkError::not_one(
                "The address holds brackets around a text that is not an IP address.",
            ));
        }
        let after = after
            .strip_prefix(':')
            .ok_or_else(|| LinkError::not_one("The address holds no port after the bracket."))?;
        (inside, after)
    } else {
        let (host, port) = text
            .split_once(':')
            .ok_or_else(|| LinkError::not_one("The address holds no port."))?;
        if port.contains(':') {
            return Err(LinkError::not_one(
                "The address holds more than one colon, and an IPv6 address is in brackets.",
            ));
        }
        (host, port)
    };
    if !check_host(host) {
        return Err(LinkError::not_one(format!(
            "The host {host:?} is not an IP address or a DNS name of at most {MAX_HOST_CHARS} characters."
        )));
    }
    let port = parse_port(port)?;
    Ok((host.to_owned(), port))
}

fn parse_port(text: &str) -> Result<u16, LinkError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) || text.len() > 5 {
        return Err(LinkError::not_one("The port is not a number."));
    }
    text.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| LinkError::not_one("The port is 0, and a server listens on 1 to 65535."))
}

fn bracket_host(host: &str) -> String {
    if host.parse::<Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn authority(host: &str, port: u16) -> String {
    format!("{}:{port}", bracket_host(host))
}

fn inside_v4(address: Ipv4Addr) -> bool {
    let (network, bits) = OVERLAY_V4;
    let mask = u32::MAX << (32 - bits);
    u32::from(address) & mask == u32::from(network) & mask
}

fn inside_v6(address: Ipv6Addr) -> bool {
    let (network, bits) = OVERLAY_V6;
    let mask = u128::MAX << (128 - bits);
    u128::from(address) & mask == u128::from(network) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_rule_names_its_three_ranges() {
        for allowed in [
            "127.0.0.1",
            "127.255.255.254",
            "100.64.0.0",
            "100.100.100.100",
            "100.127.255.255",
            "::1",
            "fd7a:115c:a1e0::1",
            "fd7a:115c:a1e0:ffff:ffff:ffff:ffff:ffff",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = allowed.parse().expect("an address");
            assert!(allows_address(ip), "{allowed} is one address of a link");
            assert!(check_address(ip).is_ok(), "{allowed}");
        }
        for refused in [
            "0.0.0.0",
            "::",
            "8.8.8.8",
            "10.0.0.5",
            "192.168.1.10",
            "100.63.255.255",
            "100.128.0.0",
            "169.254.1.1",
            "::ffff:100.64.0.1",
            "fe80::1",
            "fd7a:115c:a1df::1",
            "fd7a:115c:a1e1::1",
        ] {
            let ip: IpAddr = refused.parse().expect("an address");
            assert!(!allows_address(ip), "{refused} is no address of a link");
            let error = check_address(ip).expect_err(refused);
            assert_eq!(error.code(), "address_not_allowed");
            let sentence = error.to_string();
            assert!(
                sentence.contains(refused) && sentence.contains("100.64.0.0/10"),
                "the refusal names the address and the rule: {sentence}"
            );
        }
    }

    #[test]
    fn a_join_link_is_one_scheme_one_address_one_path_and_one_code() {
        let link = JoinLink::parse("horizon://server-computer:7000/join#ABCD-EFGH-JKMN").expect("one link");
        assert_eq!(link.host(), "server-computer");
        assert_eq!(link.port(), 7000);
        assert_eq!(link.code(), "ABCD-EFGH-JKMN");
        assert_eq!(link.authority(), "server-computer:7000");
        assert_eq!(link.socket_addr(), None, "a name needs a lookup");
        assert_eq!(
            link.to_uri(),
            "horizon://server-computer:7000/join#ABCD-EFGH-JKMN",
            "the link reads back as the text of the QR code"
        );
        assert_eq!(JoinLink::parse(&link.to_uri()).expect("the same link"), link);

        // The same link without dashes, in lower case, with spaces that a paste
        // brings, and as an IPv6 address.
        for text in [
            "horizon://server-computer:7000/join#abcdefghjkmn",
            "  horizon://server-computer:7000/join#ABCDEFGHJKMN\n",
            "horizon://server-computer:7000/join#ABCD--EFGH-JKMN",
        ] {
            let parsed = JoinLink::parse(text).expect(text);
            assert_eq!(parsed.code(), "ABCD-EFGH-JKMN", "{text}");
            assert_eq!(parsed.host(), "server-computer");
        }
        let v6 = JoinLink::parse("horizon://[fd7a:115c:a1e0::1]:7000/join#ABCD-EFGH-JKMN").expect("an IPv6 link");
        assert_eq!(v6.host(), "fd7a:115c:a1e0::1");
        assert_eq!(v6.authority(), "[fd7a:115c:a1e0::1]:7000");
        assert_eq!(
            v6.socket_addr(),
            Some(SocketAddr::new("fd7a:115c:a1e0::1".parse().expect("an address"), 7000))
        );
        assert_eq!(
            JoinLink::new("127.0.0.1", 7001, "abcd-efgh-jkmn")
                .expect("a link")
                .to_uri(),
            "horizon://127.0.0.1:7001/join#ABCD-EFGH-JKMN"
        );
    }

    /// The text is untrusted: it is pasted by a person and it comes from a QR
    /// code. Each of these is one refusal and no panic.
    #[test]
    fn a_hostile_link_is_refused_with_one_sentence() {
        for text in [
            "",
            "horizon://",
            "horizon:///join#ABCD-EFGH-JKMN",
            "http://server:7000/join#ABCD-EFGH-JKMN",
            "HORIZON://server:7000/join#ABCD-EFGH-JKMN",
            "horizon:/server:7000/join#ABCD-EFGH-JKMN",
            "horizon://server/join#ABCD-EFGH-JKMN",
            "horizon://server:0/join#ABCD-EFGH-JKMN",
            "horizon://server:65536/join#ABCD-EFGH-JKMN",
            "horizon://server:port/join#ABCD-EFGH-JKMN",
            "horizon://server:7000/join/../join#ABCD-EFGH-JKMN",
            "horizon://server:7000/#ABCD-EFGH-JKMN",
            "horizon://server:7000/join",
            "horizon://server:7000/join#",
            "horizon://server:7000/join#SHORT",
            "horizon://server:7000/join#ABCD-EFGH-JKMN-XYZ",
            "horizon://server:7000/join#ABCD-EFGH-JKMI",
            "horizon://server:7000/join#ABCD-EFGH-JKML",
            "horizon://server:7000/join#ABCD-EFGH-JKMO",
            "horizon://server:7000/join#ABCD-EFGH-JKMU",
            "horizon://server:7000/join#ABCD-EFGH-JKMN#ABCD-EFGH-JKMN",
            "horizon://server:7000/join?code=ABCD-EFGH-JKMN",
            "horizon://person@server:7000/join#ABCD-EFGH-JKMN",
            "horizon://server:7000:7001/join#ABCD-EFGH-JKMN",
            "horizon://[fd7a:115c:a1e0::1/join#ABCD-EFGH-JKMN",
            "horizon://[not-an-address]:7000/join#ABCD-EFGH-JKMN",
            "horizon://serv er:7000/join#ABCD-EFGH-JKMN",
            "horizon://server\n:7000/join#ABCD-EFGH-JKMN",
            "horizon://server_name:7000/join#ABCD-EFGH-JKMN",
            "horizon://-server:7000/join#ABCD-EFGH-JKMN",
            "horizon://server..name:7000/join#ABCD-EFGH-JKMN",
            "horizon://.server:7000/join#ABCD-EFGH-JKMN",
            "horizon://servér:7000/join#ABCD-EFGH-JKMN",
            "horizon://server:7000/join#'; rm -rf /",
            "horizon://server:7000/join#$(id)",
            "horizon://server:7000/join#../../etc/passwd",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh server",
        ] {
            let error = JoinLink::parse(text).expect_err(&format!("{text:?} is refused"));
            assert_eq!(error.code(), "not_one", "{text:?}");
            let sentence = error.to_string();
            assert!(
                !sentence.contains('\n') && sentence.len() < 160,
                "the refusal is one short sentence: {sentence}"
            );
        }
        let long = "a".repeat(MAX_HOST_CHARS + 1);
        assert!(JoinLink::parse(&format!("horizon://{long}:7000/join#ABCD-EFGH-JKMN")).is_err());
        // A name of 253 characters is one host, and the code rule is separate.
        let longest = format!("{}.{}", "a".repeat(63), "b".repeat(63));
        assert!(check_host(&longest));
    }

    #[test]
    fn the_address_of_a_server_is_one_host_and_one_port() {
        for (text, host, port) in [
            ("server-computer:7000", "server-computer", 7000),
            (" 127.0.0.1:7000 ", "127.0.0.1", 7000),
            ("server-computer:7000\n", "server-computer", 7000),
            ("[fd7a:115c:a1e0::1]:7000", "fd7a:115c:a1e0::1", 7000),
        ] {
            let address = ServerAddress::parse(text).expect(text);
            assert_eq!(address.host(), host);
            assert_eq!(address.port(), port);
            assert_eq!(
                ServerAddress::parse(&address.to_string()).expect("its own text"),
                address
            );
        }
        assert_eq!(
            ServerAddress::parse("[::1]:7000").expect("an IPv6 address").authority(),
            "[::1]:7000"
        );
        assert_eq!(
            ServerAddress::parse("127.0.0.1:7000")
                .expect("an IPv4 address")
                .socket_addr(),
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7000))
        );
        for text in [
            "",
            "server",
            "server:",
            ":7000",
            "server:0",
            "server:7000:7001",
            "person@server:7000",
            "server:7000/path",
            "srv_name:7000",
        ] {
            let error = ServerAddress::parse(text).expect_err(text);
            assert_eq!(error.code(), "not_one", "{text:?}");
        }
        let long = format!("{}:7000", "a".repeat(MAX_ADDRESS_CHARS));
        assert!(ServerAddress::parse(&long).is_err());
    }

    #[test]
    fn a_code_is_twelve_characters_in_three_groups() {
        assert_eq!(normalize_code("abcd-efgh-jkmn").expect("a code"), "ABCD-EFGH-JKMN");
        assert_eq!(normalize_code("ABCDEFGHJKMN").expect("a code"), "ABCD-EFGH-JKMN");
        assert_eq!(normalize_code("ABCD-EFGH-JKMN").expect("a code"), "ABCD-EFGH-JKMN");
        for bad in [
            "",
            "ABCD",
            "ABCD-EFGH-JKM",
            "ABCD-EFGH-JKMNN",
            "ABCD-EFGH-JKMI",
            "ABCD EFGH JKMN",
        ] {
            assert!(!check_code(bad), "{bad:?} is no code");
        }
        assert_eq!(CODE_CHARS, 12);
        assert_eq!(CODE_ALPHABET.len(), 32, "Crockford base32 is 32 characters");
        for excluded in *b"ILOU" {
            assert!(!CODE_ALPHABET.contains(&excluded), "{} is left out", excluded as char);
        }
    }
}
