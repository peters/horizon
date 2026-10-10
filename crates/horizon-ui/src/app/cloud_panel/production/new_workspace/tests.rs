use super::*;
use crate::{
    app::{sidebar::NewWorkspace, test_support::test_app},
    test_egui::DiscardTextures as _,
};

/// Profiles like Horizon's example: `dev` on a CPU worker and `gpu`, the only one with a GPU.
const GPU_TOO: &str = "version: 1
default: dev
profiles:
  dev:
    provider: runpod
    image: registry.example.com/horizon/dev
    min_cpu: 2
    min_memory_gb: 4
    gpu: false
  gpu:
    provider: runpod
    image: registry.example.com/horizon/gpu
    min_cpu: 8
    min_memory_gb: 32
    gpu: true
";
const CPU_ONLY: &str = "version: 1
default: dev
profiles:
  dev:
    provider: runpod
    image: registry.example.com/horizon/dev
    min_cpu: 2
    min_memory_gb: 4
    gpu: false
";

fn config(text: &str) -> CloudConfig {
    CloudConfig::parse(text).unwrap()
}

fn texts(form: &Production) -> (bool, Vec<String>) {
    let mut chosen = false;
    let output = egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| chosen = notes(ui, form))
        .discard_textures();
    let texts = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    (chosen, texts)
}

#[test]
fn cloud_gpu_takes_the_first_gpu_profile_of_each_repository_or_its_default() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("cloud");
    let form = &mut app.cloud_prototype.production;
    let intent = |gpu| Intent {
        workspace,
        gpu,
        chosen_for: None,
        no_gpu: false,
    };
    form.new_workspace = Some(intent(true));
    form.repository = "/synthetic/cpu-only".into();
    assert_eq!(
        gpu_profile(form, &config(CPU_ONLY)).as_deref(),
        Some("dev"),
        "no GPU profile: the repository's default, not a kept name"
    );
    form.profiles = Some(config(CPU_ONLY));
    let (_, shown) = texts(form);
    assert!(shown.iter().any(|text| text.contains("no GPU profile")), "{shown:?}");
    // Another repository, although it also has `dev`, gets its own GPU profile.
    form.repository = "/synthetic/gpu-too".into();
    assert_eq!(gpu_profile(form, &config(GPU_TOO)).as_deref(), Some("gpu"));
    form.profiles = Some(config(GPU_TOO));
    let (_, shown) = texts(form);
    assert!(!shown.iter().any(|text| text.contains("no GPU profile")), "{shown:?}");
    // A reread of the same repository keeps the person's pick.
    assert_eq!(gpu_profile(form, &config(GPU_TOO)), None);
    // Plain Cloud asks for nothing.
    form.new_workspace = Some(intent(false));
    assert_eq!(gpu_profile(form, &config(GPU_TOO)), None);
}

#[test]
fn a_repository_that_asks_for_this_pc_is_offered_it() {
    let (_temp, mut app) = test_app();
    let form = &mut app.cloud_prototype.production;
    form.profiles = Some(config(GPU_TOO));
    let (chosen, shown) = texts(form);
    assert!(!chosen);
    assert!(
        !shown.iter().any(|text| text == "Open on This PC"),
        "only with placement: local"
    );
    form.profiles = Some(config(&format!("{CPU_ONLY}placement: local\n")));
    let (chosen, shown) = texts(form);
    assert!(!chosen, "nothing is chosen without a click");
    assert!(shown.iter().any(|text| text == "Open on This PC"));
    // Nothing is said while the profiles are being read again.
    form.profiles = None;
    assert!(!texts(form).1.iter().any(|text| text == "Open on This PC"));
    form.profiles = Some(config(&format!("{CPU_ONLY}placement: local\n")));
    let (_, shown) = texts(form);
    assert!(shown.iter().any(|text| text == "This repository runs on This PC"));
    assert!(shown.iter().any(|text| text == "Open on This PC"));
}

#[test]
fn a_cancelled_cloud_takes_away_only_the_empty_workspace_new_workspace_made() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    // This PC makes a plain workspace and no dialog.
    let before = app.board.workspaces.len();
    app.create_new_workspace(&ctx, NewWorkspace::ThisPc, None);
    assert_eq!(app.board.workspaces.len(), before + 1);
    assert!(!app.cloud_prototype.production.creating);
    // Cloud makes one and opens New cloud for it; Cancel takes it away again.
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    assert!(app.cloud_prototype.production.creating);
    assert_eq!(app.board.workspaces.len(), before + 2);
    app.close_cloud_creation();
    assert_eq!(app.board.workspaces.len(), before + 1, "the empty cloud workspace goes");
    // A workspace with something in it stays.
    app.create_new_workspace(&ctx, NewWorkspace::CloudGpu, None);
    let intent = app.cloud_prototype.production.new_workspace.clone().unwrap();
    assert!(intent.gpu);
    let note = PanelOptions {
        kind: horizon_core::PanelKind::Editor,
        ..PanelOptions::default()
    };
    app.create_panel_with_options(note, intent.workspace).unwrap();
    app.discard_new_cloud_workspace();
    assert!(app.board.workspace(intent.workspace).is_some());
}

#[test]
fn a_session_switch_ends_the_dialog_and_its_empty_workspace_before_saving() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    let _existing = app.board.create_workspace("existing workspace");
    let before = app.board.workspaces.len();
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    assert_eq!(app.board.workspaces.len(), before + 1);
    app.close_cloud_creation_for_session_switch();
    assert!(!app.cloud_prototype.production.creating);
    assert_eq!(
        app.board.workspaces.len(),
        before,
        "nothing empty is saved with the board"
    );
    // A dialog that another way opened has no workspace to take away.
    let workspace = app.board.create_workspace("plain");
    app.open_cloud_for_workspace(&ctx, workspace);
    app.close_cloud_creation_for_session_switch();
    assert!(app.board.workspace(workspace).is_some());
}

#[test]
fn a_session_switch_also_ends_the_account_setup_of_a_first_cloud() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    let _existing = app.board.create_workspace("existing workspace");
    let before = app.board.workspaces.len();
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    // The dialog steps aside for the account setup, to come back once keys are saved.
    app.open_cloud_accounts(&ctx, true);
    assert!(!app.cloud_prototype.production.creating);
    assert!(app.cloud_prototype.production.setup.resumes_creation());
    app.close_cloud_creation_for_session_switch();
    assert!(
        !app.cloud_prototype.production.setup.open,
        "the setup ends with the session"
    );
    assert_eq!(
        app.board.workspaces.len(),
        before,
        "nothing empty is saved with the board"
    );
}
