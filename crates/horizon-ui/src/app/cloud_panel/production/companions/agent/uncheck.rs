//! An agent unchecks a companion as the owner's checkbox on the source cloud's card
//! does. Agents may remove access but never grant it: checking stays the owner's
//! authorization, so there is no tool for it.
use super::super::Entry;
use horizon_core::browser::manifest;
use horizon_core::cloud_runtime::companions::{self, Action, Owner, Status};
use serde_json::{Value, json};
use std::path::Path;

/// The answer while a companion change is in progress on the source cloud.
pub(super) const BUSY: &str = "cloud_companion_busy: a companion change is already in progress on this cloud, by the owner on the card or by an earlier uncheck; retry in a moment";

/// Checks an uncheck of `alias` on the source cloud `source`: `None` when the companion is
/// checked, so its deselection is saved next, else the answer.
pub(super) fn uncheck(entry: &Entry, source: &str, alias: &str) -> Result<Option<Value>, String> {
    // A companion change is in progress, the owner's or an earlier uncheck's, of this
    // companion or another: queuing a clear would drop or race it, so it lands first
    // and the agent asks again. A pending refresh is no change: the clear replaces it,
    // as the card's own clear does.
    let selection_pending = matches!(entry.pending, Some(Action::Select { .. }));
    if !entry.selecting.is_empty() || !entry.clearing.is_empty() || selection_pending {
        return Err(BUSY.into());
    }
    let snapshot = entry
        .snapshot
        .as_ref()
        .ok_or("cloud_companion_unavailable: Horizon has not read this cloud's companions yet; retry shortly")?;
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.companion.alias == alias)
        .ok_or("cloud_companion_unknown_alias: the source repository declares no companion with this alias")?;
    if !row.companion.selected {
        // Unchecked already, but its access may still be on its way out.
        let status = serde_json::to_value(row.companion.status).unwrap_or(Value::Null);
        let message = if row.companion.status == Status::Unselected {
            "The companion was not checked; there is no access to remove."
        } else {
            "The companion is already unchecked, but the removal of its access is not done: wait until cloud_companions shows status unselected."
        };
        return Ok(Some(json!({
            "cloud": source,
            "alias": alias,
            "queued": false,
            "changed": false,
            "status": status,
            "message": message,
        })));
    }
    Ok(None)
}

/// Saves the deselection of `alias` in the source's journal, waiting a little while a
/// refresh still holds it: `false` when the agent stopped waiting first, since a late
/// uncheck changes nothing, and else the answer when it is not saved.
pub(super) fn save(root: &Path, owner: &Owner, alias: &str, deadline: i64) -> Result<bool, String> {
    let saved = super::retry_busy(|| {
        if manifest::now_millis() >= deadline {
            return Ok(false);
        }
        companions::persist_deselections(root, owner, std::slice::from_ref(&alias.to_owned())).map(|()| true)
    });
    let Err(error) = saved else {
        return Ok(saved.unwrap_or_default());
    };
    // A save can fail after the new journal is in place: the journal says what holds.
    match super::retry_busy(|| companions::deselected(root, owner, alias)) {
        Ok(true) => Ok(true),
        Ok(false) => Err(format!(
            "cloud_companion_unavailable: Horizon could not save the uncheck, so nothing changed: {error}; retry in a moment"
        )),
        Err(_) => Err(format!(
            "cloud_companion_unavailable: Horizon could not save the uncheck and cannot read whether it took effect: {error}; cloud_companions shows status unselected or revocation_pending when it did, else retry"
        )),
    }
}

/// The answer to an uncheck once its save ended.
pub(super) fn answer(source: &str, alias: &str, saved: Result<bool, String>) -> Result<Value, String> {
    match saved {
        Ok(true) => Ok(json!({
            "cloud": source,
            "alias": alias,
            "queued": true,
            "changed": true,
            "message": "The uncheck is saved and queued as the box on the card queues it: the source's alias, key and the agent's copy go first, and the target's grant once both workers confirm; an offline worker confirms when it next runs. The removal is complete when cloud_companions shows status unselected; selected false alone comes first, and status revocation_pending means the cleanup is not done yet. No cloud was started, stopped or deleted. Only the owner can check it again, on the card.",
        })),
        Ok(false) => Err(super::EXPIRED.into()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
