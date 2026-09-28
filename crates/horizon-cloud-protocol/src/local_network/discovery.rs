//! Discovery for Local Network Bridge: an agent on the worker asks which devices are on the
//! owner's network.
//!
//! Discovery runs on the owner's computer, never on the worker, because mDNS, SSDP and the
//! neighbor table do not travel through a TCP relay. Requests and answers share the bridge's
//! own SSH session: the helper on the worker writes one [`Call`] per line on its standard
//! output, and the owner's Horizon writes one [`Message`] per line on the helper's standard
//! input, between the empty heartbeat lines. The owner's Horizon announces what it answers
//! with a [`Hello`] first; a helper that never hears one knows discovery is unavailable.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::Ipv4Addr, time::Duration};

/// The discovery protocol version this build speaks.
pub const VERSION: u32 = 1;
/// The longest line the owner's Horizon writes; the helper discards longer lines unread.
pub const MAX_LINE: usize = 256 * 1024;
/// The longest call line the owner's Horizon reads.
pub const MAX_CALL: usize = 4096;
/// How long the helper waits for the owner's Horizon to answer one call.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_DEVICES: usize = 256;
pub const MAX_NAMES: usize = 4;
pub const MAX_SERVICES: usize = 16;
pub const MAX_ATTRIBUTES: usize = 8;
pub const MAX_PORTS: usize = 32;
/// Bytes of the message around an answer: its kind and number.
const ENVELOPE: usize = 256;
/// Characters in any one name, type, attribute or note; devices choose most of these strings.
pub const MAX_TEXT: usize = 128;
pub const MAX_NOTES: usize = 8;
/// Ports one probe tries, at most.
pub const MAX_PROBE_PORTS: usize = 16;
/// What a probe tries when the agent names no ports: remote shells, web interfaces, cameras,
/// printers, MQTT, dev servers and home automation hubs.
pub const DEFAULT_PROBE_PORTS: [u16; 14] = [
    22, 80, 443, 554, 631, 1883, 3000, 5000, 8000, 8080, 8123, 8443, 8554, 9100,
];
/// Host names are DNS names or IPv4 literals.
const MAX_HOST: usize = 253;

/// What the owner's Horizon answers, announced once per session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The discovery protocol [`VERSION`].
    pub discovery: u32,
    /// What this computer browses; [`Source::Probe`] when it also answers port probes.
    pub sources: Vec<Source>,
    /// What an agent should know about discovery on this computer's system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A request from the helper to the owner's Horizon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// The devices on the bridged network.
    Discover,
    /// Which of a few TCP ports one host accepts connections on.
    Probe {
        host: String,
        /// Empty for [`DEFAULT_PROBE_PORTS`].
        #[serde(default)]
        ports: Vec<u16>,
    },
}

impl Request {
    /// Refuses requests the owner's Horizon never runs, before anything is sent on the network.
    ///
    /// # Errors
    /// Explains the refusal in words an agent can act on.
    pub fn validate(&self) -> Result<(), String> {
        let Self::Probe { host, ports } = self else {
            return Ok(());
        };
        if host.trim().is_empty()
            || host.len() > MAX_HOST
            || host
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err("Name one device by its address or host name".into());
        }
        if ports.len() > MAX_PROBE_PORTS {
            return Err(format!("A probe tries at most {MAX_PROBE_PORTS} ports"));
        }
        if ports.contains(&0) {
            return Err("Ports are 1 to 65535".into());
        }
        Ok(())
    }

    /// The ports a probe tries, in order, without repeats and at most [`MAX_PROBE_PORTS`].
    #[must_use]
    pub fn probe_ports(ports: &[u16]) -> Vec<u16> {
        let ports = if ports.is_empty() {
            &DEFAULT_PROBE_PORTS[..]
        } else {
            ports
        };
        let mut chosen = Vec::with_capacity(MAX_PROBE_PORTS);
        for &port in ports {
            if port != 0 && !chosen.contains(&port) && chosen.len() < MAX_PROBE_PORTS {
                chosen.push(port);
            }
        }
        chosen
    }
}

/// One request, numbered so its answer finds the agent that asked. The owner's Horizon reads
/// `request` on its own, so a request it does not know is refused by number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call<R = Request> {
    pub id: u64,
    pub request: R,
}

/// A line from the owner's Horizon to the helper, besides the empty heartbeat lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Answer { id: u64, answer: Answer },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Discovery(Discovery),
    Probe(Probe),
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
    /// A port probe an agent asked for.
    Probe,
    /// A source added after this build.
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// The TCP connect results for one host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The host as the agent named it.
    pub host: String,
    pub address: Ipv4Addr,
    /// Accepted a connection.
    pub open: Vec<u16>,
    /// Refused the connection.
    pub closed: Vec<u16>,
    /// Did not answer in time; a firewall may drop connections there.
    pub silent: Vec<u16>,
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
        if self.notes.len() > MAX_NOTES {
            self.notes.truncate(MAX_NOTES);
            self.truncated = true;
        }
        for note in &mut self.notes {
            let short = text(note);
            self.truncated |= short != *note;
            *note = short;
        }
        self.devices.sort_by_key(|device| (!device.described(), device.address));
        if self.devices.len() > MAX_DEVICES {
            self.devices.truncate(MAX_DEVICES);
            self.truncated = true;
        }
        // Room for everything around the devices, the message that carries the answer
        // included; the flag is set in advance, since it only grows the envelope.
        let devices = std::mem::take(&mut self.devices);
        let truncated = std::mem::replace(&mut self.truncated, true);
        let envelope = serde_json::to_vec(&self).map_or(MAX_LINE, |bytes| bytes.len());
        (self.devices, self.truncated) = (devices, truncated);
        let mut room = MAX_LINE.saturating_sub(envelope + ENVELOPE);
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
    fn calls_and_messages_keep_their_wire_shape() {
        let call = Call {
            id: 7,
            request: Request::Discover,
        };
        assert_eq!(
            serde_json::to_string(&call).unwrap(),
            r#"{"id":7,"request":"discover"}"#
        );
        assert_eq!(
            serde_json::from_str::<Call>(r#"{"id":7,"request":"discover"}"#).unwrap(),
            call
        );
        // The helper's ready line never reads as a call.
        assert!(serde_json::from_str::<Call<serde_json::Value>>(r#"{"proxy":"127.0.0.1:1"}"#).is_err());
        // An unknown request still yields its number, so it can be refused.
        let unknown: Call<serde_json::Value> = serde_json::from_str(r#"{"id":8,"request":"sweep"}"#).unwrap();
        assert_eq!(unknown.id, 8);
        assert!(serde_json::from_value::<Request>(unknown.request).is_err());
        let answer = Message::Answer {
            id: 7,
            answer: Answer::Refused("busy".into()),
        };
        let line = serde_json::to_string(&answer).unwrap();
        assert_eq!(line, r#"{"answer":{"id":7,"answer":{"refused":"busy"}}}"#);
        assert_eq!(serde_json::from_str::<Message>(&line).unwrap(), answer);
        let hello: Message =
            serde_json::from_str(r#"{"hello":{"discovery":1,"sources":["mdns","sonar"],"later":1}}"#).unwrap();
        assert_eq!(
            hello,
            Message::Hello(Hello {
                discovery: 1,
                sources: vec![Source::Mdns, Source::Other],
                note: None
            })
        );
    }

    #[test]
    fn answers_round_trip_and_older_workers_read_newer_ones() {
        let answer = Answer::Refused("busy".into());
        let line = serde_json::to_string(&answer).unwrap();
        assert_eq!(line, r#"{"refused":"busy"}"#);
        assert_eq!(serde_json::from_str::<Answer>(&line).unwrap(), answer);
        // Worker images lag behind Horizon, so fields a newer Horizon adds are skipped.
        let newer = r#"{"discovery":{"age_seconds":1,"devices":[{"address":"192.168.1.5","sources":["mdns","sonar"],"model":"x"}],"later":true}}"#;
        let Answer::Discovery(discovery) = serde_json::from_str(newer).unwrap() else {
            panic!("discovery");
        };
        assert_eq!(discovery.devices[0].address, Ipv4Addr::new(192, 168, 1, 5));
        assert_eq!(discovery.devices[0].sources, [Source::Mdns, Source::Other]);
    }

    #[test]
    fn probes_name_one_host_and_a_few_real_ports() {
        let probe = |host: &str, ports: Vec<u16>| Request::Probe {
            host: host.into(),
            ports,
        };
        assert!(probe("192.168.1.5", vec![]).validate().is_ok());
        assert!(probe("printer.local", vec![80, 631]).validate().is_ok());
        for host in ["", " ", "a\nb", "two words", &"a".repeat(254)] {
            assert!(probe(host, vec![]).validate().is_err(), "{host:?}");
        }
        assert!(probe("host", vec![0]).validate().is_err());
        assert!(probe("host", (1..=17).collect()).validate().is_err());
        assert_eq!(Request::probe_ports(&[]), DEFAULT_PROBE_PORTS.to_vec());
        assert_eq!(Request::probe_ports(&[80, 80, 22]), vec![80, 22]);
        // Ports outside 1 to 65535 do not parse at all.
        assert!(serde_json::from_str::<Request>(r#"{"probe":{"host":"h","ports":[70000]}}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"probe":{"host":"h","ports":[-1]}}"#).is_err());
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

        let noisy = Discovery {
            notes: (0..12).map(|_| "e".repeat(10_000)).collect(),
            ..Discovery::default()
        }
        .bounded();
        assert!(noisy.truncated);
        assert_eq!(noisy.notes.len(), MAX_NOTES);
        assert!(noisy.notes.iter().all(|note| note.len() == MAX_TEXT));

        let small = Discovery {
            devices: vec![device(9, 1), device(3, 0)],
            ..Discovery::default()
        }
        .bounded();
        assert!(small.truncated, "attributes over the limit were dropped");
        assert_eq!(small.devices[0].address, Ipv4Addr::new(192, 168, 1, 3));
    }
}
