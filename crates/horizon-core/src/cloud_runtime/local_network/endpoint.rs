//! An exact TCP endpoint gate using the existing bridge relay. Host-controller code
//! must authorize the recipient and human admission before starting this primitive.
//! No worker, UI, CLI or MCP path starts it; the feature is off by default.
mod admission;
pub use admission::{Admitted, Recipient, RecipientView, StatusSnapshot as AdmissionStatus};
pub mod attachment;
mod source;
mod tailnet;
#[cfg(test)]
mod tests;

use super::{Counters, Destination, Relay, socks};
use source::{Binding, Source};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Choose one IPv4 host and a TCP port from 1 to 65535")]
    InvalidEndpoint,
    #[error("The approved endpoint or its source binding changed; approve it again")]
    BindingChanged,
    #[error("The endpoint source is unavailable")]
    SourceUnavailable,
    #[error("This endpoint source is not supported by the gateway")]
    UnsupportedSource,
    #[error("The endpoint source is being checked; try again")]
    Busy,
    #[error("The gateway could not start")]
    StartFailed,
    #[error("This receipt belongs to another gateway generation")]
    StaleReceipt,
}

/// Input only: constructing it grants no access or owner authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    host: String,
    port: u16,
}

impl Endpoint {
    /// # Errors
    /// Rejects URLs, malformed names, IPv6 and port zero before any lookup.
    pub fn new(host: &str, port: u16) -> Result<Self, Error> {
        let host = host.strip_suffix('.').unwrap_or(host);
        if port == 0 || host.is_empty() || host.len() > 253 {
            return Err(Error::InvalidEndpoint);
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if !ip.is_ipv4() {
                return Err(Error::InvalidEndpoint);
            }
        } else if host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }) {
            return Err(Error::InvalidEndpoint);
        }
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    fn destination(&self) -> Destination {
        match self.host.parse::<IpAddr>() {
            Ok(ip) => Destination::Address(SocketAddr::new(ip, self.port)),
            Err(_) => Destination::Name(self.host.clone(), self.port),
        }
    }

    fn matches(&self, destination: &Destination, address: SocketAddr) -> bool {
        match destination {
            Destination::Address(requested) => {
                SocketAddr::new(requested.ip().to_canonical(), requested.port()) == address
            }
            Destination::Name(name, port) => {
                *port == self.port && name.strip_suffix('.').unwrap_or(name).eq_ignore_ascii_case(&self.host)
            }
        }
    }
}

/// A read-only preflight. It is not a public actor authorization or a persisted grant.
pub struct Prepared {
    attempt: Attempt,
    endpoint: Endpoint,
    source: Source,
    binding: Binding,
}

impl Prepared {
    /// Identity available before binding, for the host controller's durable intent.
    #[must_use]
    pub const fn attempt(&self) -> Attempt {
        self.attempt
    }

    /// Exact checked endpoint displayed to the native owner before acceptance.
    /// This snapshot does not authorize a gateway.
    /// # Errors
    /// Refuses an unsupported address family before returning an approval view.
    pub fn checked(&self) -> Result<crate::cloud_runtime::state::service_admission::Checked, Error> {
        use crate::cloud_runtime::state::service_admission::{Binding as Retained, Checked};
        let binding = match &self.binding {
            Binding::Lan { network, .. } => Retained::Lan {
                network: network.subnet.to_string(),
                desktop: network.address,
                interface: network.interface.clone(),
            },
            Binding::Tailnet {
                desktop, peer, name, ..
            } => Retained::Tailnet {
                desktop: desktop.clone(),
                peer: peer.clone(),
                name: name.clone(),
            },
        };
        let address = match self.binding.address() {
            SocketAddr::V4(address) => address,
            SocketAddr::V6(_) => return Err(Error::UnsupportedSource),
        };
        Ok(Checked {
            host: self.endpoint.host.clone(),
            address,
            binding,
        })
    }

    /// Retained intent available before starting any proxy or SSH helper. The
    /// native controller supplies verified scope after its explicit owner action.
    /// # Errors
    /// Refuses malformed scope or a source outside the supported IPv4 boundary.
    pub fn retained(
        &self,
        owner: crate::cloud_runtime::state::service_admission::OwnerScope,
        worker_id: String,
    ) -> crate::cloud_runtime::Result<crate::cloud_runtime::state::service_admission::Record> {
        crate::cloud_runtime::state::service_admission::Record::armed(
            owner,
            worker_id,
            self.attempt.operation_id,
            self.attempt.generation,
            self.checked()
                .map_err(|_| crate::cloud_runtime::Error::Invalid("Unsupported endpoint address"))?,
        )
    }

    /// Narrows the current LAN policy to exactly one resolved address and port.
    /// # Errors
    /// Refuses an ambiguous name, a local/outside device or an unavailable network.
    pub fn lan(endpoint: Endpoint) -> Result<Self, Error> {
        let source = Source::lan()?;
        let binding = source.observe(&endpoint)?;
        Ok(Self {
            attempt: Attempt::new(),
            endpoint,
            source,
            binding,
        })
    }

    /// Reads an existing Linux kernel-mode desktop Tailscale client. Never enrolls it.
    /// # Errors
    /// Refuses unsupported client modes, unknown/ambiguous peers or failed status checks.
    pub fn tailnet(endpoint: Endpoint) -> Result<Self, Error> {
        let source = Source::tailnet()?;
        let binding = source.observe(&endpoint)?;
        Ok(Self {
            attempt: Attempt::new(),
            endpoint,
            source,
            binding,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Lan,
    Tailnet,
}

/// A host must retain this identity before starting a gateway. It grants no authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Attempt {
    operation_id: Uuid,
    generation: Uuid,
}

impl Attempt {
    fn new() -> Self {
        Self {
            operation_id: Uuid::new_v4(),
            generation: Uuid::new_v4(),
        }
    }
}

/// Nonsecret identity of this exact local gateway, never just its reusable TCP port.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Receipt {
    operation_id: Uuid,
    generation: Uuid,
    proxy_port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    OwnerClosed,
    HostStopped,
    BindingChanged,
    SourceUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Active,
    Revoked { reason: Reason },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub receipt: Receipt,
    pub host: String,
    pub address: SocketAddr,
    pub source: SourceKind,
    pub state: State,
    pub connections: usize,
    pub bytes: u64,
    pub refused: u64,
}

struct Gate {
    prepared: Prepared,
    reason: Mutex<Option<Reason>>,
}

impl Gate {
    fn revoke(&self, reason: Reason) {
        self.reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
    }

    fn reason(&self) -> Option<Reason> {
        *self.reason.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check(&self) -> Result<(), Error> {
        if self.reason().is_some() {
            return Err(Error::BindingChanged);
        }
        let observed = self.prepared.source.observe(&self.prepared.endpoint);
        let result = observed.and_then(|binding| {
            (binding == self.prepared.binding)
                .then_some(())
                .ok_or(Error::BindingChanged)
        });
        if let Err(ref error) = result {
            match error {
                Error::Busy => {}
                Error::BindingChanged => self.revoke(Reason::BindingChanged),
                _ => self.revoke(Reason::SourceUnavailable),
            }
        }
        result
    }
}

impl socks::Gate for Gate {
    fn admit(
        &self,
        destination: &Destination,
    ) -> Result<Vec<SocketAddr>, horizon_cloud_protocol::local_network::Reply> {
        use horizon_cloud_protocol::local_network::Reply;
        let address = self.prepared.binding.address();
        if !self.prepared.endpoint.matches(destination, address) || self.reason().is_some() {
            return Err(Reply::NotAllowed);
        }
        self.check().map_err(|error| match error {
            Error::Busy => Reply::GeneralFailure,
            _ => Reply::NotAllowed,
        })?;
        if !self.keeps(address) {
            return Err(Reply::NotAllowed);
        }
        Ok(vec![address])
    }

    fn keeps(&self, address: SocketAddr) -> bool {
        self.reason().is_none() && address == self.prepared.binding.address()
    }
}

/// Host-only primitive. The future scoped controller owns membership, approval and
/// durable intent; this type does not infer any of them from a state path.
pub struct Gateway {
    receipt: Receipt,
    gate: Arc<Gate>,
    proxy: Arc<socks::Proxy>,
    stopped: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
}

impl Gateway {
    /// # Errors
    /// Revalidates preflight before binding and fails without a usable relay on any error.
    pub fn start(prepared: Prepared) -> Result<Self, Error> {
        Self::start_with(prepared, socks::Proxy::start)
    }

    /// Creates an exact private Unix ingress for an original pinned SSH attachment.
    /// This primitive grants no owner authority.
    /// # Errors
    /// Refuses source drift, an unsupported private parent or listener startup failure.
    #[cfg(unix)]
    pub fn start_private(prepared: Prepared) -> Result<Self, Error> {
        Self::start_with(prepared, socks::Proxy::start_private)
    }

    /// # Errors
    /// Private SSH ingress is unsupported on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn start_private(_prepared: Prepared) -> Result<Self, Error> {
        Err(Error::UnsupportedSource)
    }

    fn start_with(
        prepared: Prepared,
        start: fn(Arc<dyn socks::Gate>) -> std::io::Result<socks::Proxy>,
    ) -> Result<Self, Error> {
        let attempt = prepared.attempt;
        let gate = Arc::new(Gate {
            prepared,
            reason: Mutex::new(None),
        });
        gate.check()?;
        let proxy = Arc::new(start(Arc::clone(&gate) as Arc<dyn socks::Gate>).map_err(|_| Error::StartFailed)?);
        let receipt = Receipt {
            operation_id: attempt.operation_id,
            generation: attempt.generation,
            proxy_port: proxy.port(),
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(&proxy);
        let monitor_gate = Arc::clone(&gate);
        let monitor_stopped = Arc::clone(&stopped);
        let monitor = thread::Builder::new()
            .name("one-service-source".into())
            .spawn(move || monitor(&weak, &monitor_gate, &monitor_stopped))
            .map_err(|_| Error::StartFailed)?;
        Ok(Self {
            receipt,
            gate,
            proxy,
            stopped,
            monitor: Some(monitor),
        })
    }

    /// A cached, nonsecret snapshot. It does not read the client, filesystem or keychain.
    #[must_use]
    pub fn status(&self) -> Status {
        let Counters {
            connections,
            bytes,
            refused,
        } = self.proxy.counters();
        Status {
            receipt: self.receipt.clone(),
            host: self.gate.prepared.endpoint.host.clone(),
            address: self.gate.prepared.binding.address(),
            source: self.gate.prepared.binding.kind(),
            state: self
                .gate
                .reason()
                .map_or(State::Active, |reason| State::Revoked { reason }),
            connections,
            bytes,
            refused,
        }
    }

    #[must_use]
    pub fn relays(&self) -> Vec<Relay> {
        self.proxy.relays()
    }

    /// # Errors
    /// A stale receipt changes nothing, even if a local port has been reused.
    pub fn revoke(&self, receipt: &Receipt) -> Result<(), Error> {
        if *receipt != self.receipt {
            return Err(Error::StaleReceipt);
        }
        self.gate.revoke(Reason::OwnerClosed);
        self.proxy.close();
        self.gate.prepared.source.cancel();
        Ok(())
    }
}

fn monitor(proxy: &Weak<socks::Proxy>, gate: &Gate, stopped: &AtomicBool) {
    let mut next = Instant::now() + Duration::from_secs(1);
    while !stopped.load(Ordering::Acquire) {
        if Instant::now() >= next {
            let _ = gate.check();
            next = Instant::now() + Duration::from_secs(1);
        }
        if gate.reason().is_some() {
            if let Some(proxy) = proxy.upgrade() {
                proxy.close();
            }
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.gate.prepared.source.cancel();
        self.gate.revoke(Reason::HostStopped);
        self.proxy.close();
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}
