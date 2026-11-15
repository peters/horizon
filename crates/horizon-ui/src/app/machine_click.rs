//! The terminal that a click on a machine row opens.
//!
//! The person's click is a second way into the panel that an agent's
//! `open_terminal { machine }` opens, so it re-validates the record and checks
//! the key on the coordination worker exactly as the agent path does, with
//! [`HorizonApp::open_machine_terminal`]. It differs in one way before that call:
//! a machine that this computer cannot open yet says why in the sidebar, instead
//! of a login that could only reach a host that this computer never agreed to.

use egui::Context;
use horizon_core::MachineError;

use super::HorizonApp;

/// The titlebar mark of a terminal that the person opened from the machine row.
pub(in crate::app) const OPENED_FROM_MACHINES: &str = "Machines";

impl HorizonApp {
    /// Opens the terminal of `name` in the active workspace and focuses it, as a
    /// click on the machine row asks for it. A caller focuses the panel of a
    /// machine that is already open instead of calling this.
    pub(in crate::app) fn connect_machine(&mut self, name: &str, ctx: &Context) {
        let key = self.machine_list.key(name).map(str::to_owned);
        // A machine that only the server gave has no key on this computer, and a
        // login with the keys of the person would reach a host that this computer
        // never agreed to. The snapshot of the link is the one the worker published,
        // so the click of a person waits for no disk.
        let link = self.server_sync.as_ref().map(super::server_sync::ServerSync::snapshot);
        let foreign = link.as_ref().is_some_and(|link| link.is_foreign(name));
        let link_has_key = link
            .as_ref()
            .map(|link| key.as_deref().is_some_and(|key| link.has_machine_key(key)));
        if let Some(notice) = missing_key_notice(name, foreign, key.as_deref(), link_has_key) {
            self.set_sidebar_notice(notice);
            return;
        }
        self.open_machine_terminal(name, None, ctx);
    }

    /// Says why a click on a machine opened nothing, in the sidebar.
    pub(in crate::app) fn machine_refused(&mut self, name: &str, error: &MachineError) {
        self.set_sidebar_notice(format!("{name}: {} ({})", error, error.code()));
    }
}

/// The notice of a machine whose key is not on this computer: a person deleted it
/// here or on another computer, so a click opens nothing. The person asks an agent
/// for a new key, and the agent installs the new public line on the machine.
pub(in crate::app) fn deleted_key_notice(name: &str) -> String {
    format!("The key of {name} was deleted. Ask an agent to make a new key and install it.")
}

/// The notice of a machine that this computer cannot open yet: a record that
/// arrived from the server with no key binding here, or a record whose binding
/// names a key that this computer does not hold. A click on it opens nothing,
/// because the only login it could try is the one of the keys of the person.
///
/// `link_has_key` is `None` when this computer follows no server, and then the
/// key files themselves decide, with the error of the key that is not there.
pub(in crate::app) fn missing_key_notice(
    name: &str,
    foreign: bool,
    key: Option<&str>,
    link_has_key: Option<bool>,
) -> Option<String> {
    match (key, link_has_key) {
        (Some(_), Some(false)) => Some(deleted_key_notice(name)),
        (Some(_), Some(true) | None) => None,
        (None, _) => foreign.then(|| format!("This computer has no key for {name} yet")),
    }
}

#[cfg(test)]
mod tests;
