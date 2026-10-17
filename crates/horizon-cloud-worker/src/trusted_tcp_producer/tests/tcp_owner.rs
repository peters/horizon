use super::super::procfs::bounded;
use super::{Error, live};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::{net::TcpStream, time::Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub accepted: u64,
    pub reverse: u64,
    pub local: SocketAddr,
    pub peer: SocketAddr,
}
struct Row {
    local: SocketAddr,
    peer: SocketAddr,
    state: u8,
    inode: u64,
}

pub(super) fn observe(stream: &TcpStream, until: Instant) -> Result<Binding, Error> {
    live(until)?;
    let inode = rustix::fs::fstat(stream)?.st_ino;
    let local = stream.local_addr()?;
    let peer = stream.peer_addr()?;
    if inode == 0 || !local.ip().is_loopback() || !peer.ip().is_loopback() {
        return Err(Error::Custody);
    }
    let mut rows = table(&bounded("/proc/self/net/tcp", 1024 * 1024)?, false)?;
    rows.extend(table(&bounded("/proc/self/net/tcp6", 1024 * 1024)?, true)?);
    live(until)?;
    binding(&rows, inode, local, peer)
}

pub(super) struct Proof {
    binding: Binding,
    descriptors: Vec<u32>,
}

pub(super) async fn prove_owned(stream: &TcpStream, pid: rustix::process::Pid, until: Instant) -> Result<Proof, Error> {
    loop {
        match prove_once(stream, pid, until) {
            Err(Error::Observation) => {
                tokio::time::timeout_at(until, tokio::time::sleep(std::time::Duration::from_millis(1)))
                    .await
                    .map_err(|_| Error::Deadline)?;
            }
            result => return result,
        }
    }
}

fn prove_once(stream: &TcpStream, pid: rustix::process::Pid, until: Instant) -> Result<Proof, Error> {
    let binding = observe(stream, until)?;
    let mut descriptors = Vec::new();
    for (index, entry) in std::fs::read_dir(format!("/proc/{pid}/fd"))?.enumerate() {
        live(until)?;
        if index >= 4096 {
            return Err(Error::Custody);
        }
        let entry = entry?;
        match std::fs::read_link(entry.path()) {
            Ok(link) if link == format!("socket:[{}]", binding.reverse) => {
                descriptors.push(
                    entry
                        .file_name()
                        .to_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or(Error::Custody)?,
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if descriptors.is_empty() {
        return Err(Error::Observation);
    }
    let proof = Proof { binding, descriptors };
    recheck(stream, pid, &proof, until)?;
    Ok(proof)
}

pub(super) fn recheck(
    stream: &TcpStream,
    pid: rustix::process::Pid,
    proof: &Proof,
    until: Instant,
) -> Result<(), Error> {
    let current = observe(stream, until)?;
    if current != proof.binding {
        return Err(Error::Observation);
    }
    for fd in &proof.descriptors {
        let link = std::fs::read_link(format!("/proc/{pid}/fd/{fd}"))?;
        if link != format!("socket:[{}]", proof.binding.reverse) {
            return Err(Error::Custody);
        }
    }
    live(until)
}

fn binding(rows: &[Row], inode: u64, local: SocketAddr, peer: SocketAddr) -> Result<Binding, Error> {
    let own: Vec<_> = rows.iter().filter(|r| r.local == local && r.peer == peer).collect();
    let reversed: Vec<_> = rows.iter().filter(|r| r.local == peer && r.peer == local).collect();
    if own.is_empty() {
        return Err(Error::Custody);
    }
    if own.len() != 1 || reversed.len() != 1 {
        return Err(Error::Observation);
    }
    let (a, b) = (own[0], reversed[0]);
    if a.inode != inode || b.inode == inode || matches!(a.state, 6 | 7 | 10) || matches!(b.state, 6 | 7 | 10) {
        return Err(Error::Custody);
    }
    // The TCP handshake can finish before the server calls accept and gains an FD.
    if b.inode == 0 {
        return Err(Error::Observation);
    }
    if !matches!((a.state, b.state), (1, 1) | (8, 4 | 5)) {
        return Err(Error::Observation);
    }
    Ok(Binding {
        accepted: inode,
        reverse: b.inode,
        local,
        peer,
    })
}

fn table(bytes: &[u8], v6: bool) -> Result<Vec<Row>, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Encoding)?;
    let mut lines = text.lines();
    if !lines
        .next()
        .is_some_and(|s| s.contains("local_address") && s.contains("inode"))
    {
        return Err(Error::Encoding);
    }
    lines
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 10 {
                return Err(Error::Encoding);
            }
            Ok(Row {
                local: address(fields[1], v6)?,
                peer: address(fields[2], v6)?,
                state: u8::from_str_radix(fields[3], 16).map_err(|_| Error::Encoding)?,
                inode: fields[9].parse().map_err(|_| Error::Encoding)?,
            })
        })
        .collect()
}

fn address(text: &str, v6: bool) -> Result<SocketAddr, Error> {
    let (ip, port) = text.split_once(':').ok_or(Error::Encoding)?;
    if port.len() != 4 || ip.len() != if v6 { 32 } else { 8 } {
        return Err(Error::Encoding);
    }
    let mut bytes = Vec::with_capacity(16);
    for chunk in ip.as_bytes().as_chunks::<8>().0 {
        let word = std::str::from_utf8(chunk).map_err(|_| Error::Encoding)?;
        bytes.extend(
            u32::from_str_radix(word, 16)
                .map_err(|_| Error::Encoding)?
                .to_ne_bytes(),
        );
    }
    let ip = if v6 {
        IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(bytes).map_err(|_| Error::Encoding)?,
        ))
    } else {
        IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).map_err(|_| Error::Encoding)?))
    };
    Ok(SocketAddr::new(
        ip,
        u16::from_str_radix(port, 16).map_err(|_| Error::Encoding)?,
    ))
}

#[tokio::test]
async fn connected_socket_requires_original_server_accept_before_ownership_passes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let until = Instant::now() + std::time::Duration::from_secs(3);
    let pid = rustix::process::getpid();
    assert!(matches!(prove_once(&client, pid, until), Err(Error::Observation)));
    let proof = prove_owned(&client, pid, until);
    tokio::pin!(proof);
    tokio::select! {
        _ = &mut proof => panic!("An unaccepted connection cannot prove server ownership"),
        () = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
    }
    let (accepted, _) = listener.accept().await.unwrap();
    let proof = proof.await.unwrap();
    assert_eq!(proof.binding.reverse, rustix::fs::fstat(&accepted).unwrap().st_ino);
    recheck(&client, pid, &proof, until).unwrap();
}

#[tokio::test]
async fn unaccepted_connection_expires_without_an_ownership_proof() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let until = Instant::now() + std::time::Duration::from_millis(25);
    assert!(matches!(
        prove_owned(&client, rustix::process::getpid(), until).await,
        Err(Error::Deadline)
    ));
}
