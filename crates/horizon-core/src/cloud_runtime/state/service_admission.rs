//! Retained one-service attempt metadata. A decoded record is never permission
//! to restart a gateway; only the current native owner controller admits one.
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4};
use uuid::Uuid;

pub(in crate::cloud_runtime) const RECORD_VERSION: u32 = 6;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerScope {
    pub host_instance: String,
    pub session_id: Uuid,
    pub workspace_id: String,
    pub cloud_id: String,
}

impl OwnerScope {
    /// Scope facts only; the caller must be the live native controller, after an
    /// explicit owner decision and durable saved-membership verification.
    /// # Errors
    /// Refuses malformed, nil or noncanonical identities before copying strings.
    pub fn new(host: &str, session: &str, workspace: &str, cloud: &str) -> Result<Self, super::Error> {
        if !crate::browser::manifest::valid_host_instance(host) {
            return Err(super::Error::Invalid("Invalid one-service owner scope"));
        }
        let host_instance = host.to_owned();
        let session_id = canonical_uuid(session)?;
        if !local_id(workspace) || !local_id(cloud) {
            return Err(super::Error::Invalid("Invalid one-service owner scope"));
        }
        Ok(Self {
            host_instance,
            session_id,
            workspace_id: workspace.to_owned(),
            cloud_id: cloud.to_owned(),
        })
    }

    fn valid(&self) -> bool {
        crate::browser::manifest::valid_host_instance(&self.host_instance)
            && !self.session_id.is_nil()
            && local_id(&self.workspace_id)
            && local_id(&self.cloud_id)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    operation_id: Uuid,
    generation: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum Binding {
    Lan {
        network: String,
        desktop: Ipv4Addr,
        interface: String,
    },
    Tailnet {
        desktop: String,
        peer: String,
        name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Checked {
    pub(in crate::cloud_runtime) host: String,
    pub(in crate::cloud_runtime) address: SocketAddrV4,
    pub(in crate::cloud_runtime) binding: Binding,
}

impl Checked {
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }
    #[must_use]
    pub const fn address(&self) -> SocketAddrV4 {
        self.address
    }
    #[must_use]
    pub const fn is_tailnet(&self) -> bool {
        matches!(self.binding, Binding::Tailnet { .. })
    }

    fn valid(&self) -> bool {
        if self.address.port() == 0
            || !host_name(&self.host)
            || self.host.parse::<Ipv4Addr>().is_ok_and(|ip| ip != *self.address.ip())
        {
            return false;
        }
        match &self.binding {
            Binding::Lan {
                network,
                desktop,
                interface,
            } => {
                network.len() <= 18
                    && opaque(interface)
                    && network
                        .parse::<horizon_cloud_protocol::local_network::Subnet>()
                        .is_ok_and(|subnet| {
                            subnet.contains_host(*desktop)
                                && subnet.contains_host(*self.address.ip())
                                && desktop != self.address.ip()
                        })
            }
            Binding::Tailnet { desktop, peer, name } => {
                opaque(desktop)
                    && opaque(peer)
                    && desktop != peer
                    && host_name(name)
                    && (self.host.parse::<Ipv4Addr>().is_ok() || self.host == *name)
                    && self.address.ip().octets()[0] == 100
                    && self.address.ip().octets()[1] & 0xc0 == 0x40
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Armed {},
    Attached { proxy_port: u16 },
    Closed {},
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "Fields")]
pub struct Record {
    owner: OwnerScope,
    worker_id: String,
    attempt: Identity,
    checked: Checked,
    phase: Phase,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    owner: OwnerScope,
    worker_id: String,
    attempt: Identity,
    checked: Checked,
    phase: Phase,
}

impl TryFrom<Fields> for Record {
    type Error = &'static str;
    fn try_from(fields: Fields) -> Result<Self, Self::Error> {
        let record = Self {
            owner: fields.owner,
            worker_id: fields.worker_id,
            attempt: fields.attempt,
            checked: fields.checked,
            phase: fields.phase,
        };
        if record.valid() {
            Ok(record)
        } else {
            Err("Invalid retained one-service admission")
        }
    }
}

impl Record {
    pub(in crate::cloud_runtime) fn valid(&self) -> bool {
        self.owner.valid()
            && opaque(&self.worker_id)
            && !self.attempt.operation_id.is_nil()
            && !self.attempt.generation.is_nil()
            && self.checked.valid()
            && !matches!(self.phase, Phase::Attached { proxy_port: 0 })
    }
    #[must_use]
    pub const fn owner(&self) -> &OwnerScope {
        &self.owner
    }
    #[must_use]
    pub const fn checked(&self) -> &Checked {
        &self.checked
    }
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }
    #[must_use]
    pub const fn attempt(&self) -> Identity {
        self.attempt
    }
    #[must_use]
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
}

fn local_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn opaque(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.trim() == value && value.bytes().all(|b| b.is_ascii_graphic())
}

fn host_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.bytes().any(|b| b.is_ascii_uppercase())
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn canonical_uuid(value: &str) -> Result<Uuid, super::Error> {
    if value.len() != 36 {
        return Err(super::Error::Invalid("Invalid one-service owner scope"));
    }
    let id = Uuid::parse_str(value).map_err(|_| super::Error::Invalid("Invalid one-service owner scope"))?;
    if id.is_nil() || id.to_string() != value {
        return Err(super::Error::Invalid("Invalid one-service owner scope"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests;
