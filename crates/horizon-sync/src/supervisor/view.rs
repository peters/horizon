//! The machine list and the key bindings that this computer shows: the mirror of
//! the server with the intents that wait for it applied.
//!
//! The answer of a load is remembered in the same call, so a save turns only what
//! its caller changed into an intent and never what the server answered between
//! the two.

use std::collections::BTreeMap;

use horizon_core::Machine;

use super::SyncState;
use crate::intent::Op;

pub(super) fn records_of(state: &mut SyncState) -> Vec<Machine> {
    let merged = merged_records(state);
    state.view = merged
        .iter()
        .map(|machine| (machine.name.clone(), machine.clone()))
        .collect();
    merged
}

/// The records of the state with its waiting intents applied, in name order.
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
            | Op::ObjectPut { .. }
            | Op::ObjectRemove { .. } => {}
        }
    }
    merged.into_values().collect()
}
