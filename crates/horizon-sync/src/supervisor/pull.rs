//! What one answer of the server changes on this computer: the records, the key
//! bindings and the secrets, and the seed of the values that this computer held
//! before its first link.
//!
//! A secret value is written outside the lock of the state, because a file and a
//! credential store may take a while and no frame of the UI waits for them.

use std::collections::{BTreeMap, BTreeSet};

use horizon_core::Machine;
use horizon_core::secret::SecretId;

use super::SyncHandle;
use super::files::{MirrorFile, MirroredMachine, MirroredSetting};
use super::secrets::{local_machine_keys, refuse_secrets, reseed_secrets, secret_rows_of, write_secrets};
use super::settings::{RefusedSection, apply as apply_settings, held_by_store, refuse_settings};
use super::view::merged_records;
use super::{MIRROR_FORMAT, SyncState, reset};
use crate::client::PullOutcome;
use crate::error::SyncError;
use crate::intent::{KIND_BINDING, KIND_MACHINE, KIND_SETTING, Op, Outcome, PushReply};
use crate::protocol::HelloOk;
use crate::store::Changes;

pub(super) fn accept_pushed(handle: &SyncHandle, reply: &PushReply) {
    // The queue loses the refused intent here, so the ids of the secrets that the
    // server refused, the version that a refusal names and the version that a
    // section of this push wrote are read before the queue is settled.
    let mut written: Vec<(String, u64, serde_json::Value)> = Vec::new();
    let (refused_secrets, refused_settings, rewind): (Vec<String>, Vec<RefusedSection>, Option<u64>) = {
        let state = handle.lock();
        let mut ids = Vec::new();
        let mut sections = Vec::new();
        let mut oldest: Option<u64> = None;
        for intent in state.client.pending() {
            if !intent.op.is_secret() && intent.op.kind() != crate::intent::KIND_SETTING {
                continue;
            }
            let found = reply
                .results
                .iter()
                .find(|result| result.id == intent.id)
                .map(|result| &result.outcome);
            // The row of a section that this push wrote is here before the request
            // behind the push brings it, so a save in that window names its version.
            if let (Op::SettingPut { id, body }, Some(Outcome::Applied { index })) = (&intent.op, found) {
                written.push((id.clone(), *index, body.clone()));
            }
            let Some(Outcome::Refused(refusal)) = found else {
                continue;
            };
            if intent.op.kind() == crate::intent::KIND_SETTING {
                sections.push((intent.op.object_id().to_owned(), refusal.version()));
            } else {
                ids.push(intent.op.object_id().to_owned());
            }
            if let Some(version) = refusal.version() {
                oldest = Some(oldest.map_or(version, |old: u64| old.min(version)));
            }
        }
        (ids, sections, oldest)
    };
    // The store is read outside the lock: a file is a wait that no frame may share.
    let (path, generation) = {
        let state = handle.lock();
        (state.settings_path.clone(), state.settings_generation)
    };
    let holds = path.map_or_else(BTreeSet::new, |path| held_by_store(&path, &written));
    let outcome = {
        let mut state = handle.lock();
        let outcome = state.client.accept_push(reply);
        // A save that was queued while the store was read is not in `holds`.
        let read_in_time = state.settings_generation == generation;
        for (section, version, body) in written {
            // A newer row stays: the server applied this intent again. The version of
            // this push is a version of the store only when the store holds the content
            // of the push; otherwise the row of that version comes again.
            let seen = state.settings.get(&section).map_or(0, |row| row.version);
            let applied = state.settings_applied.get(&section).copied().unwrap_or(0);
            if seen.max(applied) >= version {
                continue;
            }
            if read_in_time && holds.contains(&section) {
                state.settings_applied.insert(section.clone(), version);
            } else {
                state.client.replay_before(version);
            }
            state.settings.insert(section, MirroredSetting { version, body });
            state.settings_generation = state.settings_generation.wrapping_add(1);
            // The drop of the lock writes the mirror of the settings.
            state.settings_dirty = true;
        }
        outcome
    };
    match outcome {
        Ok(outcome) => {
            for (id, refusal) in &outcome.refused {
                log_refusal(*id, refusal);
            }
        }
        Err(error) => {
            tracing::warn!(%error, "the answer of a push was not kept");
            return;
        }
    }
    refuse_secrets(handle, &refused_secrets, rewind);
    refuse_settings(handle, &refused_settings, rewind);
    handle.refresh_machine_keys();
    let mut state = handle.lock();
    state.generation = state.generation.wrapping_add(1);
    // The drop of the lock writes the mirror and the queue.
    state.mirror_dirty = true;
    drop(state);
    (handle.wake)();
}

/// The line about one refusal of the server.
///
/// A refusal that a person must read about is a warning: the change of this
/// computer is gone, and nothing else says why. A write that names no version and
/// meets a row of the server is not one of them, whatever the content of the two
/// is: `already_answered` is the normal answer of a first link, and the row of the
/// server decides. A value that both sides hold the same way is no loss at all,
/// and a value that differs keeps its own line from the settings or the secrets,
/// so this one stays a debug line and a first link of eleven sections writes no
/// warning.
pub(super) fn log_refusal(intent: u64, refusal: &crate::error::Refusal) {
    if refusal_is_a_warning(refusal) {
        tracing::warn!(
            intent,
            code = refusal.code(),
            reason = %refusal,
            "the server refused an intent"
        );
    } else {
        tracing::debug!(
            intent,
            code = refusal.code(),
            version = refusal.version(),
            "the server already answered this intent, and its row decides"
        );
    }
}

/// Whether a person must read about one refusal of the server.
#[must_use]
fn refusal_is_a_warning(refusal: &crate::error::Refusal) -> bool {
    !matches!(refusal, crate::error::Refusal::AlreadyAnswered { .. })
}

/// Applies one answer of changes to the mirror and moves the mark of this
/// computer. `asked_from` is the index that the request of this answer read from.
pub(super) fn apply_changes(handle: &SyncHandle, changes: &Changes, asked_from: u64) -> Result<PullOutcome, SyncError> {
    // An answer that replaces the whole mirror takes the machines and the settings of
    // this computer with it, so this computer keeps them in one dated file of its home
    // before the first row of it applies.
    let kept = reset::keep_before_reset(handle, changes, asked_from);
    let (applied, secret_rows) = {
        let mut state = handle.lock();
        let applied = apply_rows(&mut state, changes)?;
        let bindings = apply_bindings(&mut state, changes);
        let rows = secret_rows_of(&mut state, changes);
        bindings?;
        apply_setting_rows(&mut state, changes);
        (applied, rows)
    };
    // A secret is written outside the lock of the state: a file, and above all a
    // credential store, may take a while, and no frame of the UI waits for it.
    write_secrets(handle, &secret_rows);
    let mut state = handle.lock();
    seed_local(&mut state)?;
    // A server whose log is only older is behind this computer for the records that it
    // does not hold, so those records go back to the queue instead of leaving the list
    // of this computer without a word.
    if let Some(kept) = &kept {
        reset::send_missing_records(&mut state, kept)?;
    }
    state.client.accept_replay(asked_from);
    state.client.accept_mark(changes.index)?;
    state.machine_keys = local_machine_keys(&state.secrets);
    state.generation = state.generation.wrapping_add(1);
    // The drop of the lock writes the mirror and the queue.
    state.mirror_dirty = true;
    drop(state);
    tracing::debug!(
        index = applied.index,
        rows = applied.rows,
        reset = applied.reset,
        "the machine list moved"
    );
    // The settings of this computer follow the same answer, on this worker thread:
    // a section of the server becomes the row of the settings store here.
    apply_settings(handle);
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

/// Applies the setting rows of one answer to the mirror of the settings.
///
/// A reset clears the mirror and what this computer applied of it, so the store of
/// this computer takes the sections of the restored server again. A section that
/// the restored server does not have stays in the store of this computer: no
/// setting is lost, and a section that this build does not know is kept.
pub(super) fn apply_setting_rows(state: &mut SyncState, changes: &Changes) {
    let mut touched = false;
    if changes.reset {
        state.settings.clear();
        state.settings_applied.clear();
        state.settings_refused.clear();
        touched = true;
    }
    for row in &changes.rows {
        if row.kind != KIND_SETTING {
            continue;
        }
        if row.deleted {
            state.settings.remove(&row.id);
            state.settings_applied.remove(&row.id);
        } else {
            state.settings.insert(
                row.id.clone(),
                MirroredSetting {
                    version: row.version,
                    body: row.body.clone(),
                },
            );
        }
        touched = true;
    }
    if touched {
        state.settings_generation = state.settings_generation.wrapping_add(1);
        // The drop of the lock writes the mirror of the settings.
        state.settings_dirty = true;
    }
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

/// The key bindings of the state with its waiting intents applied: the bindings
/// that the server gave, and the ones that this computer wrote and the server did
/// not answer yet.
///
/// This is what the mirror file holds, and the seed of a first link is not one of
/// them, so the first pull compares it with the server.
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

/// The key bindings that the person sees: the ones above, and, until the first
/// answer of the server decides what it already holds, the bindings that this
/// computer kept before its first link.
///
/// A key binding is the name of a key of this home, and a link that never answers
/// must not take it away: without this answer a machine of the list loses its key
/// at the moment that the person links the computer to a server.
pub(super) fn view_bindings(state: &SyncState) -> BTreeMap<String, String> {
    let mut merged = merged_bindings(state);
    for (machine, key) in &state.local_bindings {
        merged.entry(machine.clone()).or_insert_with(|| key.clone());
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
///
/// A server that answers with another store than the one this computer saw last
/// replaces the mirror of this computer at the next pull, which is a machine list
/// and a set of settings that the person did not change on this computer. The
/// machine list and the settings of this computer are kept in one dated file each
/// before that pull, and the person reads one line that names the two files.
pub(super) fn accept_hello(handle: &SyncHandle, answer: &HelloOk) -> Result<(), String> {
    let mut state = handle.lock();
    let seen = state.client.store().map(str::to_owned);
    // This attempt reached the server, so the reason of the attempt before it is
    // no longer the reason of the link.
    state.last_refusal = None;
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
    if seen.as_deref().is_some_and(|seen| seen != answer.store) {
        tracing::warn!(seen = ?seen, store = %answer.store, "the server answered with another store");
        // The answer that follows this hello holds the state of that store, and it takes
        // the records and the settings of this computer with it: both go to one dated
        // file of the home first.
        let line = reset::keep_before_store_change(handle);
        handle.push_notice(line);
    }
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
