//! Test support for the setup agent's leaves.
//!
//! A test starts no agent: its panel runs no process, so it needs no agent login and no
//! systemd user scope. It places its run through `HorizonApp::place_setup_run`, which the
//! dialog also uses once the scope of its agent is decided, with an empty wrapper.
use super::build_on::Submitted;
use super::prompt::Mode;
use super::session::{self, Prepared};
use crate::app::HorizonApp;
use horizon_core::{PanelId, PanelKind, PresetConfig};
use std::path::Path;

/// The preset every setup test starts: a panel that runs no process.
pub(super) fn test_preset() -> PresetConfig {
    PresetConfig {
        name: "Test panel".into(),
        kind: PanelKind::Usage,
        alias: None,
        command: None,
        args: Vec::new(),
        resume: horizon_core::PanelResume::default(),
        ssh_connection: None,
    }
}

/// The run a thread of the dialog prepares for `folder`.
fn prepared(folder: &Path) -> Prepared {
    session::prepare(&folder.display().to_string(), Mode::New, &[]).expect("the repository was read")
}

/// Places a setup of `folder`, as the dialog does once its run is prepared, and returns the
/// panel of its run: none when the setup did not start.
pub(super) fn place(app: &mut HorizonApp, ctx: &egui::Context, folder: &Path) -> Option<PanelId> {
    let entered = folder.display().to_string();
    app.cloud_prototype.production.repository.clone_from(&entered);
    let runs = app.cloud_prototype.production.setup_runs.len();
    app.place_setup_run(
        ctx,
        Submitted {
            entered,
            preset: test_preset(),
            mode: Mode::New,
        },
        None,
        prepared(folder),
        Vec::new(),
    );
    let started = &app.cloud_prototype.production.setup_runs;
    (started.len() > runs).then(|| started[started.len() - 1].panel)
}

/// Places a setup of `folder`, asserts that it started, and follows it for one frame.
pub(super) fn start(app: &mut HorizonApp, ctx: &egui::Context, folder: &Path) -> PanelId {
    let panel = place(app, ctx, folder).expect("the setup started");
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    app.sync_cloud_repository_setup(ctx);
    panel
}
