use super::{Budgets, Counters, MAX_SOURCES};
use horizon_cloud_protocol::exposure::{ExposurePolicy, canonical_ip};
use socket2::SockRef;
use std::{
    collections::BTreeMap,
    net::{IpAddr, Shutdown},
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    time::{Duration, Instant},
};

pub(super) type Shared = Arc<Mutex<State>>;
pub(super) fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Running,
    Budget(Instant),
    Cancelled,
}

pub(super) struct State {
    pub stopped: bool,
    pub listeners: Vec<Arc<TcpListener>>,
    pub policy: ExposurePolicy,
    pub budgets: Budgets,
    pub until: Instant,
    next: u64,
    charged: u64,
    forwarded: u64,
    refused: u64,
    sources: BTreeMap<SourceKey, Source>,
    #[cfg(all(test, feature = "trusted-exposure-gate"))]
    pub verified_peers: u64,
    #[cfg(all(test, feature = "trusted-exposure-gate"))]
    pub verified_peer_id: Option<Arc<str>>,
    #[cfg(all(test, feature = "trusted-exposure-gate"))]
    pub policy_denials: u64,
    #[cfg(all(test, feature = "trusted-exposure-gate"))]
    pub normal_completed: u64,
    flows: BTreeMap<u64, Flow>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SourceKey {
    Ip(IpAddr),
    #[cfg(feature = "trusted-exposure-gate")]
    StableId(Arc<str>),
    #[cfg(feature = "trusted-exposure-gate")]
    Pending,
}

enum Client {
    Direct(Arc<TcpStream>),
    #[cfg(feature = "trusted-exposure-gate")]
    Identifying(crate::trusted_tcp_producer::PendingCancellation),
    #[cfg(feature = "trusted-exposure-gate")]
    Trusted(Arc<crate::trusted_tcp_producer::AcceptedStream>),
}
impl Client {
    fn shutdown(&self) {
        match self {
            Self::Direct(stream) => shutdown(stream),
            #[cfg(feature = "trusted-exposure-gate")]
            Self::Trusted(stream) => {
                stream.cancel();
            }
            #[cfg(feature = "trusted-exposure-gate")]
            Self::Identifying(stream) => {
                stream.cancel();
            }
        }
    }
}

#[derive(Default)]
struct Source {
    active: u32,
    charged: u64,
}
struct Flow {
    source: SourceKey,
    client: Client,
    backend: Option<Arc<TcpStream>>,
    until: Instant,
    progress: Instant,
    charged: u64,
    pending: u64,
    phase: watch::Sender<Phase>,
    graceful: bool,
}

impl State {
    pub fn new(listeners: Vec<Arc<TcpListener>>, policy: ExposurePolicy, budgets: Budgets, until: Instant) -> Self {
        Self {
            stopped: false,
            listeners,
            policy,
            budgets,
            until,
            next: 0,
            charged: 0,
            forwarded: 0,
            refused: 0,
            sources: BTreeMap::new(),
            #[cfg(all(test, feature = "trusted-exposure-gate"))]
            verified_peers: 0,
            #[cfg(all(test, feature = "trusted-exposure-gate"))]
            verified_peer_id: None,
            #[cfg(all(test, feature = "trusted-exposure-gate"))]
            policy_denials: 0,
            #[cfg(all(test, feature = "trusted-exposure-gate"))]
            normal_completed: 0,
            flows: BTreeMap::new(),
        }
    }

    pub fn admit(&mut self, client: Arc<TcpStream>) -> Option<u64> {
        if super::bounded_socket(&SockRef::from(client.as_ref())).is_err() {
            self.refused = self.refused.saturating_add(1);
            return None;
        }
        let source = canonical_ip(client.peer_addr().ok()?.ip());
        if !self.policy.allows(source) {
            self.refused = self.refused.saturating_add(1);
            return None;
        }
        self.register(Client::Direct(client), SourceKey::Ip(source))
    }

    fn register(&mut self, client: Client, source: SourceKey) -> Option<u64> {
        let previous = self.sources.get(&source);
        let limits = self.policy.limits();
        if self.stopped
            || Instant::now() >= self.until
            || self.flows.len() >= limits.total as usize
            || previous
                .is_some_and(|entry| entry.active >= limits.per_source || entry.charged >= self.budgets.source_bytes)
            || (previous.is_none() && self.sources.len() >= MAX_SOURCES)
            || self.charged >= self.budgets.gate_bytes
        {
            self.refused = self.refused.saturating_add(1);
            return None;
        }
        let id = self.next.checked_add(1)?;
        let now = Instant::now();
        let until = now.checked_add(self.budgets.connection_lifetime)?.min(self.until);
        self.next = id;
        self.sources.entry(source.clone()).or_default().active += 1;
        self.flows.insert(
            id,
            Flow {
                source,
                client,
                backend: None,
                until,
                progress: now,
                charged: 0,
                pending: 0,
                phase: watch::channel(Phase::Running).0,
                graceful: false,
            },
        );
        Some(id)
    }

    #[cfg(feature = "trusted-exposure-gate")]
    pub fn reserve_pending(&mut self, client: &crate::trusted_tcp_producer::PendingStream) -> Option<u64> {
        if client
            .bound_buffers(super::SOCKET_BUFFER_BYTES, super::MAX_SOCKET_BUFFER_BYTES)
            .is_err()
        {
            self.refuse();
            return None;
        }
        self.register(Client::Identifying(client.cancellation()), SourceKey::Pending)
    }

    #[cfg(feature = "trusted-exposure-gate")]
    pub fn identify(
        &mut self,
        id: u64,
        peer: crate::trusted_tcp_producer::AcceptedPeer,
    ) -> Option<Arc<crate::trusted_tcp_producer::AcceptedStream>> {
        let allowed = self
            .policy
            .matches_tailnet(peer.original_source().ip(), peer.full_dns_name(), peer.tags());
        let source = SourceKey::StableId(Arc::from(peer.stable_id()));
        let stream = peer.into_stream();
        let same = self.flows.get(&id).is_some_and(|flow| match &flow.client {
            Client::Identifying(original) => super::trusted::same_accept(original, &stream),
            Client::Direct(_) | Client::Trusted(_) => false,
        });
        #[cfg(test)]
        if same {
            self.verified_peers = self.verified_peers.saturating_add(1);
            self.verified_peer_id = match &source {
                SourceKey::StableId(id) => Some(Arc::clone(id)),
                SourceKey::Ip(_) | SourceKey::Pending => None,
            };
        }
        let previous = self.sources.get(&source);
        let eligible = same
            && self.deadline(id).is_some()
            && self.phase(id) == Phase::Running
            && self.charged < self.budgets.gate_bytes
            && previous.is_none_or(|entry| {
                entry.active < self.policy.limits().per_source && entry.charged < self.budgets.source_bytes
            })
            && (previous.is_some() || self.sources.len() < MAX_SOURCES);
        #[cfg(test)]
        if eligible && !allowed {
            self.policy_denials = self.policy_denials.saturating_add(1);
        }
        if !allowed || !eligible {
            self.refuse();
            stream.cancel();
            return None;
        }
        let flow = self.flows.get_mut(&id)?;
        let previous = self.sources.get_mut(&flow.source)?;
        previous.active = previous.active.saturating_sub(1);
        self.sources.entry(source.clone()).or_default().active += 1;
        let stream = Arc::new(stream);
        flow.client = Client::Trusted(Arc::clone(&stream));
        flow.source = source;
        Some(stream)
    }

    #[cfg(feature = "trusted-exposure-gate")]
    pub fn refuse(&mut self) {
        self.refused = self.refused.saturating_add(1);
    }

    pub fn deadline(&self, id: u64) -> Option<Instant> {
        let flow = self.flows.get(&id)?;
        if self.stopped || *flow.phase.borrow() == Phase::Cancelled {
            return None;
        }
        let until = flow
            .until
            .min(self.until)
            .min(flow.progress.checked_add(self.budgets.idle_timeout)?);
        (Instant::now() < until).then_some(until)
    }

    pub fn phase(&self, id: u64) -> Phase {
        self.flows
            .get(&id)
            .map_or(Phase::Cancelled, |flow| *flow.phase.borrow())
    }

    pub fn subscribe(&self, id: u64) -> Option<watch::Receiver<Phase>> {
        Some(self.flows.get(&id)?.phase.subscribe())
    }

    pub fn complete(&mut self, id: u64) {
        let grace_expired = matches!(self.phase(id), Phase::Budget(until) if Instant::now() >= until);
        if self.deadline(id).is_some() && !grace_expired {
            if let Some(flow) = self.flows.get_mut(&id) {
                #[cfg(all(test, feature = "trusted-exposure-gate"))]
                if flow.pending == 0 && !flow.graceful && *flow.phase.borrow() == Phase::Running {
                    self.normal_completed = self.normal_completed.saturating_add(1);
                }
                flow.graceful = flow.pending == 0;
            }
        } else {
            self.cancel_flow(id);
        }
    }

    pub fn attach(&mut self, id: u64, backend: Arc<TcpStream>) -> bool {
        if self.deadline(id).is_none() || self.phase(id) != Phase::Running {
            shutdown(&backend);
            return false;
        }
        let Some(flow) = self.flows.get_mut(&id) else {
            shutdown(&backend);
            return false;
        };
        #[cfg(feature = "trusted-exposure-gate")]
        if matches!(flow.client, Client::Identifying(_)) {
            shutdown(&backend);
            return false;
        }
        flow.backend = Some(backend);
        true
    }

    /// Charge before writing. Failed writes retain their conservative byte charge.
    pub fn reserve(&mut self, id: u64, count: usize) -> usize {
        if self.deadline(id).is_none() {
            self.cancel_flow(id);
            return 0;
        }
        let Some(flow) = self.flows.get_mut(&id) else {
            return 0;
        };
        #[cfg(feature = "trusted-exposure-gate")]
        if matches!(flow.client, Client::Identifying(_)) {
            return 0;
        }
        let Some(source) = self.sources.get_mut(&flow.source) else {
            return 0;
        };
        let allowed = (count as u64)
            .min(self.budgets.gate_bytes.saturating_sub(self.charged))
            .min(self.budgets.source_bytes.saturating_sub(source.charged))
            .min(self.budgets.connection_bytes.saturating_sub(flow.charged));
        self.charged += allowed;
        source.charged += allowed;
        flow.charged += allowed;
        flow.pending += allowed;
        self.close_exhausted();
        usize::try_from(allowed).unwrap_or(0)
    }

    pub fn wrote(&mut self, id: u64, bytes: usize) {
        self.forwarded = self.forwarded.saturating_add(bytes as u64);
        let Some(flow) = self.flows.get_mut(&id) else {
            return;
        };
        if bytes > 0 {
            flow.progress = Instant::now();
        }
        flow.pending = flow.pending.saturating_sub(bytes as u64);
        self.close_exhausted();
    }

    fn close_exhausted(&mut self) {
        let all = self.charged >= self.budgets.gate_bytes;
        for flow in self.flows.values_mut() {
            let same = self
                .sources
                .get(&flow.source)
                .is_some_and(|source| source.charged >= self.budgets.source_bytes);
            // The coordinator may half-close only after both directions have
            // queued their reservations. Kernel acceptance is not peer receipt.
            if flow.pending == 0
                && *flow.phase.borrow() == Phase::Running
                && (all || same || flow.charged >= self.budgets.connection_bytes)
            {
                let until = flow
                    .until
                    .min(self.until)
                    .min(
                        flow.progress
                            .checked_add(self.budgets.idle_timeout)
                            .unwrap_or(self.until),
                    )
                    .min(Instant::now().checked_add(Duration::from_secs(3)).unwrap_or(self.until));
                flow.phase.send_replace(Phase::Budget(until));
            }
        }
    }

    pub fn cancel_flow(&mut self, id: u64) {
        if let Some(flow) = self.flows.get_mut(&id) {
            flow.close();
        }
    }

    pub fn release(&mut self, id: u64) {
        if let Some(flow) = self.flows.remove(&id) {
            if !flow.graceful || *flow.phase.borrow() == Phase::Cancelled {
                flow.client.shutdown();
                if let Some(backend) = &flow.backend {
                    shutdown(backend);
                }
            }
            if let Some(source) = self.sources.get_mut(&flow.source) {
                source.active = source.active.saturating_sub(1);
            }
        }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        for listener in &self.listeners {
            let _ = SockRef::from(listener.as_ref()).shutdown(Shutdown::Both);
        }
        for flow in self.flows.values_mut() {
            flow.close();
        }
    }

    pub fn counters(&self) -> Counters {
        Counters {
            active_connections: self.flows.len(),
            recorded_sources: self.sources.len(),
            refused: self.refused,
            forwarded_bytes: self.forwarded,
            charged_bytes: self.charged,
        }
    }
}

fn shutdown(stream: &TcpStream) {
    let _ = SockRef::from(stream).shutdown(Shutdown::Both);
}
impl Flow {
    fn close(&mut self) {
        self.phase.send_replace(Phase::Cancelled);
        self.client.shutdown();
        if let Some(backend) = &self.backend {
            shutdown(backend);
        }
    }
}

#[cfg(all(test, feature = "trusted-exposure-gate"))]
#[path = "tests/unix_relay.rs"]
mod unix_tests;
