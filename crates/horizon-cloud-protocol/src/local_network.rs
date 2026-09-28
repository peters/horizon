//! The Local Network Bridge contract between the Horizon client and its worker.
//!
//! The client owns every policy decision: the worker receives only a Unix socket
//! whose connections reach the client's scope-checking SOCKS5 proxy.
pub mod discovery;

use std::{
    fmt,
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
    time::Duration,
};

/// Worker directory for the bridge socket, the helper's control socket and its status.
pub const DIRECTORY: &str = "/run/horizon-local-network";
/// Printed by `horizon-cloud-worker local-network prepare` on an image that supports the bridge.
pub const PREPARED: &str = "horizon-local-network=1";
/// Creates [`DIRECTORY`]. Its errors are folded into the output, so the client can tell an
/// image without the helper (the command or subcommand is unknown) from a real failure.
pub const PREPARE_COMMAND: &str = "horizon-cloud-worker local-network prepare 2>&1 || true";
/// How often the client writes one byte to the helper's input while the bridge is on.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// The helper stops, and removes its sockets and forwards, after this long without input.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

/// The narrowest scope that still has more than one other host.
const MAX_PREFIX: u8 = 30;
/// Wider networks are refused rather than shared whole.
pub const MIN_PREFIX: u8 = 16;

/// One bridge session: 32 lowercase hexadecimal characters, fresh for every SSH connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nonce(String);

impl Nonce {
    #[must_use]
    pub fn random() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        (value.len() == 32 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The worker path where sshd listens for this session's bridge connections.
    #[must_use]
    pub fn bridge_socket(&self) -> String {
        format!("{DIRECTORY}/{}.sock", self.0)
    }

    /// The helper that holds this session open on the worker.
    #[must_use]
    pub fn hold_command(&self, subnet: Subnet) -> String {
        format!("horizon-cloud-worker local-network hold {} {subnet}", self.0)
    }
}

impl fmt::Display for Nonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An IPv4 network in CIDR notation, `/16` to `/30`, with its host bits cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Subnet {
    network: Ipv4Addr,
    prefix: u8,
}

impl Subnet {
    /// The network containing `address`.
    ///
    /// # Errors
    /// Refuses prefixes wider than [`MIN_PREFIX`] or narrower than `/30`.
    pub fn containing(address: Ipv4Addr, prefix: u8) -> Result<Self, SubnetError> {
        if prefix < MIN_PREFIX {
            return Err(SubnetError::TooWide);
        }
        if prefix > MAX_PREFIX {
            return Err(SubnetError::TooNarrow);
        }
        Ok(Self {
            network: Ipv4Addr::from_bits(address.to_bits() & Self::mask(prefix)),
            prefix,
        })
    }

    fn mask(prefix: u8) -> u32 {
        u32::MAX << (32 - u32::from(prefix))
    }

    #[must_use]
    pub const fn network(self) -> Ipv4Addr {
        self.network
    }

    #[must_use]
    pub const fn prefix(self) -> u8 {
        self.prefix
    }

    #[must_use]
    pub fn broadcast(self) -> Ipv4Addr {
        Ipv4Addr::from_bits(self.network.to_bits() | !Self::mask(self.prefix))
    }

    #[must_use]
    pub fn contains(self, address: Ipv4Addr) -> bool {
        address.to_bits() & Self::mask(self.prefix) == self.network.to_bits()
    }

    /// Inside the network and neither its network nor its broadcast address.
    #[must_use]
    pub fn contains_host(self, address: Ipv4Addr) -> bool {
        self.contains(address) && address != self.network && address != self.broadcast()
    }
}

impl fmt::Display for Subnet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

impl FromStr for Subnet {
    type Err = SubnetError;

    /// Only the canonical form [`Subnet`] displays, so both sides agree on one spelling.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value.split_once('/').ok_or(SubnetError::Invalid)?;
        if prefix.is_empty() || prefix.len() > 2 || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(SubnetError::Invalid);
        }
        let address: Ipv4Addr = address.parse().map_err(|_| SubnetError::Invalid)?;
        let prefix: u8 = prefix.parse().map_err(|_| SubnetError::Invalid)?;
        let subnet = Self::containing(address, prefix)?;
        if subnet.network != address {
            return Err(SubnetError::Invalid);
        }
        Ok(subnet)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubnetError {
    #[error("Invalid local network")]
    Invalid,
    #[error("The local network is wider than /16; Local Network Bridge shares /16 or narrower networks")]
    TooWide,
    #[error("The local network has no other devices to share")]
    TooNarrow,
}

/// The line the helper prints once the bridge is usable on the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ready {
    /// The worker loopback SOCKS5 endpoint for tools and browsers.
    pub proxy: SocketAddrV4,
}

impl Ready {
    /// A proxy on the worker's loopback with a real port; anything else is not a ready helper.
    #[must_use]
    pub fn usable(&self) -> bool {
        self.proxy.ip().is_loopback() && self.proxy.port() != 0
    }
}

/// The SOCKS5 reply codes the client proxy answers with, and the refusal agents see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Succeeded,
    /// Limits reached or an internal error.
    GeneralFailure,
    /// The destination is outside the bridged subnet or is this computer.
    NotAllowed,
    /// This computer is no longer on the bridged network.
    NetworkUnreachable,
    HostUnreachable,
    ConnectionRefused,
    CommandNotSupported,
    AddressTypeNotSupported,
}

impl Reply {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Succeeded => 0,
            Self::GeneralFailure => 1,
            Self::NotAllowed => 2,
            Self::NetworkUnreachable => 3,
            Self::HostUnreachable => 4,
            Self::ConnectionRefused => 5,
            Self::CommandNotSupported => 7,
            Self::AddressTypeNotSupported => 8,
        }
    }

    /// Codes outside the subset read as a general failure.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Succeeded,
            2 => Self::NotAllowed,
            3 => Self::NetworkUnreachable,
            4 | 6 => Self::HostUnreachable,
            5 => Self::ConnectionRefused,
            7 => Self::CommandNotSupported,
            8 => Self::AddressTypeNotSupported,
            _ => Self::GeneralFailure,
        }
    }

    /// The refusal agents see.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Succeeded => "Connected",
            Self::GeneralFailure => {
                "The bridge refused the connection: its connection or data limit is reached, it is stopping, or it hit an internal error"
            }
            Self::NotAllowed => {
                "Outside the bridged local network: only the devices the owner shares are reachable, on the ports shared for each, and the Horizon computer only on ports the owner opened, as localhost, 127.0.0.1 or ::1"
            }
            Self::NetworkUnreachable => {
                "The Horizon computer is no longer on the bridged network; the owner must switch the bridge off and on"
            }
            Self::HostUnreachable => "Device not reachable from the Horizon computer",
            Self::ConnectionRefused => "The device refused the connection on that port",
            Self::CommandNotSupported => "Only TCP connections are bridged",
            Self::AddressTypeNotSupported => "Unsupported destination address",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnets_keep_one_canonical_spelling_and_the_prefix_bounds() {
        let subnet: Subnet = "192.168.1.0/24".parse().unwrap();
        assert_eq!(subnet.to_string(), "192.168.1.0/24");
        assert_eq!(subnet.broadcast(), Ipv4Addr::new(192, 168, 1, 255));
        assert_eq!(
            Subnet::containing(Ipv4Addr::new(10, 1, 2, 3), 16).unwrap().to_string(),
            "10.1.0.0/16"
        );
        for invalid in [
            "192.168.1.1/24",
            "192.168.1.0",
            "192.168.1.0/",
            "192.168.1.0/024",
            "192.168.1.0/+4",
            "192.168.1.0/24 ",
            "::1/24",
            "192.168.1.0/24;id",
        ] {
            assert_eq!(invalid.parse::<Subnet>(), Err(SubnetError::Invalid), "{invalid}");
        }
        assert_eq!("10.0.0.0/8".parse::<Subnet>(), Err(SubnetError::TooWide));
        assert_eq!("0.0.0.0/0".parse::<Subnet>(), Err(SubnetError::TooWide));
        assert_eq!("10.0.0.0/31".parse::<Subnet>(), Err(SubnetError::TooNarrow));
        assert_eq!("10.0.0.1/32".parse::<Subnet>(), Err(SubnetError::TooNarrow));
    }

    #[test]
    fn hosts_exclude_the_network_and_broadcast_addresses() {
        let subnet: Subnet = "192.168.1.0/30".parse().unwrap();
        assert!(!subnet.contains_host(Ipv4Addr::new(192, 168, 1, 0)));
        assert!(subnet.contains_host(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(subnet.contains_host(Ipv4Addr::new(192, 168, 1, 2)));
        assert!(!subnet.contains_host(Ipv4Addr::new(192, 168, 1, 3)));
        assert!(!subnet.contains_host(Ipv4Addr::new(192, 168, 1, 4)));
        assert!(!subnet.contains_host(Ipv4Addr::new(192, 168, 0, 255)));
    }

    #[test]
    fn nonces_are_fresh_lowercase_hex_and_build_the_worker_commands() {
        let nonce = Nonce::random();
        assert!(Nonce::parse(nonce.as_str()).is_some());
        assert_ne!(nonce, Nonce::random());
        for invalid in [
            "",
            "A".repeat(32).as_str(),
            "a".repeat(31).as_str(),
            "../../etc/passwd0000000000000000",
        ] {
            assert!(Nonce::parse(invalid).is_none(), "{invalid}");
        }
        let nonce = Nonce::parse(&"a".repeat(32)).unwrap();
        assert_eq!(
            nonce.bridge_socket(),
            format!("/run/horizon-local-network/{}.sock", "a".repeat(32))
        );
        assert_eq!(
            nonce.hold_command("192.168.1.0/24".parse().unwrap()),
            format!(
                "horizon-cloud-worker local-network hold {} 192.168.1.0/24",
                "a".repeat(32)
            )
        );
    }

    #[test]
    fn the_ready_line_is_strict_json() {
        let ready: Ready = serde_json::from_str(r#"{"proxy":"127.0.0.1:41234"}"#).unwrap();
        assert_eq!(ready.proxy, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 41234));
        assert!(serde_json::from_str::<Ready>(r#"{"proxy":"127.0.0.1:1","extra":1}"#).is_err());
        assert!(serde_json::from_str::<Ready>(r#"{"proxy":"[::1]:1"}"#).is_err());
        assert!(ready.usable());
        for unusable in [
            r#"{"proxy":"0.0.0.0:41234"}"#,
            r#"{"proxy":"192.168.1.5:41234"}"#,
            r#"{"proxy":"127.0.0.1:0"}"#,
        ] {
            assert!(!serde_json::from_str::<Ready>(unusable).unwrap().usable(), "{unusable}");
        }
    }

    #[test]
    fn reply_codes_round_trip_and_unknown_codes_are_general_failures() {
        for code in [0, 1, 2, 3, 4, 5, 7, 8] {
            assert_eq!(Reply::from_code(code).code(), code);
        }
        assert_eq!(Reply::from_code(6), Reply::HostUnreachable);
        assert_eq!(Reply::from_code(9), Reply::GeneralFailure);
        assert_eq!(Reply::from_code(255), Reply::GeneralFailure);
    }
}
