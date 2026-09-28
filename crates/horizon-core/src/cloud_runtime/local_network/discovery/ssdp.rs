//! One SSDP search (`UPnP`): devices answer this computer directly with their type, server and
//! the address of their description, which an agent can fetch through the bridge.
use super::{Finding, multicast_socket};
use horizon_cloud_protocol::local_network::discovery::{Service, Source, text};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

const GROUP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
/// Devices spread their answers over up to `MX` seconds.
const SEARCH: &[u8] =
    b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n";
/// UDP loses datagrams, so the search goes out twice.
const REPEAT: Duration = Duration::from_millis(300);
pub(super) const WINDOW: Duration = Duration::from_millis(2600);
/// The `UPnP` default; a search does not leave the local network.
const TTL: u32 = 2;
const MAX_PACKET: usize = 2048;
const MAX_RESPONSES: usize = 1024;
const MAX_ERRORS: usize = 16;

/// One device's answer to the search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Answer {
    kind: String,
    server: Option<String>,
    location: Option<String>,
    port: Option<u16>,
}

/// Reads one answer from `from`; anything but a successful HTTP-over-UDP answer is ignored.
pub(super) fn parse(from: Ipv4Addr, bytes: &[u8]) -> Option<Answer> {
    let message = std::str::from_utf8(bytes).ok()?;
    let mut lines = message.split("\r\n").flat_map(|line| line.split('\n'));
    let status = lines.next()?;
    if !status.starts_with("HTTP/1.") || status.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers
                .entry(name.trim().to_ascii_lowercase())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    let kind = headers
        .get("st")
        .map(|kind| text(kind))
        .filter(|kind| !kind.is_empty())?;
    // The description is useful only where the bridge can reach it: on the answering device.
    let location = headers
        .get("location")
        .and_then(|location| url::Url::parse(location).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str() == Some(&from.to_string()));
    Some(Answer {
        kind,
        server: headers
            .get("server")
            .map(|server| text(server))
            .filter(|server| !server.is_empty()),
        port: location.as_ref().and_then(url::Url::port_or_known_default),
        location: location.map(|url| text(url.as_str())),
    })
}

/// How much an answer says about what the device is: a `UPnP` device type best, then the root
/// device, then any service type.
fn rank(kind: &str) -> u8 {
    if kind.contains(":device:") {
        3
    } else if kind == "upnp:rootdevice" {
        2
    } else {
        u8::from(kind.starts_with("urn:"))
    }
}

/// One service per device and description, named by the most telling type it answered with.
pub(super) fn findings(answers: impl IntoIterator<Item = (Ipv4Addr, Answer)>) -> Vec<Finding> {
    let mut best: BTreeMap<(Ipv4Addr, Option<String>), Answer> = BTreeMap::new();
    for (address, answer) in answers {
        match best.entry((address, answer.location.clone())) {
            Entry::Vacant(entry) => {
                entry.insert(answer);
            }
            Entry::Occupied(mut entry) => {
                if rank(&answer.kind) > rank(&entry.get().kind) {
                    entry.insert(answer);
                }
            }
        }
    }
    let mut findings = Vec::with_capacity(best.len() * 2);
    for ((address, _), answer) in best {
        let mut attributes = BTreeMap::new();
        if let Some(server) = answer.server {
            attributes.insert("server".to_owned(), server);
        }
        if let Some(location) = answer.location {
            attributes.insert("location".to_owned(), location);
        }
        findings.push(Finding::Seen(address, Source::Ssdp));
        findings.push(Finding::Service(
            address,
            Service {
                source: Source::Ssdp,
                kind: answer.kind,
                name: None,
                port: answer.port,
                attributes,
            },
        ));
    }
    findings
}

/// Searches once from `local` and gathers answers for a few seconds.
pub(super) fn browse(local: Ipv4Addr) -> io::Result<Vec<Finding>> {
    search(&multicast_socket(local, TTL)?, GROUP.into())
}

/// The search over `socket`, sent to `group`.
pub(super) fn search(socket: &UdpSocket, group: SocketAddr) -> io::Result<Vec<Finding>> {
    socket.send_to(SEARCH, group)?;
    let start = Instant::now();
    let (until, mut repeated) = (start + WINDOW, false);
    let mut buffer = vec![0; MAX_PACKET];
    let (mut answers, mut errors) = (Vec::new(), 0);
    while let Some(left) = until
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
    {
        if !repeated && start.elapsed() >= REPEAT {
            socket.send_to(SEARCH, group)?;
            repeated = true;
        }
        let wait = if repeated {
            left
        } else {
            REPEAT.saturating_sub(start.elapsed()).max(Duration::from_millis(1))
        };
        socket.set_read_timeout(Some(wait))?;
        match socket.recv_from(&mut buffer) {
            Ok((count, SocketAddr::V4(from))) if answers.len() < MAX_RESPONSES => {
                if let Some(answer) = parse(*from.ip(), &buffer[..count]) {
                    answers.push((*from.ip(), answer));
                }
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) if errors < MAX_ERRORS => errors += 1,
            Err(error) => return Err(error),
        }
    }
    Ok(findings(answers))
}
