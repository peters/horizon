use crate::panel::{PanelKind, PanelOptions};

mod core;
mod layout;
mod workspace;

fn editor_panel_options() -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Editor,
        ..PanelOptions::default()
    }
}
