//! Read-only metadata from a root-owned installed Linux Tailscale CLI. No PATH lookup,
//! enrollment, credential storage or API mutation. Status/WhoIs values are never logged.
use super::Error;
use crate::cloud_runtime::{Cancellation, command::Runner};
use serde::de::DeserializeOwned;
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(2);
const SOCKET: &str = "/run/tailscale/tailscaled.sock";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub(super) id: String,
    pub(super) addresses: Vec<Ipv4Addr>,
}

pub(super) struct Peer {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) addresses: Vec<Ipv4Addr>,
}

pub(super) struct Client {
    program: PathBuf,
    cancel: Cancellation,
}

impl Client {
    pub(super) fn existing() -> Result<Self, Error> {
        #[cfg(target_os = "linux")]
        {
            for path in ["/usr/bin/tailscale", "/usr/local/bin/tailscale"] {
                if trusted(Path::new(path), PathKind::Program) && trusted(Path::new(SOCKET), PathKind::Socket) {
                    return Ok(Self {
                        program: PathBuf::from(path),
                        cancel: Cancellation::default(),
                    });
                }
            }
            Err(Error::SourceUnavailable)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::UnsupportedSource)
        }
    }

    pub(super) fn cancel(&self) {
        self.cancel.cancel();
    }

    fn read<T: DeserializeOwned>(&self, arguments: &[&str]) -> Result<T, Error> {
        if !trusted(&self.program, PathKind::Program) || !trusted(Path::new(SOCKET), PathKind::Socket) {
            return Err(Error::SourceUnavailable);
        }
        let mut command = self.command(arguments);
        let runner = Runner {
            cancel: &self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        // The existing private exchange caps the reply at 64 KiB and kills its owned
        // command group on timeout/cancellation. It logs neither status nor stderr.
        let bytes = Zeroizing::new(
            runner
                .private_exchange(&mut command, &[], TIMEOUT)
                .map_err(|_| Error::SourceUnavailable)?,
        );
        serde_json::from_slice(&bytes).map_err(|_| Error::SourceUnavailable)
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command.env_clear().arg(format!("--socket={SOCKET}")).args(arguments);
        command
    }

    pub(super) fn desktop(&self) -> Result<Identity, Error> {
        let status: Status = self.read(&["status", "--json", "--peers=false"])?;
        status.identity()
    }

    pub(super) fn peer(&self, ip: Ipv4Addr) -> Result<Peer, Error> {
        let reply: WhoIs = self.read(&["whois", "--json", &ip.to_string()])?;
        reply.peer()
    }
}

fn addresses(values: Vec<String>) -> Result<Vec<Ipv4Addr>, Error> {
    if values.len() > 16 {
        return Err(Error::SourceUnavailable);
    }
    let mut addresses: Vec<_> = values.into_iter().filter_map(|address| address.parse().ok()).collect();
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_graphic())
}

#[derive(Clone, Copy)]
enum PathKind {
    Program,
    Socket,
}

#[cfg(unix)]
fn trusted(path: &Path, kind: PathKind) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    path.ancestors().all(|part| {
        std::fs::symlink_metadata(part).is_ok_and(|m| {
            !m.file_type().is_symlink()
                && m.uid() == 0
                && if part == path {
                    match kind {
                        PathKind::Program => {
                            m.is_file() && m.permissions().mode() & 0o022 == 0 && m.permissions().mode() & 0o111 != 0
                        }
                        PathKind::Socket => m.file_type().is_socket(),
                    }
                } else {
                    m.is_dir() && m.permissions().mode() & 0o022 == 0
                }
        })
    })
}

#[derive(serde::Deserialize)]
struct Status {
    #[serde(rename = "BackendState")]
    backend: String,
    #[serde(rename = "TUN")]
    kernel: bool,
    #[serde(rename = "Self")]
    node: SelfNode,
}

#[derive(serde::Deserialize)]
struct SelfNode {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "TailscaleIPs")]
    addresses: Vec<String>,
}

#[derive(serde::Deserialize)]
struct WhoIs {
    #[serde(rename = "Node")]
    node: PeerNode,
}

#[derive(serde::Deserialize)]
struct PeerNode {
    #[serde(rename = "StableID")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Addresses")]
    addresses: Vec<String>,
}

impl Status {
    fn identity(self) -> Result<Identity, Error> {
        if self.backend != "Running" || !self.kernel {
            return Err(Error::UnsupportedSource);
        }
        let identity = Identity {
            id: self.node.id,
            addresses: addresses(self.node.addresses)?,
        };
        if !identifier(&identity.id) || identity.addresses.is_empty() {
            return Err(Error::SourceUnavailable);
        }
        Ok(identity)
    }
}

impl WhoIs {
    fn peer(self) -> Result<Peer, Error> {
        let name = self
            .node
            .name
            .strip_suffix('.')
            .unwrap_or(&self.node.name)
            .to_ascii_lowercase();
        if !identifier(&self.node.id)
            || !name.contains('.')
            || name.parse::<std::net::IpAddr>().is_ok()
            || super::Endpoint::new(&name, 1).is_err()
            || self.node.addresses.len() > 16
        {
            return Err(Error::SourceUnavailable);
        }
        let mut addresses: Vec<Ipv4Addr> = self
            .node
            .addresses
            .into_iter()
            .filter_map(|address| {
                let (ip, prefix) = address.split_once('/')?;
                (prefix == "32").then(|| ip.parse().ok()).flatten()
            })
            .collect();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(Error::SourceUnavailable);
        }
        Ok(Peer {
            id: self.node.id,
            name,
            addresses,
        })
    }
}

#[cfg(test)]
#[path = "tests/tailnet.rs"]
mod tests;
