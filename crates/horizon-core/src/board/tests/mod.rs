use crate::panel::{PanelKind, PanelOptions};

mod alignment;
#[cfg(feature = "cloud-workspaces")]
mod cloud_collisions;
mod core;
mod layout;
mod reordering;
mod workspace;
mod workspace_separation;

/// Shell panels whose program Windows can launch: the default shell is
/// `/bin/bash` when `SHELL` is unset (#688), so they run `cmd.exe` there.
fn shell_panel_options() -> PanelOptions {
    PanelOptions {
        command: cfg!(windows).then(|| "cmd.exe".to_string()),
        ..PanelOptions::default()
    }
}

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
