use crate::panel::{PanelKind, PanelOptions};

mod alignment;
mod core;
mod layout;
mod workspace;
mod workspace_separation;

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
