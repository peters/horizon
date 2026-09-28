//! The connections a proxy is relaying: listed on the cloud card, and closed when the owner's
//! rules stop allowing them.
use super::super::Destination;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError, atomic::AtomicU64, atomic::Ordering},
    time::Instant,
};

/// One connection the proxy is relaying now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relay {
    /// What the worker asked for: an address, or a name and port resolved here.
    pub requested: Destination,
    /// The admitted address the proxy dialled.
    pub address: SocketAddr,
    /// Bytes relayed in both directions so far.
    pub bytes: u64,
    pub opened: Instant,
}

/// A relay's record while it runs; its byte count grows as the relay copies.
pub(super) struct Entry {
    requested: Destination,
    address: SocketAddr,
    pub(super) bytes: AtomicU64,
    opened: Instant,
    /// The proxy's registration ids of the relay's two sockets.
    sockets: Vec<u64>,
}

/// Open relays by id, removed when they end.
#[derive(Default)]
pub(super) struct Relays(Mutex<BTreeMap<u64, Arc<Entry>>>);

impl Relays {
    pub(super) fn insert(&self, id: u64, requested: Destination, address: SocketAddr, sockets: Vec<u64>) -> Arc<Entry> {
        let entry = Arc::new(Entry {
            requested,
            address,
            bytes: AtomicU64::new(0),
            opened: Instant::now(),
            sockets,
        });
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::clone(&entry));
        entry
    }

    pub(super) fn remove(&self, id: u64) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }

    /// The open relays, oldest first. Relays that start together may take their ids in either
    /// order, so the list is ordered by when each opened.
    pub(super) fn snapshot(&self) -> Vec<Relay> {
        let mut relays: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|entry| Relay {
                requested: entry.requested.clone(),
                address: entry.address,
                bytes: entry.bytes.load(Ordering::Acquire),
                opened: entry.opened,
            })
            .collect();
        relays.sort_by_key(|relay| relay.opened);
        relays
    }

    /// The socket ids of every relay whose dialled address `keep` no longer allows.
    pub(super) fn rejected(&self, keep: impl Fn(SocketAddr) -> bool) -> Vec<u64> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|entry| !keep(entry.address))
            .flat_map(|entry| entry.sockets.iter().copied())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).is_empty()
    }
}
