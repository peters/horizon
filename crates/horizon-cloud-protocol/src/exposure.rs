//! Validated raw-TCP policy facts, never an owner approval or authenticated identity.
#[cfg(feature = "exposure-control")]
pub mod control;
pub mod ingress;
mod route;
pub use route::Route;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_SOURCE_RULES: usize = 32;
pub const MAX_CONNECTIONS: u32 = 128;
pub const EXPOSURE_LEASE_SECONDS: u64 = 8 * 60 * 60;
pub const RESERVED_INGRESS_START: u16 = 20000;
pub const RESERVED_INGRESS_END: u16 = 20031;
pub const RESERVED_BACKEND_START: u16 = 20032;
pub const RESERVED_BACKEND_END: u16 = 20063;

/// Fixed private backend paired with a producer virtual port; these are facts, not custody.
#[must_use]
pub const fn producer_backend_port(virtual_port: u16) -> Option<u16> {
    if virtual_port >= RESERVED_INGRESS_START && virtual_port <= RESERVED_INGRESS_END {
        Some(RESERVED_BACKEND_START + virtual_port - RESERVED_INGRESS_START)
    } else {
        None
    }
}
const BLOCKED_PORTS: [u16; 8] = [22, 5900, 9222, 2375, 1055, 1056, 47280, 47281];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureMode {
    #[default]
    Off,
    TailnetOnly,
    Restricted,
    Public,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceRule {
    Cidr(String),
    DnsName(String),
    Tag(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionLimits {
    pub total: u32,
    pub per_source: u32,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            total: MAX_CONNECTIONS,
            per_source: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureRequest {
    #[serde(default)]
    pub mode: ExposureMode,
    pub target_port: u16,
    #[serde(default)]
    pub sources: Vec<SourceRule>,
    #[serde(default)]
    pub limits: ConnectionLimits,
}

impl ExposureRequest {
    /// Decode bounded, untrusted configuration without granting runtime authority.
    ///
    /// # Errors
    /// Refuses oversized or malformed JSON, including unknown and duplicate fields.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Error::RequestSize);
        }
        serde_json::from_slice(bytes).map_err(|_| Error::Encoding)
    }

    /// Validate the complete policy before a worker can bind a listener.
    ///
    /// # Errors
    /// Refuses invalid ports, bounds, empty restrictions and malformed rules.
    /// Successful syntax validation is not a trusted peer or owner decision.
    pub fn validate(&self) -> Result<ExposurePolicy, Error> {
        if self.target_port == 0
            || ingress::is_physical_port(self.target_port)
            || BLOCKED_PORTS.contains(&self.target_port)
            || (RESERVED_INGRESS_START..=RESERVED_BACKEND_END).contains(&self.target_port)
        {
            return Err(Error::TargetPort);
        }
        if self.limits.total == 0
            || self.limits.total > MAX_CONNECTIONS
            || self.limits.per_source == 0
            || self.limits.per_source > self.limits.total
        {
            return Err(Error::ConnectionLimits);
        }
        if self.sources.len() > MAX_SOURCE_RULES {
            return Err(Error::SourceCount);
        }
        match self.mode {
            ExposureMode::Restricted if self.sources.is_empty() => return Err(Error::EmptySources),
            ExposureMode::Restricted => {}
            _ if !self.sources.is_empty() => return Err(Error::UnexpectedSources),
            _ => {}
        }
        let sources = self
            .sources
            .iter()
            .map(|source| match source {
                SourceRule::Cidr(value) => Cidr::parse(value).map(Rule::Cidr),
                SourceRule::DnsName(value) => dns_name(value).map(Rule::DnsName),
                SourceRule::Tag(value) => tag(value).map(Rule::Tag),
            })
            .collect::<Result<_, _>>()?;
        Ok(ExposurePolicy {
            mode: self.mode,
            target_port: self.target_port,
            sources,
            limits: self.limits,
        })
    }
}

/// Immutable validated policy. No deserializer can manufacture this type.
pub struct ExposurePolicy {
    mode: ExposureMode,
    target_port: u16,
    sources: Vec<Rule>,
    limits: ConnectionLimits,
}

impl ExposurePolicy {
    #[must_use]
    pub const fn mode(&self) -> ExposureMode {
        self.mode
    }
    #[must_use]
    pub const fn target_port(&self) -> u16 {
        self.target_port
    }
    #[must_use]
    pub const fn limits(&self) -> ConnectionLimits {
        self.limits
    }

    /// Pure CIDR matching; the worker must obtain this IP from its accepted socket.
    #[must_use]
    pub fn allows(&self, ip: IpAddr) -> bool {
        match self.mode {
            ExposureMode::Public => true,
            ExposureMode::Restricted => self
                .sources
                .iter()
                .any(|source| matches!(source, Rule::Cidr(cidr) if cidr.contains(canonical_ip(ip)))),
            ExposureMode::Off | ExposureMode::TailnetOnly => false,
        }
    }
    #[must_use]
    pub fn has_named_sources(&self) -> bool {
        self.sources.iter().any(|source| !matches!(source, Rule::Cidr(_)))
    }

    /// Pure policy facts, not authentication. The worker supplies these arguments
    /// only from the opaque peer that owns the original producer-verified stream.
    #[must_use]
    pub fn matches_tailnet(&self, source_ip: IpAddr, full_dns: &str, tags: &[String]) -> bool {
        self.mode == ExposureMode::Restricted
            && self.sources.iter().any(|source| match source {
                Rule::Cidr(cidr) => cidr.contains(canonical_ip(source_ip)),
                Rule::DnsName(name) => dns_name(full_dns).is_ok_and(|actual| name == &actual),
                Rule::Tag(tag) => tags.contains(tag),
            })
    }
}

#[must_use]
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip @ IpAddr::V4(_) => ip,
    }
}

enum Rule {
    Cidr(Cidr),
    DnsName(String),
    Tag(String),
}
fn dns_name(value: &str) -> Result<String, Error> {
    let name = value.strip_suffix('.').unwrap_or(value).to_ascii_lowercase();
    if name.len() > 253
        || !name.contains('.')
        || !name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::DnsName);
    }
    Ok(name)
}
fn tag(value: &str) -> Result<String, Error> {
    let body = value.strip_prefix("tag:").ok_or(Error::Tag)?;
    if body.is_empty()
        || body.len() > 63
        || !body.as_bytes()[0].is_ascii_alphabetic()
        || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(Error::Tag);
    }
    Ok(value.to_owned())
}

struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(value: &str) -> Result<Self, Error> {
        if value.len() > 254 {
            return Err(Error::Cidr);
        }
        let (address, prefix) = value.split_once('/').ok_or(Error::Cidr)?;
        let mut address: IpAddr = address.parse().map_err(|_| Error::Cidr)?;
        let mut bits: u8 = prefix.parse().map_err(|_| Error::Cidr)?;
        if bits.to_string() != prefix {
            return Err(Error::Cidr);
        }
        if let IpAddr::V6(v6) = address
            && let Some(v4) = v6.to_ipv4_mapped()
        {
            bits = bits.checked_sub(96).ok_or(Error::Cidr)?;
            address = IpAddr::V4(v4);
        }
        let width = if address.is_ipv4() { 32 } else { 128 };
        if bits > width {
            return Err(Error::Cidr);
        }
        Ok(Self {
            network: address,
            prefix: bits,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                network.to_bits() & mask == ip.to_bits() & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                network.to_bits() & mask == ip.to_bits() & mask
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Unsupported explicit exposure route or ingress observation")]
    Route,
    #[error("Exposure request exceeds its encoded size limit")]
    RequestSize,
    #[error("Invalid exposure request encoding")]
    Encoding,
    #[error("Service port is zero, reserved or blocked for management")]
    TargetPort,
    #[error("Invalid bounded connection limits")]
    ConnectionLimits,
    #[error("Too many source rules")]
    SourceCount,
    #[error("Restricted exposure requires a nonempty source list")]
    EmptySources,
    #[error("Source rules require Restricted mode")]
    UnexpectedSources,
    #[error("Invalid exposure CIDR")]
    Cidr,
    #[error("Invalid full tailnet DNS name")]
    DnsName,
    #[error("Invalid exact tailnet tag")]
    Tag,
    #[error("Tailnet membership, DNS and tag admission need a trusted accepted-connection producer")]
    IdentityUnavailable,
}

#[cfg(test)]
mod tests;
