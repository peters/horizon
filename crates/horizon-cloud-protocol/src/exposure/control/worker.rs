use super::Error;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Original daemon facts only. The actual held root stream and opaque daemon
/// observer, never this decoded value, supply the live observation boundary.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub self_id: String,
    pub full_dns_name: String,
    pub magic_dns_suffix: String,
    pub addresses: Vec<IpAddr>,
}

impl WorkerIdentity {
    pub(super) fn validate(&self) -> Result<(), Error> {
        if self.self_id.is_empty()
            || self.self_id.len() > 128
            || !self
                .self_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
            || !super::super::dns_name(&self.full_dns_name).is_ok_and(|name| name == self.full_dns_name)
            || !super::super::dns_name(&self.magic_dns_suffix).is_ok_and(|suffix| suffix == self.magic_dns_suffix)
            || !self
                .full_dns_name
                .strip_suffix(&self.magic_dns_suffix)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
            || self.addresses.is_empty()
            || self.addresses.len() > 16
            || self.addresses.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .addresses
                .iter()
                .any(|address| address.is_unspecified() || address.is_loopback() || address.is_multicast())
        {
            return Err(Error::Worker);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerObservation {
    /// Available only for an original no-tailnet direct ingress. Losing an
    /// original daemon pin must never downgrade its state to Unavailable.
    Unavailable {},
    Running {
        identity: WorkerIdentity,
    },
    Unverified {
        identity: WorkerIdentity,
    },
}

impl WorkerObservation {
    pub(super) fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Unavailable {} => Ok(()),
            Self::Running { identity } | Self::Unverified { identity } => identity.validate(),
        }
    }
}
