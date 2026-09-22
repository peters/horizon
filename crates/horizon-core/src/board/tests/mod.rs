use crate::panel::{PanelKind, PanelOptions};

mod alignment;
#[cfg(feature = "cloud-workspaces")]
mod cloud_collisions;
mod core;
mod layout;
mod reordering;
mod workspace;
mod workspace_separation;

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
