//! The owner's narrowing of a bridge's scope: which devices on the bridged network, on which
//! ports, and which of this computer's own loopback services. Only the owner sets them, on the
//! cloud card; nothing reads them from configuration or from an agent.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Devices one scope may name.
pub const MAX_DEVICES: usize = 32;
/// Ports one device, or this computer, may name.
pub const MAX_PORTS: usize = 16;

/// A device the bridge may reach, on `ports` or, when that is empty, on any port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub address: Ipv4Addr,
    pub ports: Vec<u16>,
}

/// What the owner allows beyond, or instead of, the whole bridged network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Empty: every device on the bridged network. Otherwise only these devices.
    pub devices: Vec<Device>,
    /// This computer's loopback services the worker may reach, by port. Empty: none.
    pub local_ports: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    #[error("{0} is not a device on the bridged network")]
    OutsideNetwork(Ipv4Addr),
    #[error("{0} is listed twice")]
    Duplicate(String),
    #[error("Port 0 is not a port")]
    PortZero,
    #[error(
        "At most {MAX_DEVICES} devices, each with at most {MAX_PORTS} ports, and at most {MAX_PORTS} ports on this computer"
    )]
    TooMany,
    #[error("Port {0} on this computer is the bridge itself")]
    BridgePort(u16),
}

impl Rules {
    /// Checks the rules against the bridged network before they apply. `on_network` says
    /// whether an address is a device the bridge could reach at all; `bridge_port` is the
    /// proxy's own loopback port, which would relay into itself.
    ///
    /// # Errors
    /// Names the first entry that cannot apply.
    pub(super) fn validate(&self, on_network: impl Fn(Ipv4Addr) -> bool, bridge_port: u16) -> Result<(), RulesError> {
        if self.devices.len() > MAX_DEVICES
            || self.local_ports.len() > MAX_PORTS
            || self.devices.iter().any(|device| device.ports.len() > MAX_PORTS)
        {
            return Err(RulesError::TooMany);
        }
        for (index, device) in self.devices.iter().enumerate() {
            if !on_network(device.address) {
                return Err(RulesError::OutsideNetwork(device.address));
            }
            if self.devices[..index]
                .iter()
                .any(|other| other.address == device.address)
            {
                return Err(RulesError::Duplicate(device.address.to_string()));
            }
            ports(&device.ports)?;
        }
        ports(&self.local_ports)?;
        if self.local_ports.contains(&bridge_port) {
            return Err(RulesError::BridgePort(bridge_port));
        }
        Ok(())
    }

    /// Whether a device the network admits may be reached on `port`.
    pub(super) fn permits_device(&self, address: Ipv4Addr, port: u16) -> bool {
        self.devices.is_empty()
            || self
                .devices
                .iter()
                .any(|device| device.address == address && (device.ports.is_empty() || device.ports.contains(&port)))
    }

    /// Whether a device the network admits may be reached on some port.
    pub(super) fn permits_host(&self, address: Ipv4Addr) -> bool {
        self.devices.is_empty() || self.devices.iter().any(|device| device.address == address)
    }

    /// The loopback addresses to try for `destination` on this computer, when the owner opened
    /// its port; `None` when the destination is not this computer's loopback. Only the
    /// loopback addresses themselves and the name `localhost` count: this computer's other
    /// addresses stay refused.
    pub(super) fn local(&self, host: LocalHost, port: u16) -> Option<Vec<SocketAddr>> {
        if !self.local_ports.contains(&port) {
            return None;
        }
        Some(match host {
            LocalHost::Name => vec![
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
            ],
            LocalHost::Address(address) => vec![SocketAddr::new(address, port)],
        })
    }

    /// Whether a relay to `address` that the scope admitted before may stay open.
    pub(super) fn keeps(&self, address: SocketAddr) -> bool {
        match address.ip().to_canonical() {
            ip if ip.is_loopback() => self.local_ports.contains(&address.port()),
            IpAddr::V4(ip) => self.permits_device(ip, address.port()),
            IpAddr::V6(_) => false,
        }
    }
}

/// How a destination names this computer's loopback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalHost {
    /// `localhost`, tried on both loopback addresses.
    Name,
    /// `127.0.0.1` or `::1` exactly.
    Address(IpAddr),
}

impl LocalHost {
    pub(super) fn of_name(name: &str) -> Option<Self> {
        name.trim_end_matches('.')
            .eq_ignore_ascii_case("localhost")
            .then_some(Self::Name)
    }

    /// Only the exact literals: an IPv4-mapped form such as `::ffff:127.0.0.1` is not one.
    pub(super) fn of_address(address: IpAddr) -> Option<Self> {
        (address == IpAddr::V4(Ipv4Addr::LOCALHOST) || address == IpAddr::V6(Ipv6Addr::LOCALHOST))
            .then_some(Self::Address(address))
    }
}

fn ports(ports: &[u16]) -> Result<(), RulesError> {
    for (index, port) in ports.iter().enumerate() {
        if *port == 0 {
            return Err(RulesError::PortZero);
        }
        if ports[..index].contains(port) {
            return Err(RulesError::Duplicate(format!("Port {port}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, 50)
    }

    fn only_camera_rtsp() -> Rules {
        Rules {
            devices: vec![Device {
                address: camera(),
                ports: vec![554],
            }],
            local_ports: vec![3000],
        }
    }

    #[test]
    fn no_devices_means_the_whole_network_and_a_list_means_only_those_ports() {
        let open = Rules::default();
        assert!(open.permits_device(camera(), 80) && open.permits_host(Ipv4Addr::new(192, 168, 1, 7)));
        let narrowed = only_camera_rtsp();
        assert!(narrowed.permits_device(camera(), 554));
        assert!(!narrowed.permits_device(camera(), 80));
        assert!(!narrowed.permits_device(Ipv4Addr::new(192, 168, 1, 7), 554));
        assert!(narrowed.permits_host(camera()) && !narrowed.permits_host(Ipv4Addr::new(192, 168, 1, 7)));
        let any_port = Rules {
            devices: vec![Device {
                address: camera(),
                ports: Vec::new(),
            }],
            local_ports: Vec::new(),
        };
        assert!(any_port.permits_device(camera(), 8080));
    }

    #[test]
    fn this_computer_is_reached_only_on_opened_loopback_ports() {
        let rules = only_camera_rtsp();
        assert_eq!(LocalHost::of_name("LocalHost."), Some(LocalHost::Name));
        assert_eq!(LocalHost::of_name("localhost.example"), None);
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(LocalHost::of_address(v4), Some(LocalHost::Address(v4)));
        assert_eq!(
            LocalHost::of_address(IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped())),
            None,
            "only the exact loopback literals open a port"
        );
        assert_eq!(LocalHost::of_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))), None);
        assert_eq!(LocalHost::of_address(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))), None);
        assert_eq!(
            rules.local(LocalHost::Name, 3000),
            Some(vec!["127.0.0.1:3000".parse().unwrap(), "[::1]:3000".parse().unwrap()])
        );
        assert_eq!(rules.local(LocalHost::Name, 22), None);
        assert_eq!(Rules::default().local(LocalHost::Address(v4), 3000), None);
    }

    #[test]
    fn open_relays_stay_only_while_the_rules_still_allow_them() {
        let rules = only_camera_rtsp();
        assert!(rules.keeps("192.168.1.50:554".parse().unwrap()));
        assert!(!rules.keeps("192.168.1.50:80".parse().unwrap()));
        assert!(!rules.keeps("192.168.1.7:554".parse().unwrap()));
        assert!(rules.keeps("127.0.0.1:3000".parse().unwrap()));
        assert!(rules.keeps("[::1]:3000".parse().unwrap()));
        assert!(!rules.keeps("127.0.0.1:22".parse().unwrap()));
        assert!(Rules::default().keeps("192.168.1.7:80".parse().unwrap()));
        assert!(!Rules::default().keeps("127.0.0.1:3000".parse().unwrap()));
    }

    #[test]
    fn rules_that_cannot_apply_are_refused_before_they_do() {
        let on_network = |address: Ipv4Addr| address.octets()[..3] == [192, 168, 1];
        assert_eq!(only_camera_rtsp().validate(on_network, 41234), Ok(()));
        let mut outside = only_camera_rtsp();
        outside.devices[0].address = Ipv4Addr::new(10, 0, 0, 5);
        assert_eq!(
            outside.validate(on_network, 41234),
            Err(RulesError::OutsideNetwork(Ipv4Addr::new(10, 0, 0, 5)))
        );
        let mut twice = only_camera_rtsp();
        twice.devices.push(twice.devices[0].clone());
        assert!(matches!(
            twice.validate(on_network, 41234),
            Err(RulesError::Duplicate(_))
        ));
        let mut zero = only_camera_rtsp();
        zero.local_ports.push(0);
        assert_eq!(zero.validate(on_network, 41234), Err(RulesError::PortZero));
        let mut repeated = only_camera_rtsp();
        repeated.devices[0].ports.push(554);
        assert!(matches!(
            repeated.validate(on_network, 41234),
            Err(RulesError::Duplicate(_))
        ));
        assert_eq!(
            only_camera_rtsp().validate(on_network, 3000),
            Err(RulesError::BridgePort(3000)),
            "the proxy never relays into itself"
        );
        let many = Rules {
            devices: (1..=u8::try_from(MAX_DEVICES + 1).unwrap())
                .map(|last| Device {
                    address: Ipv4Addr::new(192, 168, 1, last),
                    ports: Vec::new(),
                })
                .collect(),
            local_ports: Vec::new(),
        };
        assert_eq!(many.validate(on_network, 41234), Err(RulesError::TooMany));
    }
}
