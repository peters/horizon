//! The Local Network Bridge contract between the Horizon client and its worker.
//!
//! The client owns every policy decision: the worker only reaches the client's
//! scope-checking SOCKS5 proxy, and reads its refusals through [`Reply`].
use std::{fmt, net::Ipv4Addr, str::FromStr};

/// The narrowest scope that still has more than one other host.
const MAX_PREFIX: u8 = 30;
/// Wider networks are refused rather than shared whole.
pub const MIN_PREFIX: u8 = 16;

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

/// The SOCKS5 reply codes the client proxy answers with.
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
}
