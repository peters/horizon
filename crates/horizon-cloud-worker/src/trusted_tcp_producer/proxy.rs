use super::{Error, ancillary};
use std::net::SocketAddr;
use tokio::{
    net::UnixStream,
    time::{Instant, timeout_at},
};

pub(super) struct Header {
    pub source: SocketAddr,
    pub destination: SocketAddr,
}

pub(super) async fn read(stream: &UnixStream, until: Instant) -> Result<Header, Error> {
    let mut line = Vec::with_capacity(108);
    line.push(b'P');
    timeout_at(until, async {
        loop {
            if line.len() == 108 {
                return Err(Error::Encoding);
            }
            line.push(ancillary::byte(stream, false).await?.0.ok_or(Error::Encoding)?);
            if line.ends_with(b"\r\n") {
                return parse(&line);
            }
        }
    })
    .await
    .map_err(|_| Error::Deadline)?
}

pub(super) fn parse(bytes: &[u8]) -> Result<Header, Error> {
    if bytes.len() > 108 || !bytes.ends_with(b"\r\n") {
        return Err(Error::Encoding);
    }
    let text = std::str::from_utf8(&bytes[..bytes.len() - 2]).map_err(|_| Error::Encoding)?;
    let fields: Vec<_> = text.split(' ').collect();
    if fields.len() != 6 || fields[0] != "PROXY" || !matches!(fields[1], "TCP4" | "TCP6") {
        return Err(Error::Encoding);
    }
    let mut addresses = Vec::with_capacity(2);
    for (ip, port) in [(fields[2], fields[4]), (fields[3], fields[5])] {
        let ip: std::net::IpAddr = ip.parse().map_err(|_| Error::Encoding)?;
        let number = port.parse::<u16>().map_err(|_| Error::Encoding)?;
        if number == 0
            || number.to_string() != port
            || ip.is_unspecified()
            || ip.is_multicast()
            || ip.is_ipv4() != (fields[1] == "TCP4")
            || matches!(ip, std::net::IpAddr::V6(a) if a.to_ipv4_mapped().is_some())
        {
            return Err(Error::Encoding);
        }
        addresses.push(SocketAddr::new(ip, number));
    }
    Ok(Header {
        source: addresses[0],
        destination: addresses[1],
    })
}
