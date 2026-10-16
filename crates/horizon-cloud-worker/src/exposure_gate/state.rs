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
    sources: BTreeMap<IpAddr, Source>,
    flows: BTreeMap<u64, Flow>,
}

#[derive(Default)]
struct Source {
    active: u32,
    charged: u64,
}
struct Flow {
    source: IpAddr,
    client: Arc<TcpStream>,
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
            flows: BTreeMap::new(),
        }
    }

    pub fn admit(&mut self, client: Arc<TcpStream>) -> Option<u64> {
        if super::bounded_socket(&SockRef::from(client.as_ref())).is_err() {
            self.refused = self.refused.saturating_add(1);
            return None;
        }
        let source = canonical_ip(client.peer_addr().ok()?.ip());
        let previous = self.sources.get(&source);
        let limits = self.policy.limits();
        if self.stopped
            || Instant::now() >= self.until
            || !self.policy.allows(source)
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
        self.sources.entry(source).or_default().active += 1;
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
                shutdown(&flow.client);
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
        shutdown(&self.client);
        if let Some(backend) = &self.backend {
            shutdown(backend);
        }
    }
}
