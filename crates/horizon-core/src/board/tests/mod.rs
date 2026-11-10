use crate::panel::{PanelKind, PanelOptions};

mod alignment;
mod core;
mod layout;
mod machine_restore;
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
