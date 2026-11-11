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
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || required_key(&keys, key.as_deref()),
            move |app, required| {
                let key = match required {
                    Some(Ok(key)) => key,
                    Some(Err(error)) => {
                        app.machine_refused(&name, &error);
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

#[cfg(test)]
mod tests;
