//! What one push changes, decided and written inside the one transaction of that
//! push.
//!
//! Every intent that the writer reads is decided: it is applied, or it is refused
//! with a typed reason. Either way the decision takes the next log index and the
//! identifier of the intent is remembered, so a resend answers the first result
//! and changes nothing. A refusal writes no object and no version: a computer
//! drops a refused intent instead of sending it again.

use horizon_core::MAX_MACHINES;
use horizon_core::secret::{MAX_SECRET_BYTES as MAX_SECRET_BODY_BYTES, SecretId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::error::{Refusal, StoreError};
use crate::intent::{
    Intent, IntentResult, KIND_BINDING, KIND_MACHINE, KIND_PROBE, KIND_SECRET, MAX_PROBES, MAX_SECRET_BYTES,
    MAX_SECRETS, Op, Outcome, PushReply,
};
use crate::store::rows::to_sql;
use crate::store::secrets::{self, SecretKey};

/// The most characters of a value of a frame that the log or a refusal quotes.
const QUOTED_CHARS: usize = 80;

/// Decides and writes the intents of one push. The caller commits the
/// transaction and answers after the commit.
///
/// # Errors
/// Returns why the log or the objects cannot be written; the caller rolls the
/// whole push back, so a failed push writes nothing.
pub(super) fn push(
    transaction: &Transaction<'_>,
    device: &str,
    intents: &[Intent],
    key: &SecretKey,
    now_millis: i64,
) -> Result<PushReply, StoreError> {
    let mut next_index = next_index(transaction)?;
    let mut last_intent = last_intent(transaction, device)?;
    let mut results = Vec::with_capacity(intents.len());
    for intent in intents {
        let outcome = if let Some(outcome) = remembered(transaction, device, intent.id)? {
            outcome
        } else {
            let outcome = decide(transaction, device, intent, key, next_index, now_millis)?;
            log(transaction, device, intent, next_index, now_millis, &outcome)?;
            next_index += 1;
            outcome
        };
        if intent.id > last_intent {
            last_intent = intent.id;
        }
        results.push(IntentResult { id: intent.id, outcome });
    }
    remember_device(transaction, device, last_intent, now_millis)?;
    Ok(PushReply {
        results,
        next_expected: last_intent.saturating_add(1),
    })
}

/// The first index of this push: one above the last row of the log, so the index
/// has no gap.
fn next_index(transaction: &Transaction<'_>) -> Result<u64, StoreError> {
    let index: i64 = transaction.query_row("SELECT COALESCE(MAX(idx), 0) FROM log", [], |row| row.get(0))?;
    Ok(u64::try_from(index).unwrap_or(0).saturating_add(1))
}

/// The highest intent counter of `device` that the server has already decided.
fn last_intent(transaction: &Transaction<'_>, device: &str) -> Result<u64, StoreError> {
    let last: Option<i64> = transaction
        .query_row("SELECT last_intent FROM devices WHERE name = ?1", [device], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(last.map_or(0, |last| u64::try_from(last).unwrap_or(0)))
}

/// The result that this device already got for this intent counter, when the
/// server decided it before.
fn remembered(transaction: &Transaction<'_>, device: &str, intent: u64) -> Result<Option<Outcome>, StoreError> {
    let stored: Option<String> = transaction
        .query_row(
            "SELECT log.result_json FROM applied JOIN log ON log.idx = applied.idx
             WHERE applied.device = ?1 AND applied.intent = ?2",
            params![device, to_sql(intent)?],
            |row| row.get(0),
        )
        .optional()?;
    stored
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| StoreError::Damaged(format!("the result of an applied intent is not JSON: {error}")))
        })
        .transpose()
}

/// Writes the decision of one intent as the log row that gives it its index, and
/// remembers the identifier of the intent.
fn log(
    transaction: &Transaction<'_>,
    device: &str,
    intent: &Intent,
    index: u64,
    now_millis: i64,
    outcome: &Outcome,
) -> Result<(), StoreError> {
    let result_json = serde_json::to_string(outcome)
        .map_err(|error| StoreError::Damaged(format!("the server could not encode a result: {error}")))?;
    transaction.execute(
        "INSERT INTO log (idx, at_millis, device, intent, kind, object_id, result_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            to_sql(index)?,
            now_millis,
            device,
            to_sql(intent.id)?,
            quoted(intent.op.kind()),
            quoted(intent.op.object_id()),
            result_json
        ],
    )?;
    transaction.execute(
        "INSERT INTO applied (device, intent, idx) VALUES (?1, ?2, ?3)",
        params![device, to_sql(intent.id)?, to_sql(index)?],
    )?;
    Ok(())
}

/// Writes the row of the computer and the highest intent counter that it has
/// spent.
fn remember_device(
    transaction: &Transaction<'_>,
    device: &str,
    last_intent: u64,
    now_millis: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO devices (name, first_seen, last_seen, last_intent) VALUES (?1, ?2, ?2, ?3)
         ON CONFLICT (name) DO UPDATE SET last_seen = excluded.last_seen,
             last_intent = MAX(devices.last_intent, excluded.last_intent)",
        params![device, now_millis, to_sql(last_intent)?],
    )?;
    Ok(())
}

/// Applies one intent, or refuses it with the reason. Nothing is written here.
fn decide(
    transaction: &Transaction<'_>,
    device: &str,
    intent: &Intent,
    key: &SecretKey,
    index: u64,
    now_millis: i64,
) -> Result<Outcome, StoreError> {
    if intent.device != device {
        return Ok(refused(Refusal::InvalidIntent {
            message: format!(
                "The intent {} names the computer {:?}, but this link is {device:?}",
                intent.id,
                quoted(&intent.device)
            ),
        }));
    }
    match &intent.op {
        Op::MachinePut { record } => machine_put(transaction, record, index, now_millis),
        Op::MachineRemove { name } => machine_remove(transaction, name, index),
        Op::BindingPut { machine, key } => binding_put(transaction, machine, key.as_deref(), index),
        Op::SecretPut { id, body } => secret_put(transaction, key, intent.seen, id, body.as_ref(), index),
        Op::SecretRemove { id } => secret_remove(transaction, key, id, index),
        Op::ObjectPut { kind, id, body } => object_put(transaction, intent.seen, kind, id, body, index),
        Op::ObjectRemove { kind, id } => object_remove(transaction, kind, id, index),
    }
}

/// Adds or replaces one machine. The machine list is not versioned: the commit
/// order of the server decides between two computers, and `seen` is not read.
///
/// The time that the record says it was added is the time of this server, never
/// the clock of the computer that sent it: the first store of a name stamps the
/// time of that store, and a later change of the same name keeps the time that
/// the server already holds, so two computers read one day for one machine.
fn machine_put(
    transaction: &Transaction<'_>,
    record: &horizon_core::Machine,
    index: u64,
    now_millis: i64,
) -> Result<Outcome, StoreError> {
    let mut record = match horizon_core::check_record(record) {
        Ok(record) => record,
        Err(error) => return Ok(invalid_record(error.code(), &error.to_string())),
    };
    let stored = stored_machine(transaction, &record.name)?;
    if stored.is_none() && live_count(transaction, KIND_MACHINE)? >= count_of(MAX_MACHINES) {
        return Ok(refused(Refusal::TooMany {
            kind: KIND_MACHINE.to_owned(),
            limit: count_of(MAX_MACHINES),
        }));
    }
    record.added_at_millis = stored.map_or_else(
        || u64::try_from(now_millis).unwrap_or(0),
        |stored| stored.added_at_millis,
    );
    let body = serde_json::to_value(&record)
        .map_err(|error| StoreError::Damaged(format!("the server could not encode a machine: {error}")))?;
    put(transaction, KIND_MACHINE, &record.name, &body, index, false)?;
    Ok(Outcome::Applied { index })
}

/// The record that the server holds for `name`, when it holds a live one. Its
/// time is the one that a later change keeps.
fn stored_machine(transaction: &Transaction<'_>, name: &str) -> Result<Option<horizon_core::Machine>, StoreError> {
    let body: Option<String> = transaction
        .query_row(
            "SELECT body_json FROM objects WHERE kind = ?1 AND id = ?2 AND deleted = 0",
            params![KIND_MACHINE, name],
            |row| row.get(0),
        )
        .optional()?;
    let Some(body) = body else {
        return Ok(None);
    };
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|error| StoreError::Damaged(format!("the record of the machine {name} is not JSON: {error}")))
}

/// Forgets one machine. The row stays as a tombstone, so a computer that was away
/// removes it from its own list.
fn machine_remove(transaction: &Transaction<'_>, name: &str, index: u64) -> Result<Outcome, StoreError> {
    if let Err(error) = horizon_core::machines::machine_name(name) {
        return Ok(invalid_record(error.code(), &error.to_string()));
    }
    put(transaction, KIND_MACHINE, name, &Value::Null, index, true)?;
    Ok(Outcome::Applied { index })
}

/// Sets the key that a machine uses, or forgets it. A binding is last write wins,
/// as the machine list is, because two computers that bind a key decide by commit
/// order and not by a version.
fn binding_put(
    transaction: &Transaction<'_>,
    machine: &str,
    key: Option<&str>,
    index: u64,
) -> Result<Outcome, StoreError> {
    if let Err(error) = horizon_core::machines::machine_name(machine) {
        return Ok(invalid_record(error.code(), &error.to_string()));
    }
    let body = match key {
        Some(key) => {
            if let Err(error) = horizon_core::machines::machine_key_name(key) {
                return Ok(invalid_record(error.code(), &error.to_string()));
            }
            serde_json::json!({ "key": key })
        }
        None => Value::Null,
    };
    put(transaction, KIND_BINDING, machine, &body, index, key.is_none())?;
    Ok(Outcome::Applied { index })
}

/// Writes one versioned secret, by the version rule: the first intent on a
/// version wins, and a later one is refused with the version that the server has.
/// The body is the only value that reaches the `secrets` table, and the log row of
/// this change holds the id and the version and not the body.
fn secret_put(
    transaction: &Transaction<'_>,
    key: &SecretKey,
    seen: Option<u64>,
    id: &str,
    body: Option<&crate::intent::SecretBody>,
    index: u64,
) -> Result<Outcome, StoreError> {
    if let Err(refusal) = check_secret_id(id) {
        return Ok(refused(refusal));
    }
    let Some(body) = body else {
        return Ok(refused(Refusal::InvalidIntent {
            message: format!(
                "The intent on the secret {:?} carries no value; a computer reads the value from its own record before it pushes",
                quoted(id)
            ),
        }));
    };
    if body.as_str().len() > MAX_SECRET_BODY_BYTES {
        return Ok(refused(Refusal::InvalidIntent {
            message: format!("A secret holds at most {MAX_SECRET_BYTES} bytes, and this one is larger"),
        }));
    }
    match (seen, secrets::version_of(transaction, key, id)?) {
        (None, None) => {
            if secrets::live_count(transaction)? >= MAX_SECRETS {
                return Ok(refused(Refusal::TooMany {
                    kind: KIND_SECRET.to_owned(),
                    limit: MAX_SECRETS,
                }));
            }
            secrets::put(transaction, key, id, Some(body.as_str().as_bytes()), index)?;
            Ok(Outcome::Applied { index })
        }
        (None, Some(version)) => Ok(refused(Refusal::AlreadyAnswered { version })),
        (Some(seen), Some(version)) if seen == version => {
            secrets::put(transaction, key, id, Some(body.as_str().as_bytes()), index)?;
            Ok(Outcome::Applied { index })
        }
        (Some(seen), Some(version)) => Ok(refused(Refusal::ChangedSinceSeen { seen, version })),
        (Some(seen), None) => Ok(refused(Refusal::ChangedSinceSeen { seen, version: 0 })),
    }
}

/// Removes one secret. A removal is last write wins, as a machine removal is, so
/// a computer that was away removes its own copy at its next pull.
fn secret_remove(transaction: &Transaction<'_>, key: &SecretKey, id: &str, index: u64) -> Result<Outcome, StoreError> {
    if let Err(refusal) = check_secret_id(id) {
        return Ok(refused(refusal));
    }
    secrets::put(transaction, key, id, None, index)?;
    Ok(Outcome::Applied { index })
}

/// Whether `id` names one secret that Horizon keeps: a machine key of this name,
/// or a credential of this reference. The server does not parse the body, but it
/// does check the id, so no id is a path or a command.
fn check_secret_id(id: &str) -> Result<SecretId, Refusal> {
    SecretId::parse(id).map_err(|error| Refusal::InvalidIntent {
        message: error.to_string(),
    })
}

/// Writes one versioned object of an internal kind, by the version rule: the
/// first intent on a version wins, and a later one is refused with the version
/// that the server has.
fn object_put(
    transaction: &Transaction<'_>,
    seen: Option<u64>,
    kind: &str,
    id: &str,
    body: &Value,
    index: u64,
) -> Result<Outcome, StoreError> {
    if kind != KIND_PROBE {
        return Ok(refused(Refusal::UnknownKind { kind: quoted(kind) }));
    }
    if let Err(refusal) = check_object_id(id) {
        return Ok(refused(refusal));
    }
    match (seen, version_of(transaction, kind, id)?) {
        (None, None) => {
            if live_count(transaction, KIND_PROBE)? >= MAX_PROBES {
                return Ok(refused(Refusal::TooMany {
                    kind: KIND_PROBE.to_owned(),
                    limit: MAX_PROBES,
                }));
            }
            put(transaction, kind, id, body, index, false)?;
            Ok(Outcome::Applied { index })
        }
        (None, Some(version)) => Ok(refused(Refusal::AlreadyAnswered { version })),
        (Some(seen), Some(version)) if seen == version => {
            put(transaction, kind, id, body, index, false)?;
            Ok(Outcome::Applied { index })
        }
        (Some(seen), Some(version)) => Ok(refused(Refusal::ChangedSinceSeen { seen, version })),
        (Some(seen), None) => Ok(refused(Refusal::ChangedSinceSeen { seen, version: 0 })),
    }
}

/// Removes one versioned object of an internal kind. A removal is last write
/// wins, as the machine list is, and its tombstone is the change.
fn object_remove(transaction: &Transaction<'_>, kind: &str, id: &str, index: u64) -> Result<Outcome, StoreError> {
    if kind != KIND_PROBE {
        return Ok(refused(Refusal::UnknownKind { kind: quoted(kind) }));
    }
    if let Err(refusal) = check_object_id(id) {
        return Ok(refused(refusal));
    }
    put(transaction, kind, id, &Value::Null, index, true)?;
    Ok(Outcome::Applied { index })
}

/// The version of one live object, or `None` when it is not there or is removed.
fn version_of(transaction: &Transaction<'_>, kind: &str, id: &str) -> Result<Option<u64>, StoreError> {
    let found: Option<i64> = transaction
        .query_row(
            "SELECT version FROM objects WHERE kind = ?1 AND id = ?2 AND deleted = 0",
            params![kind, id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.and_then(|version| u64::try_from(version).ok()))
}

/// The number of live objects of one kind.
fn live_count(transaction: &Transaction<'_>, kind: &str) -> Result<u64, StoreError> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM objects WHERE kind = ?1 AND deleted = 0",
        [kind],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// Writes one object row at this version, or replaces it.
fn put(
    transaction: &Transaction<'_>,
    kind: &str,
    id: &str,
    body: &Value,
    index: u64,
    deleted: bool,
) -> Result<(), StoreError> {
    let body_json = serde_json::to_string(body)
        .map_err(|error| StoreError::Damaged(format!("the server could not encode an object: {error}")))?;
    transaction.execute(
        "INSERT INTO objects (kind, id, body_json, version, deleted) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (kind, id) DO UPDATE SET body_json = excluded.body_json,
             version = excluded.version, deleted = excluded.deleted",
        params![kind, id, body_json, to_sql(index)?, i64::from(deleted)],
    )?;
    Ok(())
}

/// An object id of the internal kind: 1 to 64 characters of `a-z`, `0-9`, `-`,
/// `_` and `.`, so no id is a path or a command.
fn check_object_id(id: &str) -> Result<(), Refusal> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        return Ok(());
    }
    Err(Refusal::InvalidIntent {
        message: format!(
            "The object id {:?} is not 1 to 64 characters of a-z, 0-9, '-', '_' and '.'",
            quoted(id)
        ),
    })
}

/// A value that the computer sent, as the log and a refusal quote it: at most 64
/// characters, so one intent writes about one line of log and not a frame.
fn quoted(text: &str) -> String {
    if text.chars().count() <= QUOTED_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(QUOTED_CHARS).collect();
    out.push_str("...");
    out
}

/// `count` as the number that the store holds.
fn count_of(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

fn invalid_record(code: &str, message: &str) -> Outcome {
    refused(Refusal::InvalidRecord {
        code: code.to_owned(),
        message: message.to_owned(),
    })
}

fn refused(refusal: Refusal) -> Outcome {
    Outcome::Refused(refusal)
}
