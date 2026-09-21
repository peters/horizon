use super::*;
use crate::app::test_support::test_app_with_startup;
use horizon_core::{RuntimeState, StartupDecision};
use std::time::{Duration, Instant};

fn wait(app: &mut HorizonApp, ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.cloud_prototype.production.setup.receiver.is_some() {
        assert!(Instant::now() < deadline, "account operation did not complete");
        app.poll_cloud_accounts(ctx);
        std::thread::yield_now();
    }
}

#[test]
fn first_use_validation_retains_input_and_cancel_does_not_write() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let root = temp.path().join("cloud");
    app.cloud_prototype.root = Some(root.clone());
    app.open_cloud_accounts(&ctx, true);
    wait(&mut app, &ctx);
    assert!(app.cloud_creation_open());
    assert!(!app.cloud_prototype.production.creating);
    let draft = app.cloud_prototype.production.setup.draft.as_mut().unwrap();
    *draft.runpod_key = "synthetic-key".into();
    draft.settings.default_agents.clear();
    app.save_cloud_accounts(&ctx);
    wait(&mut app, &ctx);
    let state = &app.cloud_prototype.production.setup;
    assert!(state.error.as_ref().unwrap().contains("Choose at least one"));
    assert_eq!(state.draft.as_ref().unwrap().runpod_key.as_str(), "synthetic-key");
    assert!(!root.exists());
    app.cloud_prototype.production.setup = State::default();
    assert!(!app.cloud_creation_open());
    assert!(!root.exists());
}

#[test]
fn successful_first_use_continues_to_creation_and_reopening_preserves_bindings() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let root = temp.path().join("cloud");
    app.cloud_prototype.root = Some(root.clone());
    app.open_cloud_accounts(&ctx, true);
    wait(&mut app, &ctx);
    *app.cloud_prototype.production.setup.draft.as_mut().unwrap().runpod_key = "synthetic-key".into();
    app.save_cloud_accounts(&ctx);
    wait(&mut app, &ctx);
    assert!(app.cloud_prototype.production.creating);
    assert!(!app.cloud_prototype.production.setup.open);
    let before = std::fs::read(root.join("settings.json")).unwrap();
    app.open_cloud_accounts(&ctx, true);
    wait(&mut app, &ctx);
    assert!(app.cloud_prototype.production.creating);
    assert_eq!(std::fs::read(root.join("settings.json")).unwrap(), before);
    app.open_cloud_accounts(&ctx, false);
    wait(&mut app, &ctx);
    assert!(!app.cloud_prototype.production.creating);
    assert!(
        app.cloud_prototype
            .production
            .setup
            .draft
            .as_ref()
            .unwrap()
            .runpod_key
            .is_empty()
    );
}

#[test]
fn account_form_never_renders_the_secret_value() {
    let temp = tempfile::tempdir().unwrap();
    let mut draft = Draft::load(temp.path()).unwrap();
    *draft.runpod_key = "synthetic-secret-marker".into();
    let ctx = Context::default();
    for _ in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| fields::render(ui, &mut draft));
        output.textures_delta.clear();
        for shape in output.shapes {
            if let egui::epaint::Shape::Text(text) = shape.shape {
                assert!(!text.galley.job.text.contains("synthetic-secret-marker"));
            }
        }
    }
}

#[test]
fn account_dialog_stays_inside_the_viewport_after_shrinking() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([3840.0, 2140.0], None));
    }
    app.cloud_prototype.root = Some(temp.path().join("cloud"));
    app.open_cloud_accounts(&ctx, false);
    wait(&mut app, &ctx);
    app.cloud_prototype.production.setup.draft.as_mut().unwrap().openai_auth =
        horizon_core::cloud_runtime::setup::Authentication::ApiKey;
    app.cloud_prototype.production.setup.error = Some("Account settings need correction.\n".repeat(30));
    for size in [[3840.0, 2140.0], [900.0, 700.0], [800.0, 600.0]] {
        for _ in 0..8 {
            run_app_frame_with_input(&ctx, &mut app, raw_input(size, None));
        }
        let output = run_app_frame_with_input(&ctx, &mut app, raw_input(size, None));
        let dialog = ctx
            .memory(|memory| memory.area_rect(Id::new("cloud-accounts")))
            .unwrap();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
        assert!(
            viewport.contains_rect(dialog),
            "{size:?}: dialog {dialog:?} exceeds {viewport:?}"
        );
        for label in ["Save settings", "Cancel"] {
            let shape = output
                .shapes
                .iter()
                .find(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text == label))
                .unwrap();
            let egui::epaint::Shape::Text(text) = &shape.shape else {
                unreachable!()
            };
            let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
            assert!(viewport.contains_rect(bounds));
            assert!(shape.clip_rect.contains_rect(bounds), "{label} is clipped at {size:?}");
        }
    }
}

#[test]
fn held_escape_closes_accounts_without_reaching_terminal_or_fullscreen() {
    use crate::app::test_support::{raw_input, run_app_frame_with_input};
    use egui::{Event, Key, Modifiers};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let received = temp.path().join("pty-input");
    let ready = temp.path().join("ready");
    let workspace = app.board.ensure_workspace();
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "stty raw -echo; : > \"$1\"; cat > \"$2\"".into(),
                    "capture".into(),
                    ready.to_string_lossy().into(),
                    received.to_string_lossy().into(),
                ],
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() || !received.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.board.focus(panel);
    for _ in 0..2 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    }
    app.cloud_prototype.root = Some(temp.path().join("cloud"));
    app.fullscreen_panel = Some(panel);
    app.open_cloud_accounts(&ctx, false);
    wait(&mut app, &ctx);
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    for (pressed, repeat) in [(true, false), (true, true), (false, false)] {
        let mut input = raw_input([1400.0, 900.0], None);
        input.events = vec![Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        }];
        run_app_frame_with_input(&ctx, &mut app, input);
    }
    assert!(!app.cloud_creation_open());
    assert_eq!(app.fullscreen_panel, Some(panel));
    std::thread::sleep(Duration::from_millis(50));
    assert!(std::fs::read(&received).unwrap().is_empty());
    app.board.panel_mut(panel).unwrap().write_input(b"capture-check");
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read(&received).unwrap() != b"capture-check" {
        assert!(Instant::now() < deadline, "PTY capture must observe actual input");
        std::thread::sleep(Duration::from_millis(10));
    }
}
