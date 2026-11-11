//! The named secret values that this process gives to the agent panels it starts.
//!
//! A panel reads its environment once, when its program starts, so the values of
//! the named secrets must be in this process before a panel starts. A worker
//! thread reads them from the secrets file and publishes them here, and
//! [`apply`] copies them into the environment of one agent panel. Nothing else
//! reads this map, so no frame of the UI and no command line ever holds a value.
//!
//! The map is a process-global place on purpose: the environment of a panel is
//! inherited from this process, and the code that starts a panel is reached from
//! the board, from a preset, from a restored session and from an MCP operation
//! alike. One published snapshot is what all of those paths read.

use std::collections::{BTreeMap, HashMap};
use std::sync::{PoisonError, RwLock};

use zeroize::Zeroizing;

use super::SecretName;

static PUBLISHED: RwLock<BTreeMap<SecretName, Zeroizing<String>>> = RwLock::new(BTreeMap::new());

/// Replaces the values that the next agent panel inherits. It runs on the worker
/// thread that read the store, never on the UI thread.
pub fn publish(values: BTreeMap<SecretName, Zeroizing<String>>) {
    *PUBLISHED.write().unwrap_or_else(PoisonError::into_inner) = values;
}

/// Drops every published value, for a test that starts from no secret.
#[cfg(test)]
pub(crate) fn clear() {
    publish(BTreeMap::new());
}

/// One test at a time reads and writes the published values: the map belongs to
/// the process, and two tests that publish at the same time would read each
/// other's secret.
#[cfg(test)]
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    clear();
    guard
}

/// Publishes one value, as the store answers it after a write.
pub(crate) fn set(name: SecretName, value: Zeroizing<String>) {
    PUBLISHED
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name, value);
}

/// Drops one value, because its secret was deleted or its store lost it.
pub(crate) fn remove(name: &SecretName) {
    PUBLISHED.write().unwrap_or_else(PoisonError::into_inner).remove(name);
}

/// The names that this process holds a value for now.
#[must_use]
pub fn published_names() -> Vec<SecretName> {
    PUBLISHED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
        .cloned()
        .collect()
}

/// Copies every published value into the environment of one agent panel.
pub fn apply<S: std::hash::BuildHasher>(environment: &mut HashMap<String, String, S>) {
    for (name, value) in PUBLISHED.read().unwrap_or_else(PoisonError::into_inner).iter() {
        environment.insert(name.as_str().to_owned(), value.to_string());
    }
}
