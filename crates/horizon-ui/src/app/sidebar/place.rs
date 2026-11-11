//! The Place menu of a workspace: where it lives, and where it can live.
//!
//! A workspace lives on this computer or on one machine of Horizon's machine
//! list. The menu says where it lives now, with a check mark, and one entry per
//! machine of the list. A place whose machine was forgotten stays in the menu,
//! dimmed and marked `(gone)`, so the person sees why a new terminal of that
//! workspace fails and can put the workspace somewhere else.

use horizon_core::{Machines, WorkspacePlace};

/// The words of the entry of this computer.
pub(in crate::app) const THIS_COMPUTER: &str = "This computer";

/// What the menu adds to the name of a machine that the list no longer keeps.
pub(in crate::app) const GONE: &str = "(gone)";

/// One entry of a workspace's Place menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct PlaceEntry {
    /// What the row says.
    pub(in crate::app) label: String,
    /// The place that the entry puts the workspace in.
    pub(in crate::app) place: WorkspacePlace,
    /// Whether the workspace is in this place now.
    pub(in crate::app) current: bool,
    /// Whether the machine of this entry is gone from the list.
    pub(in crate::app) gone: bool,
}

/// The entries of the Place menu of a workspace that is in `place`.
pub(in crate::app) fn entries(place: &WorkspacePlace, machines: &Machines) -> Vec<PlaceEntry> {
    let mut entries = vec![PlaceEntry {
        label: THIS_COMPUTER.to_owned(),
        place: WorkspacePlace::Local,
        current: place.is_local(),
        gone: false,
    }];
    let mut place_is_kept = false;
    for record in machines.records() {
        let current = place.is_machine(&record.name);
        place_is_kept |= current;
        entries.push(PlaceEntry {
            label: record.name.clone(),
            place: WorkspacePlace::Machine(record.name.clone()),
            current,
            gone: false,
        });
    }
    if let Some(name) = place.machine().filter(|_| !place_is_kept) {
        entries.push(PlaceEntry {
            label: format!("{name} {GONE}"),
            place: WorkspacePlace::Machine(name.to_owned()),
            current: true,
            gone: true,
        });
    }
    entries
}

#[cfg(test)]
mod tests;
