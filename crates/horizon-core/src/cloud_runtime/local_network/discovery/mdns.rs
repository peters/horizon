//! One short DNS-SD browse over mDNS. Queries leave from an ephemeral port on the bridged
//! address, so responders answer this computer directly (RFC 6762 section 6.7, "legacy
//! unicast"): nothing binds port 5353, joins a multicast group or answers for this computer.
use super::{Finding, multicast_socket};
use horizon_cloud_protocol::local_network::discovery::{MAX_ATTRIBUTES, MAX_NAMES, Service, Source, text};
use simple_dns::{CLASS, Name, Packet, PacketFlag, Question, TYPE, rdata::RData};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::{Ipv4Addr, SocketAddrV4},
    time::{Duration, Instant},
};

const GROUP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);
/// The DNS-SD query for every service type on the network (RFC 6763 section 9).
const SERVICE_TYPES: &str = "_services._dns-sd._udp.local";
/// Asked from the start as well, for devices that do not answer the service type query.
const COMMON_TYPES: [&str; 22] = [
    "_http._tcp",
    "_https._tcp",
    "_ipp._tcp",
    "_ipps._tcp",
    "_printer._tcp",
    "_pdl-datastream._tcp",
    "_uscan._tcp",
    "_ssh._tcp",
    "_sftp-ssh._tcp",
    "_rtsp._tcp",
    "_googlecast._tcp",
    "_airplay._tcp",
    "_raop._tcp",
    "_hap._tcp",
    "_smb._tcp",
    "_workstation._tcp",
    "_device-info._tcp",
    "_mqtt._tcp",
    "_home-assistant._tcp",
    "_esphomelib._tcp",
    "_octoprint._tcp",
    "_spotify-connect._tcp",
];
/// How long each round listens: service types, then instances, then their hosts.
const ROUNDS: [Duration; 3] = [
    Duration::from_millis(1200),
    Duration::from_millis(1200),
    Duration::from_millis(600),
];
const QUESTIONS_PER_PACKET: usize = 16;
const MAX_QUESTIONS: usize = 8 * QUESTIONS_PER_PACKET;
const MAX_TYPES: usize = 48;
const MAX_INSTANCES: usize = 512;
const MAX_HOSTS: usize = 512;
/// Neighbors asked for their names by reverse lookup.
pub(super) const MAX_REVERSE: usize = 64;
const MAX_PACKETS: usize = 1024;
const MAX_ERRORS: usize = 16;
/// mDNS allows up to 9000 bytes over UDP.
const MAX_PACKET: usize = 9000;

#[derive(Debug)]
struct Instance {
    /// Its full name as the device spelled it; labels may hold spaces and dots.
    full: Name<'static>,
    /// The service type without `.local`, such as `_ipp._tcp`.
    kind: String,
    name: String,
    target: Option<Name<'static>>,
    port: Option<u16>,
    attributes: BTreeMap<String, String>,
}

/// What one browse has learned so far, and what it has already asked.
#[derive(Debug, Default)]
pub(super) struct Browse {
    types: BTreeSet<String>,
    instances: BTreeMap<String, Instance>,
    hosts: BTreeMap<String, BTreeSet<Ipv4Addr>>,
    reverse: BTreeMap<Ipv4Addr, String>,
    asked: BTreeSet<(String, u16)>,
}

/// A name as this module keys it: lowercase, no trailing dot.
fn key(name: &Name<'_>) -> String {
    name.to_string().trim_end_matches('.').to_ascii_lowercase()
}

/// The service type of `name` if it is one, such as `_ipp._tcp.local`.
fn service_type(name: &Name<'_>) -> Option<String> {
    let labels = name.get_labels();
    let [kind, protocol, local] = labels else {
        return None;
    };
    let (kind, protocol) = (kind.to_string(), protocol.to_string());
    (kind.starts_with('_')
        && matches!(protocol.as_str(), "_tcp" | "_udp")
        && local.to_string().eq_ignore_ascii_case("local"))
    .then(|| format!("{kind}.{protocol}.local").to_ascii_lowercase())
}

/// The instance label and service type of an instance name such as `Office._ipp._tcp.local`.
fn instance(name: &Name<'_>) -> Option<(String, String)> {
    let labels = name.get_labels();
    let (first, rest) = labels.split_first()?;
    let kind = service_type(&Name::new_with_labels(rest))?;
    Some((first.to_string(), kind))
}

/// The address a reverse lookup name such as `5.1.168.192.in-addr.arpa` stands for.
fn reverse_address(name: &str) -> Option<Ipv4Addr> {
    let octets = name.strip_suffix(".in-addr.arpa")?;
    let mut parts: Vec<u8> = octets.split('.').map(str::parse).collect::<Result<_, _>>().ok()?;
    parts.reverse();
    let octets: [u8; 4] = parts.try_into().ok()?;
    Some(Ipv4Addr::from(octets))
}

fn reverse_name(address: Ipv4Addr) -> String {
    let [a, b, c, d] = address.octets();
    format!("{d}.{c}.{b}.{a}.in-addr.arpa")
}

impl Browse {
    /// The first round: every service type, the common ones, and the neighbors' names.
    pub(super) fn first_queries(&mut self, neighbors: &[Ipv4Addr]) -> Vec<Vec<u8>> {
        let names = std::iter::once(SERVICE_TYPES.to_owned())
            .chain(COMMON_TYPES.iter().map(|kind| format!("{kind}.local")))
            .chain(neighbors.iter().take(MAX_REVERSE).map(|address| reverse_name(*address)));
        let questions = names
            .filter_map(|name| Some((Name::new(&name).ok()?.into_owned(), TYPE::PTR)))
            .collect();
        self.packets(questions)
    }

    /// Follow-ups: instances of newly learned types, then the details and hosts still missing.
    pub(super) fn next_queries(&mut self) -> Vec<Vec<u8>> {
        let mut questions: Vec<_> = self
            .types
            .iter()
            .filter_map(|kind| Some((Name::new(kind).ok()?.into_owned(), TYPE::PTR)))
            .collect();
        for instance in self.instances.values() {
            if instance.target.is_none() {
                questions.push((instance.full.clone(), TYPE::SRV));
            }
            if instance.attributes.is_empty() {
                questions.push((instance.full.clone(), TYPE::TXT));
            }
            if let Some(target) = &instance.target
                && !self.hosts.contains_key(&key(target))
            {
                questions.push((target.clone(), TYPE::A));
            }
        }
        self.packets(questions)
    }

    /// Query packets for the questions not asked before, at most [`MAX_QUESTIONS`] a round.
    fn packets(&mut self, questions: Vec<(Name<'static>, TYPE)>) -> Vec<Vec<u8>> {
        let fresh: Vec<_> = questions
            .into_iter()
            .filter(|(name, kind)| self.asked.insert((key(name), u16::from(*kind))))
            .take(MAX_QUESTIONS)
            .collect();
        fresh
            .chunks(QUESTIONS_PER_PACKET)
            .filter_map(|chunk| {
                let mut packet = Packet::new_query(0);
                for (name, kind) in chunk {
                    packet
                        .questions
                        .push(Question::new(name.clone(), (*kind).into(), CLASS::IN.into(), false));
                }
                packet.build_bytes_vec_compressed().ok()
            })
            .collect()
    }

    /// Learns from one response; anything else, or anything malformed, is ignored.
    pub(super) fn absorb(&mut self, bytes: &[u8]) {
        let Ok(packet) = Packet::parse(bytes) else {
            return;
        };
        if !packet.has_flags(PacketFlag::RESPONSE) {
            return;
        }
        let records: Vec<_> = packet.answers.iter().chain(&packet.additional_records).collect();
        // Pointers first, so the details that follow in the same packet find their instance.
        for record in &records {
            if let RData::PTR(pointer) = &record.rdata {
                self.pointer(&record.name, &pointer.0);
            }
        }
        for record in &records {
            match &record.rdata {
                RData::SRV(service) => {
                    if let Some(instance) = self.instance(&record.name) {
                        instance.target = Some(service.target.clone().into_owned());
                        instance.port = Some(service.port).filter(|port| *port != 0);
                    }
                }
                RData::TXT(attributes) => {
                    if let Some(instance) = self.instance(&record.name) {
                        for (name, value) in attributes.iter_raw() {
                            // Binary values, such as keys, say nothing an agent can read.
                            let (Ok(name), Ok(value)) = (
                                std::str::from_utf8(name),
                                std::str::from_utf8(value.unwrap_or_default()),
                            ) else {
                                continue;
                            };
                            let name = text(name).to_ascii_lowercase();
                            if name.is_empty() || instance.attributes.len() > MAX_ATTRIBUTES {
                                continue;
                            }
                            instance.attributes.entry(name).or_insert_with(|| text(value));
                        }
                    }
                }
                RData::A(address) => {
                    let host = key(&record.name);
                    if self.hosts.len() < MAX_HOSTS || self.hosts.contains_key(&host) {
                        let addresses = self.hosts.entry(host).or_default();
                        if addresses.len() < MAX_NAMES {
                            addresses.insert(Ipv4Addr::from(address.address));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn pointer(&mut self, owner: &Name<'_>, target: &Name<'_>) {
        let owner_key = key(owner);
        if owner_key == SERVICE_TYPES {
            if let Some(kind) = service_type(target)
                && self.types.len() < MAX_TYPES
            {
                self.types.insert(kind);
            }
        } else if let Some(address) = reverse_address(&owner_key) {
            if self.reverse.len() < MAX_REVERSE {
                self.reverse.insert(address, key(target));
            }
        } else if service_type(owner).is_some() {
            self.instance(target);
        }
    }

    /// The instance named `name`, created on first sight while there is room.
    fn instance(&mut self, full: &Name<'_>) -> Option<&mut Instance> {
        let (label, kind) = instance(full)?;
        let name = key(full);
        if self.instances.len() >= MAX_INSTANCES && !self.instances.contains_key(&name) {
            return None;
        }
        Some(self.instances.entry(name).or_insert_with(|| Instance {
            full: full.clone().into_owned(),
            kind: kind.trim_end_matches(".local").to_owned(),
            name: text(&label),
            target: None,
            port: None,
            attributes: BTreeMap::new(),
        }))
    }

    /// Every device the browse can place at an address.
    pub(super) fn findings(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (host, addresses) in &self.hosts {
            for address in addresses {
                findings.push(Finding::Seen(*address, Source::Mdns));
                findings.push(Finding::Name(*address, text(host)));
            }
        }
        for (address, host) in self.reverse {
            findings.push(Finding::Seen(address, Source::Mdns));
            findings.push(Finding::Name(address, text(&host)));
        }
        for instance in self.instances.into_values() {
            let Some(addresses) = instance.target.as_ref().and_then(|target| self.hosts.get(&key(target))) else {
                continue;
            };
            for address in addresses {
                findings.push(Finding::Service(
                    *address,
                    Service {
                        source: Source::Mdns,
                        kind: instance.kind.clone(),
                        name: Some(instance.name.clone()).filter(|name| !name.is_empty()),
                        port: instance.port,
                        attributes: instance.attributes.clone(),
                    },
                ));
            }
        }
        findings
    }
}

/// Browses from `local` in a few short rounds, stopping early once nothing is left to ask.
pub(super) fn browse(local: Ipv4Addr, neighbors: &[Ipv4Addr]) -> io::Result<Vec<Finding>> {
    let socket = multicast_socket(local, 255)?;
    let mut browse = Browse::default();
    let mut queries = browse.first_queries(neighbors);
    let mut buffer = vec![0; MAX_PACKET];
    let (mut packets, mut errors) = (0, 0);
    for wait in ROUNDS {
        if queries.is_empty() {
            break;
        }
        for query in &queries {
            socket.send_to(query, GROUP)?;
        }
        let until = Instant::now() + wait;
        while let Some(left) = until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        {
            socket.set_read_timeout(Some(left))?;
            match socket.recv_from(&mut buffer) {
                Ok((count, _)) if packets < MAX_PACKETS => {
                    packets += 1;
                    browse.absorb(&buffer[..count]);
                }
                Ok(_) => {}
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => break,
                // An ICMP error from an earlier datagram must not end the browse, but a
                // socket that keeps failing must not spin until the round ends.
                Err(_) if errors < MAX_ERRORS => errors += 1,
                Err(error) => return Err(error),
            }
        }
        queries = browse.next_queries();
    }
    Ok(browse.findings())
}
