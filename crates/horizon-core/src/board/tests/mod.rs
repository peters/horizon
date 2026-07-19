use crate::panel::{PanelKind, PanelOptions};

mod core;
mod layout;
mod workspace;
mod workspace_attention;

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
