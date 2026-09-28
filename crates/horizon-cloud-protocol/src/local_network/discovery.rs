//! Discovery for Local Network Bridge: an agent on the worker asks which devices are on the
//! owner's network.
//!
//! Discovery runs on the owner's computer, never on the worker, because mDNS, SSDP and the
//! neighbor table do not travel through a TCP relay. The owner's Horizon sends each
//! [`Answer`] to the worker over the bridge's own SSH session.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::Ipv4Addr};

/// The longest answer line; the worker discards longer lines unread.
pub const MAX_LINE: usize = 256 * 1024;
pub const MAX_DEVICES: usize = 256;
pub const MAX_NAMES: usize = 4;
pub const MAX_SERVICES: usize = 16;
pub const MAX_ATTRIBUTES: usize = 8;
pub const MAX_PORTS: usize = 32;
/// Characters in any one name, type or attribute; devices choose these strings.
pub const MAX_TEXT: usize = 128;
pub const MAX_NOTES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    Discovery(Discovery),
    /// Why the owner's Horizon did not run the request, for example a rate limit.
    Refused(String),
}

/// Where a finding came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// mDNS / DNS-SD (Bonjour, Avahi).
    Mdns,
    /// SSDP (`UPnP`).
    Ssdp,
    /// The owner's computer recently exchanged traffic with the device.
    Neighbors,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// Seconds since the browse behind these results; requests close together share one.
    pub age_seconds: u64,
    pub devices: Vec<Device>,
    /// More devices, services or text were found than one answer carries.
    #[serde(default)]
    pub truncated: bool,
    /// What a source could not do on the owner's computer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub address: Ipv4Addr,
    /// Host names the device announces, such as `printer.local`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<Service>,
    /// TCP ports that devices advertise services on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub source: Source,
    /// A DNS-SD type such as `_ipp._tcp`, or a `UPnP` device type.
    pub kind: String,
    /// The instance name the device gives the service, such as `Office Printer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// DNS-SD TXT pairs, or the SSDP server and description location.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

/// Text a device chose, made safe to show: no control characters, at most [`MAX_TEXT`].
#[must_use]
pub fn text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_TEXT)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl Device {
    /// Applies the per-device limits; reports whether anything was dropped.
    fn bound(&mut self) -> bool {
        let before = (self.names.len(), self.services.len(), self.ports.len());
        self.names.truncate(MAX_NAMES);
        self.services.truncate(MAX_SERVICES);
        self.ports.sort_unstable();
        self.ports.dedup();
        self.ports.truncate(MAX_PORTS);
        let mut dropped = before != (self.names.len(), self.services.len(), self.ports.len());
        for service in &mut self.services {
            if service.attributes.len() > MAX_ATTRIBUTES {
                let keep: BTreeMap<_, _> = std::mem::take(&mut service.attributes)
                    .into_iter()
                    .take(MAX_ATTRIBUTES)
                    .collect();
                service.attributes = keep;
                dropped = true;
            }
        }
        dropped
    }

    /// Worth keeping over an address alone when an answer has to drop devices.
    fn described(&self) -> bool {
        !self.names.is_empty() || !self.services.is_empty()
    }
}

impl Discovery {
    /// Applies every limit and fits the answer in one [`MAX_LINE`] line, keeping described
    /// devices over bare addresses. Devices come out in address order.
    #[must_use]
    pub fn bounded(mut self) -> Self {
        for device in &mut self.devices {
            self.truncated |= device.bound();
        }
        self.notes.truncate(MAX_NOTES);
        self.devices.sort_by_key(|device| (!device.described(), device.address));
        if self.devices.len() > MAX_DEVICES {
            self.devices.truncate(MAX_DEVICES);
            self.truncated = true;
        }
        // Room for the envelope, the notes and the flags around the devices.
        let mut room = MAX_LINE.saturating_sub(4096);
        let mut kept = 0;
        for device in &self.devices {
            let size = serde_json::to_vec(device).map_or(usize::MAX, |bytes| bytes.len() + 1);
            if size > room {
                break;
            }
            room -= size;
            kept += 1;
        }
        if kept < self.devices.len() {
            self.devices.truncate(kept);
            self.truncated = true;
        }
        self.devices.sort_by_key(|device| device.address);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(last: u8, services: usize) -> Device {
        Device {
            address: Ipv4Addr::new(192, 168, 1, last),
            names: Vec::new(),
            services: (0..services)
                .map(|index| Service {
                    source: Source::Mdns,
                    kind: "_http._tcp".into(),
                    name: Some(format!("Device {last} service {index}")),
                    port: Some(80),
                    attributes: (0..12).map(|key| (format!("key{key}"), "v".repeat(MAX_TEXT))).collect(),
                })
                .collect(),
            ports: vec![80, 22, 80],
            sources: vec![Source::Mdns],
        }
    }

    #[test]
    fn answers_are_strict_json() {
        let answer = Answer::Refused("busy".into());
        let line = serde_json::to_string(&answer).unwrap();
        assert_eq!(line, r#"{"refused":"busy"}"#);
        assert_eq!(serde_json::from_str::<Answer>(&line).unwrap(), answer);
        assert!(serde_json::from_str::<Answer>(r#"{"refused":"busy","extra":1}"#).is_err());
    }

    #[test]
    fn device_text_is_printable_and_short() {
        assert_eq!(text(" Office\u{1b}[31m Printer\n "), "Office[31m Printer");
        assert_eq!(text(&"x".repeat(500)).len(), MAX_TEXT);
    }

    #[test]
    fn answers_keep_their_limits_and_fit_one_line() {
        let mut devices: Vec<_> = (1..=250).map(|last| device(last, 20)).collect();
        devices.extend((1..=40).map(|last| Device {
            address: Ipv4Addr::new(192, 168, 2, last),
            names: Vec::new(),
            services: Vec::new(),
            ports: Vec::new(),
            sources: vec![Source::Neighbors],
        }));
        let discovery = Discovery {
            devices,
            ..Discovery::default()
        }
        .bounded();
        assert!(discovery.truncated);
        assert!(serde_json::to_vec(&discovery).unwrap().len() < MAX_LINE);
        assert!(discovery.devices.iter().all(|device| {
            device.services.len() <= MAX_SERVICES
                && device.ports == [22, 80]
                && device
                    .services
                    .iter()
                    .all(|service| service.attributes.len() <= MAX_ATTRIBUTES)
        }));
        assert!(discovery.devices.is_sorted_by_key(|device| device.address));
        // Described devices win over bare addresses when room runs out.
        assert!(discovery.devices.iter().all(Device::described));

        let small = Discovery {
            devices: vec![device(9, 1), device(3, 0)],
            ..Discovery::default()
        }
        .bounded();
        assert!(small.truncated, "attributes over the limit were dropped");
        assert_eq!(small.devices[0].address, Ipv4Addr::new(192, 168, 1, 3));
    }
}
