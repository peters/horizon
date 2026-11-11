//! The terminal that a click on a machine row opens.
//!
//! The person's click is a second way into the panel that an agent's
//! `open_terminal { machine }` opens, so it re-validates the record and checks
//! the key on the coordination worker exactly as the agent path does. It differs
//! in three ways: it lands in the active workspace, it takes keyboard focus, and
//! its titlebar carries the Horizon mark of the machine section.

use egui::Context;
use horizon_core::{MachineError, MachineKeys};

use super::HorizonApp;
use super::machine_open::{connection_for, machine_panel_options, required_key};

/// The titlebar mark of a terminal that the person opened from the machine row.
pub(in crate::app) const OPENED_FROM_MACHINES: &str = "Machines";

impl HorizonApp {
    /// Opens the terminal of `name` in the active workspace and focuses it, as a
    /// click on the machine row asks for it. A caller focuses the panel of a
    /// machine that is already open instead of calling this.
    pub(in crate::app) fn connect_machine(&mut self, name: &str, ctx: &Context) {
        let Some(record) = self.machine_list.find(name).cloned() else {
            self.machine_refused(
                name,
                &MachineError::Unavailable {
                    reason: format!("This Horizon has no machine {name} anymore"),
                },
            );
            return;
        };
        let terminal = match record.checked_terminal() {
            Ok(terminal) => terminal,
            Err(error) => {
                self.machine_refused(name, &error);
                return;
            }
        };
        let Some(workspace) = self.board.active_workspace else {
            self.set_sidebar_notice(format!("{name}: there is no workspace to open a terminal in"));
            return;
        };
        let keys = MachineKeys::from_home(self.session_store.home());
        let (name, title) = (name.to_owned(), record.title());
        let key = self.machine_list.key(name.as_str()).map(str::to_owned);
        // A machine that only the server gave has no key on this computer, and a
        // login with the keys of the person would reach a host that this computer
        // never agreed to. The server keeps the public lines in a later slice.
        let foreign = self
            .server_sync
            .as_ref()
            .is_some_and(|sync| sync.handle().is_foreign(&name));
        let link_has_key = self
            .server_sync
            .as_ref()
            .map(|sync| key.as_deref().is_some_and(|key| sync.handle().has_machine_key(key)));
        if let Some(notice) = missing_key_notice(&name, foreign, key.as_deref(), link_has_key) {
            self.set_sidebar_notice(notice);
            return;
        }
        let opening = ctx.clone();
        let bound = key.clone();
        self.browser_create_host.io.then(
            move || required_key(&keys, key.as_deref()),
            move |app, required| {
                let key = match required {
                    Some(Ok(key)) => key,
                    Some(Err(error)) => {
                        // A record that binds a key which this computer no longer
                        // holds is a key that a person deleted here or on another
                        // computer, and the only login left is the one of the
                        // person's own keys.
                        if error.code() == horizon_browser_control::manifest::block_keys::KEY_UNAVAILABLE
                            && bound.is_some()
                        {
                            app.set_sidebar_notice(deleted_key_notice(&name));
                        } else {
                            app.machine_refused(&name, &error);
                        }
                        opening.request_repaint();
                        return;
                    }
                    None => return,
                };
                let options = machine_panel_options(
                    &name,
                    &title,
                    connection_for(&terminal, key.as_ref()),
                    Some(OPENED_FROM_MACHINES.to_owned()),
                );
                // The person pressed the row, so the reason of a terminal that
                // did not start is theirs to read, not the log's only.
                if let Err(error) = app.create_machine_panel(workspace, options, None) {
                    app.set_sidebar_notice(format!("{name}: {error}"));
                }
                opening.request_repaint();
            },
        );
    }

    /// Says why a click on a machine opened nothing, in the sidebar.
    fn machine_refused(&mut self, name: &str, error: &MachineError) {
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
