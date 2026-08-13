use egui::{Context, RawInput};
use horizon_core::{Config, HorizonHome, RuntimeState, SessionStore, StartupDecision};
use tempfile::TempDir;

use super::HorizonApp;
use crate::input;

pub(super) fn test_app() -> (TempDir, HorizonApp) {
    let (temp, _ctx, app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    (temp, app)
}

pub(super) fn test_app_with_startup(startup: StartupDecision) -> (TempDir, Context, HorizonApp) {
    test_app_with_config_and_startup(&Config::default(), startup)
}

pub(super) fn test_app_with_config_and_startup(
    config: &Config,
    startup: StartupDecision,
) -> (TempDir, Context, HorizonApp) {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.yaml");
    let session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        config_path.clone(),
    );
    let ctx = Context::default();
    let app = HorizonApp::new_with_egui_context(
        &ctx,
        config,
        config_path,
        session_store,
        startup,
        input::ObservedKeyboardInputs::default(),
    );
    (temp, ctx, app)
}

pub(super) fn run_app_frame(ctx: &Context, app: &mut HorizonApp) {
    run_app_frame_with_input(ctx, app, RawInput::default());
}

pub(super) fn run_app_frame_with_input(ctx: &Context, app: &mut HorizonApp, input: RawInput) {
    let mut frame = eframe::Frame::_new_kittest();
    let _ = ctx.run(input, |ctx| {
        eframe::App::update(app, ctx, &mut frame);
    });
}
