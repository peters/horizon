//! What one answer of the server changes on this computer: the records, the key
//! bindings and the secrets, and the seed of the values that this computer held
//! before its first link.
//!
//! A secret value is written outside the lock of the state, because a file and a
//! credential store may take a while and no frame of the UI waits for them.

use std::collections::BTreeMap;

use horizon_core::Machine;
use horizon_core::secret::SecretId;

use super::SyncHandle;
use super::files::{MirrorFile, MirroredMachine, write_mirror};
use super::secrets::{local_machine_keys, refuse_secrets, reseed_secrets, secret_rows_of, write_secrets};
use super::view::merged_records;
use super::{MIRROR_FORMAT, SyncState};
use crate::client::PullOutcome;
use crate::error::SyncError;
use crate::intent::{KIND_BINDING, KIND_MACHINE, Op, PushReply};
use crate::protocol::HelloOk;
use crate::store::Changes;

pub(super) fn accept_pushed(handle: &SyncHandle, reply: &PushReply) {
    // The queue loses the refused intent here, so the ids of the secrets that the
    // server refused and the version that a refusal names are read before the
    // queue is settled.
    let (refused_secrets, rewind): (Vec<String>, Option<u64>) = {
        let state = handle.lock();
        let mut ids = Vec::new();
        let mut oldest: Option<u64> = None;
        for intent in state.client.pending() {
            if !intent.op.is_secret() {
                continue;
            }
            let Some(crate::intent::Outcome::Refused(refusal)) = reply
                .results
                .iter()
                .find(|result| result.id == intent.id)
                .map(|result| &result.outcome)
            else {
                continue;
            };
            ids.push(intent.op.object_id().to_owned());
            if let Some(version) = refusal.version() {
                oldest = Some(oldest.map_or(version, |old: u64| old.min(version)));
            }
        }
        (ids, oldest)
    };
    let outcome = {
        let mut state = handle.lock();
        state.client.accept_push(reply)
    };
    match outcome {
        Ok(outcome) => {
            for (id, refusal) in &outcome.refused {
                tracing::warn!(
                    id,
                    code = refusal.code(),
                    reason = %refusal,
                    "the server refused an intent"
                );
            }
        }
        Err(error) => {
            tracing::warn!(%error, "the answer of a push was not kept");
            return;
        }
    }
    refuse_secrets(handle, &refused_secrets, rewind);
    handle.refresh_machine_keys();
    let mut state = handle.lock();
    state.generation = state.generation.wrapping_add(1);
    let file = mirror_of(&state);
    if let Err(error) = write_mirror(&state.mirror_file, &file) {
        tracing::warn!(%error, "the mirror of the machine list was not written");
    }
    drop(state);
    (handle.wake)();
}

/// Applies one answer of changes to the mirror and moves the mark of this
/// computer. `asked_from` is the index that the request of this answer read from.
pub(super) fn apply_changes(handle: &SyncHandle, changes: &Changes, asked_from: u64) -> Result<PullOutcome, SyncError> {
    let (applied, secret_rows) = {
        let mut state = handle.lock();
        let applied = apply_rows(&mut state, changes)?;
        let bindings = apply_bindings(&mut state, changes);
        let rows = secret_rows_of(&mut state, changes);
        bindings?;
        (applied, rows)
    };
    // A secret is written outside the lock of the state: a file, and above all a
    // credential store, may take a while, and no frame of the UI waits for it.
    write_secrets(handle, &secret_rows);
    let mut state = handle.lock();
    seed_local(&mut state)?;
    state.client.accept_replay(asked_from);
    state.client.accept_mark(changes.index)?;
    state.machine_keys = local_machine_keys(&state.secrets);
    state.generation = state.generation.wrapping_add(1);
    let file = mirror_of(&state);
    write_mirror(&state.mirror_file, &file)?;
    drop(state);
    tracing::debug!(
        index = applied.index,
        rows = applied.rows,
        reset = applied.reset,
        "the machine list moved"
    );
    Ok(applied)
}

/// Applies the machine rows of one answer to the mirror of the state.
pub(super) fn apply_rows(state: &mut SyncState, changes: &Changes) -> Result<PullOutcome, SyncError> {
    if changes.reset {
        state.mirror.clear();
    }
    let mut applied = 0;
    for row in &changes.rows {
        if row.kind != KIND_MACHINE {
            continue;
        }
        let name = row.id.clone();
        if row.deleted {
            state.mirror.remove(&name);
            applied += 1;
            continue;
        }
        let record: Machine = serde_json::from_value(row.body.clone()).map_err(|error| {
            SyncError::Queue(format!(
                "the server sent a machine record that this Horizon cannot read: {error}"
            ))
        })?;
        let record = horizon_core::machines::check_record(&record).map_err(|error| {
            SyncError::Queue(format!(
                "the server sent a machine record that breaks its rule: {error}"
            ))
        })?;
        let mine = state.owned.contains(&name);
        state.mirror.insert(name, MirroredMachine { record, mine });
        applied += 1;
    }
    Ok(PullOutcome {
        index: changes.index,
        reset: changes.reset,
        rows: applied,
    })
}

/// Applies the key bindings of one answer. A binding is last write wins, as the
/// machine list is.
pub(super) fn apply_bindings(state: &mut SyncState, changes: &Changes) -> Result<(), SyncError> {
    for row in &changes.rows {
        if row.kind != KIND_BINDING {
            continue;
        }
        if row.deleted {
            state.bindings.remove(&row.id);
            continue;
        }
        let key = row.body.get("key").and_then(serde_json::Value::as_str).ok_or_else(|| {
            SyncError::Queue(format!(
                "the server sent a key binding of {} that this Horizon cannot read",
                row.id
            ))
        })?;
        state.bindings.insert(row.id.clone(), key.to_owned());
    }
    Ok(())
}

/// Keeps the records, the key bindings and the secrets that this computer had
/// before the server, once the first pull says which of them the server does not
/// have.
pub(super) fn seed_local(state: &mut SyncState) -> Result<(), SyncError> {
    if let Some(records) = state.pending_seed.take() {
        for record in records {
            if state.mirror.contains_key(&record.name) {
                // A name that both have keeps the record of the server.
                continue;
            }
            let name = record.name.clone();
            state.client.enqueue(None, Op::MachinePut { record })?;
            state.owned.insert(name);
        }
        // A secret is compared once: a key or a value that this computer makes
        // after its first link enters the queue where its writer makes it.
        for id in super::secrets::local_ids(state) {
            let Ok(id) = SecretId::parse(&id) else {
                continue;
            };
            if state.client.secret_version(id.as_str()).is_some() {
                // A secret that the server has keeps the value of the server.
                continue;
            }
            state.client.enqueue_secret(id.as_str())?;
        }
    }
    for (machine, key) in std::mem::take(&mut state.local_bindings) {
        if state.bindings.contains_key(&machine) {
            // A machine that both bind keeps the binding of the server.
            continue;
        }
        state.client.enqueue_binding(&machine, Some(&key))?;
    }
    state.seeded = true;
    Ok(())
}

/// One secret of an answer of changes, with the decision that this computer made
/// about its own value. It is read from the answer, so the body of the pull is
pub(super) fn merged_bindings(state: &SyncState) -> BTreeMap<String, String> {
    let mut merged = state.bindings.clone();
    for intent in state.client.pending() {
        if let Op::BindingPut { machine, key } = &intent.op {
            match key {
                Some(key) => {
                    merged.insert(machine.clone(), key.clone());
                }
                None => {
                    merged.remove(machine);
                }
            }
        }
    }
    merged
}

/// The mirror file of the state.
pub(super) fn mirror_of(state: &SyncState) -> MirrorFile {
    MirrorFile {
        format: MIRROR_FORMAT,
        seeded: state.seeded,
        owned: state.owned.clone(),
        machines: merged_records(state),
        bindings: merged_bindings(state),
    }
}

/// Remembers the hello of the server.
///
/// A server that cannot read the secrets that it holds says so here, and every
/// computer that still holds a value sends it again: the value of this computer is
/// new to that server, so it goes as the first write of a secret.
pub(super) fn accept_hello(handle: &SyncHandle, answer: &HelloOk) -> Result<(), String> {
    let mut state = handle.lock();
    state
        .client
        .accept_hello(answer)
        .map_err(|error| format!("the hello was not kept: {error}"))?;
    if answer.secrets_lost {
        tracing::warn!("the server cannot read the secrets that it holds, so this computer sends its own again");
        reseed_secrets(&mut state).map_err(|error| format!("the secrets that this computer sends again: {error}"))?;
    }
    state.generation = state.generation.wrapping_add(1);
    drop(state);
    (handle.wake)();
    Ok(())
}

/// The time of this computer, for the name of a file that lost the version rule.
/// A time never reaches the server: an intent carries none.
pub(super) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}
