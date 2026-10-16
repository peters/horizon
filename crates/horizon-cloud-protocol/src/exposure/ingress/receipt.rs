use super::{REQUIRED_UNPRIVILEGED_PORT_START, listener_port};
use crate::exposure::{RESERVED_INGRESS_END, RESERVED_INGRESS_START};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, net::IpAddr};
use uuid::Uuid;

pub const MAX_RECEIPT_BYTES: usize = 16 * 1024;
pub const RECEIPT_VERSION: u16 = 1;

/// A bounded, internally consistent claim. It carries no host or runtime authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Receipt(ReceiptFacts);

/// Untrusted comparison facts, including when supplied by a saved receipt.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptFacts {
    pub version: u16,
    pub allocation: String,
    pub generation: Uuid,
    pub container: Container,
    pub unprivileged_port_start: u16,
    pub published: Vec<PublishedPort>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    pub id: String,
    pub requested_image: String,
    pub image_id: String,
    pub init_start_ticks: u64,
    pub netns: Namespace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedPort {
    pub protocol: Protocol,
    pub host_ip: IpAddr,
    pub host_port: u16,
    pub container_port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Invalid ingress receipt encoding or size")]
    Encoding,
    #[error("Invalid ingress receipt identity")]
    Identity,
    #[error("Invalid ingress receipt kernel boundary")]
    Kernel,
    #[error("Invalid ingress receipt publication set")]
    Publication,
}

impl Receipt {
    /// Decode at most 16 KiB and validate the complete comparison record.
    ///
    /// # Errors
    /// Refuses unknown/duplicate fields, invalid identities, unsafe claimed
    /// boundaries and any publication set other than the fixed complete layout.
    /// Successful parsing does not attest any of these claims.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_RECEIPT_BYTES {
            return Err(Error::Encoding);
        }
        let facts: ReceiptFacts = serde_json::from_slice(bytes).map_err(|_| Error::Encoding)?;
        if facts.version != RECEIPT_VERSION
            || !horizon_cloud::valid_id(&facts.allocation)
            || facts.generation.is_nil()
            || !hex_digest(&facts.container.id)
            || !pinned_image(&facts.container.requested_image)
            || !facts.container.image_id.strip_prefix("sha256:").is_some_and(hex_digest)
            || facts.container.init_start_ticks == 0
            || facts.container.netns.inode == 0
        {
            return Err(Error::Identity);
        }
        if facts.unprivileged_port_start < REQUIRED_UNPRIVILEGED_PORT_START {
            return Err(Error::Kernel);
        }
        validate_publication(&facts.published)?;
        Ok(Self(facts))
    }

    #[must_use]
    pub fn facts(&self) -> &ReceiptFacts {
        &self.0
    }
}

fn validate_publication(published: &[PublishedPort]) -> Result<(), Error> {
    if !matches!(published.len(), 33 | 66) {
        return Err(Error::Publication);
    }
    let actual: BTreeSet<_> = published.iter().copied().collect();
    if actual.len() != published.len() || actual.iter().any(|port| !port.host_ip.is_unspecified()) {
        return Err(Error::Publication);
    }
    let families: BTreeSet<_> = actual.iter().map(|port| port.host_ip).collect();
    let mut expected = BTreeSet::new();
    for host_ip in families {
        expected.insert(PublishedPort {
            protocol: Protocol::Tcp,
            host_ip,
            host_port: 22,
            container_port: 22,
        });
        for ingress in RESERVED_INGRESS_START..=RESERVED_INGRESS_END {
            let container_port = listener_port(ingress).ok_or(Error::Publication)?;
            expected.insert(PublishedPort {
                protocol: Protocol::Tcp,
                host_ip,
                host_port: ingress,
                container_port,
            });
        }
    }
    if actual == expected {
        Ok(())
    } else {
        Err(Error::Publication)
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn pinned_image(value: &str) -> bool {
    horizon_cloud::valid_image(value)
        && value
            .split_once("@sha256:")
            .is_some_and(|(_, digest)| hex_digest(digest))
}
