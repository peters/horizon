//! The machine list and the key bindings that this computer shows: the mirror of
//! the server with the intents that wait for it applied, and, while the server did
//! not answer yet, the records that this computer kept before its first link.
//!
//! The answer of a load is remembered in the same call, so a save turns only what
//! its caller changed into an intent and never what the server answered between
//! the two.

use std::collections::BTreeMap;

use horizon_core::Machine;

use super::SyncState;
use crate::intent::Op;

pub(super) fn records_of(state: &mut SyncState) -> Vec<Machine> {
    let merged = view_records(state);
    state.view = merged
        .iter()
        .map(|machine| (machine.name.clone(), machine.clone()))
        .collect();
    merged
}

/// The records that the person sees, in name order: the mirror of the server with
/// its intents, and the seed of this computer until the first answer of the
/// server decides what it already holds.
///
/// The seed is the machine list that this computer kept before it named a server.
/// A link that never answers must not take that list away: the machine of the
/// person stays in the sidebar and in `machines`, and the first pull sends a
/// record that the server does not have. A name that the mirror already holds
/// keeps the record of the server, as [`super::pull::seed_local`] decides.
pub(super) fn view_records(state: &SyncState) -> Vec<Machine> {
    let mut records = merged_records(state);
    for record in state.pending_seed.iter().flatten() {
        if !records.iter().any(|held| held.name == record.name) {
            records.push(record.clone());
        }
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));
    records
}

/// The records of the server with its waiting intents applied, in name order.
///
/// This is what the mirror file holds. The seed of a first link is not one of
/// them, so the first pull compares it with the server instead of reading it as a
/// record that the server gave.
pub(super) fn merged_records(state: &SyncState) -> Vec<Machine> {
    let mut merged: BTreeMap<String, Machine> = state
        .mirror
        .iter()
        .map(|(name, machine)| (name.clone(), machine.record.clone()))
        .collect();
    for intent in state.client.pending() {
        match &intent.op {
            Op::MachinePut { record } => {
                merged.insert(record.name.clone(), record.clone());
            }
            Op::MachineRemove { name } => {
                merged.remove(name);
            }
            Op::BindingPut { .. }
            | Op::SecretPut { .. }
            | Op::SecretRemove { .. }
            | Op::SettingPut { .. }
            | Op::ObjectPut { .. }
            | Op::ObjectRemove { .. } => {}
        }
    }
    merged.into_values().collect()
}
