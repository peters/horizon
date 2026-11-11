//! The secrets of one computer on the link: the body that a push carries, the
//! value that a pull writes, and the copy that a value which lost the version rule
//! keeps.
//!
//! The queue of this computer holds the id of a secret and the version that it saw,
//! never the value, so the body comes into the frame here and nowhere else.

use std::{collections::BTreeSet, sync::Arc};

use horizon_core::SecretTarget;
use horizon_core::secret::{SecretError, SecretId, SecretKind};

use super::{SyncHandle, SyncState, now_millis};
use crate::error::SyncError;
use crate::intent::{Intent, KIND_SECRET, Op, SecretBody};
use crate::store::Changes;

pub(super) fn fill_secrets(handle: &SyncHandle, chunk: Vec<Intent>) -> Vec<Intent> {
    let secrets = {
        let state = handle.lock();
        Arc::clone(&state.secrets)
    };
    let mut filled = Vec::with_capacity(chunk.len());
    let mut dropped = Vec::new();
    for mut intent in chunk {
        let missing = match &intent.op {
            Op::SecretPut { id, body: None } => Some(id.clone()),
            _ => None,
        };
        if let Some(id) = missing {
            let read = SecretId::parse(&id).and_then(|parsed| secrets.read(&parsed));
            match read {
                Ok(Some(body)) => {
                    let Ok(text) = String::from_utf8(body.to_vec()) else {
                        dropped.push(intent.id);
                        tracing::warn!(%id, "a local secret is not text, so it is not sent");
                        continue;
                    };
                    intent.op = Op::SecretPut {
                        id,
                        body: Some(SecretBody::new(text)),
                    };
                }
                Ok(None) => {
                    dropped.push(intent.id);
                    tracing::warn!(
                        %id,
                        "the local value of a queued secret is gone, so it is not sent; the value of the server stays"
                    );
                    continue;
                }
                Err(error) => {
                    dropped.push(intent.id);
                    tracing::warn!(%id, code = error.code(), reason = %error, "a local secret was not read, so it is not sent");
                    continue;
                }
            }
        }
        filled.push(intent);
    }
    if !dropped.is_empty() {
        let mut state = handle.lock();
        if let Err(error) = state.client.forget_intents(&dropped) {
            tracing::warn!(%error, "an intent that could not be sent was not dropped");
        }
    }
    filled
}

/// One secret of an answer of changes.
pub(super) struct SecretRow {
    pub(super) id: SecretId,
    body: Option<Vec<u8>>,
    keep_local: bool,
}
/// Reads the secret rows of one answer, remembers which versions this computer
/// saw, and decides for each value whether a local value that differs is to be
/// kept: a value that this computer never published, or one whose own intent
/// still waits for the server, is kept beside the value of the server.
pub(super) fn secret_rows_of(state: &mut SyncState, changes: &Changes) -> Vec<SecretRow> {
    let mut rows = Vec::new();
    for row in &changes.rows {
        if row.kind != KIND_SECRET {
            continue;
        }
        let Ok(id) = SecretId::parse(&row.id) else {
            tracing::warn!(id = %row.id, "the server sent a secret id that this Horizon does not read");
            continue;
        };
        if row.deleted {
            if let Err(error) = state.client.forget_secret(id.as_str()) {
                tracing::warn!(%error, "the version of a removed secret was not forgotten");
            }
            // A value that the server refused keeps one copy before the removal
            // takes the value of this computer away.
            let refused = state.refused_secrets.remove(id.as_str());
            rows.push(SecretRow {
                id,
                body: None,
                keep_local: refused,
            });
            continue;
        }
        let keep_local =
            state.client.secret_version(id.as_str()).is_none() || state.client.secret_is_pending(id.as_str());
        // A value that the server refused is kept only when it differs from the
        // value that this answer brings, so an identical key is no conflict.
        let refused = state.refused_secrets.remove(id.as_str());
        let keep_local = keep_local || refused;
        if let Err(error) = state.client.remember_secret(id.as_str(), row.version) {
            tracing::warn!(%error, "the version of a secret was not kept");
        }
        let Some(text) = row.body.as_str() else {
            tracing::warn!(id = %row.id, "the server sent a secret that is not text");
            continue;
        };
        rows.push(SecretRow {
            id,
            body: Some(text.as_bytes().to_vec()),
            keep_local,
        });
    }
    rows
}

/// Writes the secrets of one answer where this computer reads them, and keeps a
/// local value that lost.
///
/// A value that this computer cannot write is one line of the log with its id: the
/// mark of the log moves on, because one value that this computer cannot use must
/// not hold the machine list of every computer, and the value comes again at the
/// next change of that secret or at a reset of the mirror.
pub(super) fn write_secrets(handle: &SyncHandle, rows: &[SecretRow]) {
    let (secrets, at_millis) = {
        let state = handle.lock();
        (Arc::clone(&state.secrets), now_millis())
    };
    let mut notices = Vec::new();
    for row in rows {
        match write_one(&secrets, row, at_millis) {
            Ok(line) => notices.extend(line),
            Err(error) => {
                tracing::warn!(
                    id = %row.id,
                    code = error.code(),
                    reason = %error,
                    "a secret of the server was not written on this computer"
                );
            }
        }
    }
    if !notices.is_empty() {
        let mut state = handle.lock();
        state.notices.extend(notices);
    }
}

/// Writes the value of one row, and answers the line for the person.
fn write_one(secrets: &Arc<dyn SecretTarget>, row: &SecretRow, at_millis: u64) -> Result<Option<String>, SecretError> {
    let Some(body) = &row.body else {
        // A removal is last write wins, so the value of this computer goes. A
        // value that the server refused keeps one copy first, so the person still
        // has what this computer held.
        let kept = if row.keep_local {
            keep_beside(secrets, &row.id, at_millis)?
        } else {
            None
        };
        secrets.remove(&row.id)?;
        return Ok(kept);
    };
    secrets.take_over(&row.id, body, row.keep_local, at_millis)
}

/// Keeps the local value of `id` as a copy beside the value of the server.
fn keep_beside(secrets: &Arc<dyn SecretTarget>, id: &SecretId, at_millis: u64) -> Result<Option<String>, SecretError> {
    match secrets.read(id)? {
        Some(local) => secrets.keep_local(id, &local, at_millis).map(Some),
        None => Ok(None),
    }
}

/// Queues every secret of this computer again, because the server cannot read
/// what it holds and every value that this computer has is new to it.
pub(super) fn reseed_secrets(state: &mut SyncState) -> Result<(), SyncError> {
    state.client.forget_secrets()?;
    for id in local_ids(state) {
        let Ok(id) = SecretId::parse(&id) else {
            continue;
        };
        state.client.enqueue_secret(id.as_str())?;
    }
    Ok(())
}

/// The ids of every secret that this computer holds.
pub(super) fn local_ids(state: &SyncState) -> Vec<String> {
    let mut ids = state.secrets.local_ids(SecretKind::MachineKey);
    ids.extend(state.secrets.local_ids(SecretKind::Credential));
    ids.sort();
    ids.dedup();
    ids
}

/// The machine keys that this computer holds, by name.
pub(super) fn local_machine_keys(secrets: &Arc<dyn SecretTarget>) -> BTreeSet<String> {
    secrets
        .local_ids(SecretKind::MachineKey)
        .into_iter()
        .filter_map(|id| SecretId::parse(&id).ok())
        .map(|id| id.name().to_owned())
        .collect()
}

/// Remembers the secrets that the server refused, and moves the mark of this
/// computer back before the version that a refusal names, so the next answer of
/// changes brings the value of the server again.
///
/// The refusal decides nothing by itself: the value of the server that follows
/// says whether the local value differs at all. A computer that pushed the same
/// key again, after the server lost its key file, therefore keeps no copy and
/// reads no notice when the server answers the same value. A value that the server
/// cannot read is no row of an answer, so the local value stays where it is.
pub(super) fn refuse_secrets(handle: &SyncHandle, refused: &[String], version: Option<u64>) {
    if refused.is_empty() {
        return;
    }
    let mut state = handle.lock();
    state.refused_secrets.extend(refused.iter().cloned());
    if let Some(version) = version {
        state.client.replay_before(version);
    }
}
