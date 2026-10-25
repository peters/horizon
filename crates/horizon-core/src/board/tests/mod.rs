use crate::panel::{PanelKind, PanelOptions};

mod alignment;
#[cfg(feature = "cloud-workspaces")]
mod cloud_collisions;
#[cfg(feature = "cloud-workspaces")]
mod cloud_retention;
#[cfg(feature = "cloud-workspaces")]
mod cloud_slots;
mod core;
mod disposal;
mod layout;
mod panel_working_directory;
mod reordering;
mod workspace;
mod workspace_separation;

/// Shell panels with the default program: `/bin/bash` when `SHELL` is unset (#688).
fn shell_panel_options() -> PanelOptions {
    PanelOptions::default()
}

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
