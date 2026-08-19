use crate::test_egui::DiscardTextures;
use egui::{Context, Pos2, RawInput, Rect, ViewportId};
use horizon_core::{
    Config, HorizonHome, PanelKind, PanelState, RuntimeState, SessionStore, StartupDecision, WorkspaceState,
};
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

pub(super) fn run_app_frame_with_input(ctx: &Context, app: &mut HorizonApp, input: RawInput) -> egui::FullOutput {
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run_ui(input, |ui| {
        eframe::App::ui(app, ui, &mut frame);
    })
    .discard_textures()
}

#[derive(Debug)]
struct TestDroppedFile(std::path::PathBuf);

impl egui::DroppedFile for TestDroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

pub(super) fn dropped_file(path: impl Into<std::path::PathBuf>) -> egui::DroppedFileHandle {
    std::sync::Arc::new(TestDroppedFile(path.into()))
}

pub(super) fn raw_input(size: [f32; 2], position: Option<[f32; 2]>) -> RawInput {
    let inner_rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]));
    let mut input = RawInput {
        screen_rect: Some(inner_rect),
        ..RawInput::default()
    };
    let viewport = input.viewports.entry(ViewportId::ROOT).or_default();
    viewport.inner_rect = Some(inner_rect);
    viewport.outer_rect =
        position.map(|position| Rect::from_min_size(Pos2::new(position[0], position[1]), egui::vec2(size[0], size[1])));
    input
}

pub(super) fn editor_panel_state(local_id: &str, position: [f32; 2]) -> PanelState {
    PanelState {
        local_id: local_id.to_string(),
        name: format!("{local_id} notes"),
        kind: PanelKind::Editor,
        position: Some(position),
        size: Some([320.0, 220.0]),
        ..PanelState::default()
    }
}

pub(super) fn editor_workspace_state(local_id: &str, position: [f32; 2]) -> WorkspaceState {
    WorkspaceState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        position: Some(position),
        panels: vec![editor_panel_state(
            &format!("{local_id}-panel"),
            [position[0] + 20.0, position[1] + 60.0],
        )],
        ..WorkspaceState::default()
    }
}
