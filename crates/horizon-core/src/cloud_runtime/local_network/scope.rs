//! Which destinations a bridge may reach: hosts on the network it started on, reached through
//! the same interface, never this computer.
use horizon_cloud_protocol::local_network::{Subnet, SubnetError};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
};

/// Probes in both halves of the IPv4 space, three of them documentation addresses. Routing a
/// datagram socket towards them selects the route without sending anything. Only the default
/// route covers all of them, so a more specific route that catches some makes them disagree,
/// and then no network is shared.
const ROUTE_PROBES: [Ipv4Addr; 4] = [
    Ipv4Addr::new(1, 1, 1, 1),
    Ipv4Addr::new(192, 0, 2, 1),
    Ipv4Addr::new(198, 51, 100, 1),
    Ipv4Addr::new(203, 0, 113, 1),
];
/// One address of a local interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Address {
    pub(super) ip: IpAddr,
    /// IPv4 prefix length; `None` for IPv6 and for point-to-point links.
    pub(super) prefix: Option<u8>,
    pub(super) interface: String,
}

/// This computer's addresses and the one that carries its default route, read afresh for each
/// connection so an address gained after the bridge started is still refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Host {
    pub(super) addresses: Vec<Address>,
    pub(super) route: Option<Ipv4Addr>,
}

/// The network a bridge started on: its subnet, and this computer's address and interface there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Network {
    pub(super) subnet: Subnet,
    pub(super) address: Ipv4Addr,
    pub(super) interface: String,
}

impl Host {
    /// # Errors
    /// Reports when the interface list cannot be read.
    pub(super) fn read() -> io::Result<Self> {
        let addresses = if_addrs::get_if_addrs()?
            .into_iter()
            .map(|interface| Address {
                ip: interface.ip(),
                prefix: match &interface.addr {
                    if_addrs::IfAddr::V4(address) if !interface.is_p2p() => Some(address.prefixlen),
                    _ => None,
                },
                interface: interface.name,
            })
            .collect();
        Ok(Self {
            addresses,
            route: agreed(ROUTE_PROBES.map(|probe| source_for(SocketAddr::new(probe.into(), 9)))),
        })
    }

    /// The network of the interface that carries the default route.
    ///
    /// # Errors
    /// Explains why there is no shareable current network.
    pub(super) fn current_network(&self) -> Result<Network, ScopeError> {
        let route = self.route.ok_or(ScopeError::NoNetwork)?;
        if !usable_host(route) {
            return Err(ScopeError::NoNetwork);
        }
        let address = self
            .addresses
            .iter()
            .find(|address| address.ip == IpAddr::V4(route))
            .ok_or(ScopeError::NoNetwork)?;
        Ok(Network {
            subnet: Subnet::containing(route, address.prefix.ok_or(ScopeError::PointToPoint)?)?,
            address: route,
            interface: address.interface.clone(),
        })
    }

    fn owns(&self, address: Ipv4Addr) -> bool {
        self.addresses
            .iter()
            .any(|own| own.ip.to_canonical() == IpAddr::V4(address))
    }
}

/// The source every probe leaves from, if they all agree.
fn agreed(sources: [Option<Ipv4Addr>; ROUTE_PROBES.len()]) -> Option<Ipv4Addr> {
    sources
        .iter()
        .all(|source| *source == sources[0])
        .then_some(sources[0])?
}

/// The local address this computer would send from to reach `destination`. Connecting a
/// datagram socket only consults the routing table; nothing is sent.
pub(super) fn source_for(destination: SocketAddr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(destination).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_unspecified() => Some(address),
        _ => None,
    }
}

/// Addresses no bridge reaches whatever its subnet.
fn usable_host(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.octets()[0] == 0
        || address.octets()[0] >= 240)
}

/// Whether one resolved destination is in scope; the caller has checked that `host` is still
/// on `network`. IPv4-mapped IPv6 addresses are judged as the IPv4 address they carry; every
/// other IPv6 address is outside the scope in this version. `source` names the local address
/// the connection would leave from: a more specific route inside the subnet, such as a VPN or
/// a virtual machine network, or a local address this computer did not list, leaves from
/// somewhere else and is refused.
pub(super) fn admits(
    network: &Network,
    host: &Host,
    destination: IpAddr,
    source: impl FnOnce() -> Option<Ipv4Addr>,
) -> bool {
    let IpAddr::V4(address) = destination.to_canonical() else {
        return false;
    };
    usable_host(address)
        && network.subnet.contains_host(address)
        && !host.owns(address)
        && source() == Some(network.address)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    #[error("This computer is not connected to a local IPv4 network")]
    NoNetwork,
    #[error("The current network is a point-to-point link with no other devices to share")]
    PointToPoint,
    #[error(transparent)]
    Subnet(#[from] SubnetError),
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    const LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    /// A host with `(address, prefix)` interfaces, the n-th named `ifn`, whose default route
    /// leaves from `route`.
    pub(in super::super) fn host(addresses: &[(&str, Option<u8>)], route: Option<&str>) -> Host {
        Host {
            addresses: addresses
                .iter()
                .enumerate()
                .map(|(index, (ip, prefix))| Address {
                    ip: ip.parse().unwrap(),
                    prefix: *prefix,
                    interface: format!("if{index}"),
                })
                .collect(),
            route: route.map(|route| route.parse().unwrap()),
        }
    }

    fn home() -> (Network, Host) {
        let host = host(
            &[
                ("192.168.1.20", Some(24)),
                ("127.0.0.1", Some(8)),
                ("172.17.0.1", Some(16)),
                ("fe80::1", None),
            ],
            Some("192.168.1.20"),
        );
        (host.current_network().unwrap(), host)
    }

    /// Decides as if every destination were routed out of the bridged interface.
    fn check(network: &Network, host: &Host, destination: IpAddr) -> bool {
        admits(network, host, destination, || Some(LAN))
    }

    #[test]
    fn hosts_on_the_default_route_subnet_are_allowed_and_cidr_edges_are_not() {
        let (network, host) = home();
        assert_eq!(
            (network.subnet.to_string(), network.address, network.interface.as_str()),
            ("192.168.1.0/24".into(), LAN, "if0")
        );
        for allowed in [v4(192, 168, 1, 1), v4(192, 168, 1, 50), v4(192, 168, 1, 254)] {
            assert!(check(&network, &host, allowed), "{allowed}");
        }
        for refused in [
            v4(192, 168, 1, 0),
            v4(192, 168, 1, 255),
            v4(192, 168, 0, 255),
            v4(192, 168, 2, 1),
            v4(8, 8, 8, 8),
            v4(10, 0, 0, 1),
        ] {
            assert!(!check(&network, &host, refused), "{refused}");
        }
    }

    #[test]
    fn destinations_routed_out_of_another_interface_are_refused() {
        let (network, host) = home();
        let camera = v4(192, 168, 1, 50);
        for source in [
            Some(Ipv4Addr::new(10, 8, 0, 2)),
            Some(Ipv4Addr::new(192, 168, 1, 50)),
            None,
        ] {
            assert!(!admits(&network, &host, camera, || source), "{source:?}");
        }
        assert!(source_for(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 9)).is_some_and(|source| source.is_loopback()));
    }

    #[test]
    fn this_computer_is_never_reachable_through_any_of_its_addresses() {
        let (network, mut host) = home();
        for own in [
            v4(192, 168, 1, 20),
            v4(127, 0, 0, 1),
            v4(127, 1, 2, 3),
            v4(172, 17, 0, 1),
            v4(0, 0, 0, 0),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(LAN.to_ipv6_mapped()),
            IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()),
        ] {
            assert!(!check(&network, &host, own), "{own}");
        }
        // An address gained later on another interface is refused too.
        host.addresses
            .extend(self::host(&[("192.168.1.21", Some(24))], None).addresses);
        assert!(!check(&network, &host, v4(192, 168, 1, 21)));
    }

    #[test]
    fn ipv6_is_refused_except_the_mapped_form_of_an_allowed_ipv4_host() {
        let (network, host) = home();
        for refused in [
            "fe80::1",
            "fe80::abcd",
            "::1",
            "2001:db8::1",
            "fd00::1",
            "ff02::1",
            "::192.168.1.50",
        ] {
            let address: IpAddr = refused.parse().unwrap();
            assert!(!check(&network, &host, address), "{refused}");
        }
        let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 1, 50).to_ipv6_mapped());
        assert!(check(&network, &host, mapped));
    }

    #[test]
    fn special_ipv4_ranges_are_refused_even_inside_a_matching_subnet() {
        let host = host(&[("169.254.3.4", Some(16))], Some("169.254.3.4"));
        assert_eq!(host.current_network(), Err(ScopeError::NoNetwork));
        for (special, prefix) in [
            (Ipv4Addr::new(169, 254, 9, 9), 16),
            (Ipv4Addr::new(224, 0, 0, 1), 24),
            (Ipv4Addr::BROADCAST, 24),
            (Ipv4Addr::new(240, 0, 0, 1), 24),
        ] {
            let network = Network {
                subnet: Subnet::containing(special, prefix).unwrap(),
                address: special,
                interface: "if0".into(),
            };
            assert!(
                !admits(&network, &host, IpAddr::V4(special), || Some(special)),
                "{special}"
            );
        }
    }

    #[test]
    fn the_same_subnet_on_another_address_or_interface_is_another_network() {
        let (network, host) = home();
        let mut elsewhere = host.clone();
        elsewhere.route = Some(Ipv4Addr::new(172, 17, 0, 1));
        assert_ne!(elsewhere.current_network().ok(), Some(network.clone()));
        let mut renamed = host.clone();
        renamed.addresses[0].interface = "if9".into();
        assert_ne!(renamed.current_network().ok(), Some(network.clone()));
        let mut moved = host;
        moved.addresses[0].ip = v4(192, 168, 1, 21);
        moved.route = Some(Ipv4Addr::new(192, 168, 1, 21));
        assert_ne!(moved.current_network().ok(), Some(network));
    }

    #[test]
    fn the_default_route_is_known_only_when_every_probe_agrees() {
        let lan = Some(LAN);
        assert_eq!(agreed([lan; 4]), lan);
        assert_eq!(agreed([lan, lan, Some(Ipv4Addr::new(10, 8, 0, 2)), lan]), None);
        assert_eq!(agreed([None, lan, lan, lan]), None);
        assert_eq!(agreed([None; 4]), None);
    }

    #[test]
    fn unshareable_networks_explain_why() {
        let mut host = host(&[("10.8.0.2", None)], Some("10.8.0.2"));
        assert_eq!(host.current_network(), Err(ScopeError::PointToPoint));
        host.addresses[0].prefix = Some(8);
        assert_eq!(host.current_network(), Err(ScopeError::Subnet(SubnetError::TooWide)));
        host.addresses[0].prefix = Some(32);
        assert_eq!(host.current_network(), Err(ScopeError::Subnet(SubnetError::TooNarrow)));
        host.route = Some(Ipv4Addr::new(10, 9, 0, 2));
        assert_eq!(host.current_network(), Err(ScopeError::NoNetwork));
        host.route = None;
        assert_eq!(host.current_network(), Err(ScopeError::NoNetwork));
    }
}
