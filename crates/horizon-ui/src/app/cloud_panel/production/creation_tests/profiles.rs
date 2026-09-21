use super::*;
use crate::test_egui::DiscardTextures;
use egui::{PointerButton, Pos2, Rect, epaint::Shape};

fn dialog_frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>) -> egui::FullOutput {
    let mut input = raw_input([900.0, 600.0], None);
    input.events = events;
    ctx.run_ui(input, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures()
}

fn label_rect(output: &egui::FullOutput, label: &str) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing visible label: {label}"))
}

fn click(ctx: &egui::Context, app: &mut HorizonApp, position: Pos2) {
    for pressed in [true, false] {
        dialog_frame(
            ctx,
            app,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
}

fn prepare(app: &mut HorizonApp, ctx: &egui::Context, directory: &std::path::Path) {
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Initial fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(directory)
                .status()
                .unwrap()
                .success()
        );
    }
    let mut config = CloudConfig::parse(
        "version: 1\ndefault: development\nprofiles:\n  development:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    ).unwrap();
    config
        .profiles
        .insert("prebuilt".into(), config.profiles["development"].clone());
    app.cloud_prototype.production.title = "Selection regression".into();
    app.cloud_prototype.production.repository = directory.to_string_lossy().into();
    app.cloud_prototype.production.profiles = Some(config);
    app.cloud_prototype.production.selected_profile = "development".into();
    app.add_mock_cloud(ctx);
    for _ in 0..3 {
        dialog_frame(ctx, app, Vec::new());
    }
}

#[test]
fn pointer_selects_prebuilt_and_creates_it_inside_a_short_viewport() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    let profile = label_rect(&output, "prebuilt");
    let create = label_rect(&output, "Create cloud");
    assert!(Rect::from_min_max(Pos2::ZERO, egui::pos2(900.0, 600.0)).contains_rect(create));
    click(&ctx, &mut app, profile.center());
    assert!(app.cloud_creation_open());
    assert_eq!(app.cloud_prototype.production.selected_profile, "prebuilt");
    click(&ctx, &mut app, create.center());
    assert!(!app.cloud_creation_open());
    assert_eq!(
        app.cloud_prototype
            .groups
            .0
            .last()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .profile_name,
        "prebuilt"
    );
}

#[test]
fn keyboard_selects_prebuilt_without_reclaiming_cleared_focus() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    assert_eq!(ctx.memory(egui::Memory::focused), Some(Id::new("cloud-title")));
    ctx.memory_mut(|memory| memory.surrender_focus(Id::new("cloud-title")));
    dialog_frame(&ctx, &mut app, Vec::new());
    assert_eq!(ctx.memory(egui::Memory::focused), None);
    ctx.memory_mut(|memory| memory.request_focus(Id::new("cloud-revision")));
    let press = |app: &mut HorizonApp, key| {
        for pressed in [true, false] {
            dialog_frame(
                &ctx,
                app,
                vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
            );
        }
    };
    for _ in 0..3 {
        press(&mut app, Key::Tab);
    }
    press(&mut app, Key::Space);
    assert_eq!(app.cloud_prototype.production.selected_profile, "prebuilt");
    press(&mut app, Key::Escape);
    assert!(!app.cloud_creation_open());
    app.add_mock_cloud(&ctx);
    dialog_frame(&ctx, &mut app, Vec::new());
    assert_eq!(ctx.memory(egui::Memory::focused), Some(Id::new("cloud-title")));
}

#[test]
fn cloud_creation_requires_the_selected_workspace_in_the_main_window() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let workspace = app.board.ensure_workspace();
    app.detach_workspace(workspace);
    assert!(app.workspace_is_detached(workspace));
    let error = app.create_production_cloud(&ctx).unwrap_err();
    assert!(error.to_string().contains("main window"));
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.production.runtimes.is_empty());
    assert!(app.cloud_creation_open());
    app.reattach_workspace(&ctx, workspace);
    app.process_pending_detached_reattach(&ctx);
    app.create_production_cloud(&ctx).unwrap();
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
}

#[test]
fn new_cloud_placement_is_relative_to_translated_workspace() {
    for origin in [[500.0, -100.0], [-500.0, 250.0], [0.0, 0.0], [5000.0, 5000.0]] {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        prepare(&mut app, &ctx, temp.path());
        let workspace = app.board.ensure_workspace();
        app.board.workspace_mut(workspace).unwrap().position = origin;
        app.create_production_cloud(&ctx).unwrap();
        assert_position(
            app.cloud_prototype.groups.0[0].position,
            [origin[0] + 24.0, origin[1] + 128.0],
        );
        let initial_view = app.canvas_view;
        app.cloud_prototype.groups.reconcile(&mut app.board);
        app.cloud_overview(&ctx);
        assert_eq!(app.canvas_view, initial_view);
        app.create_production_cloud(&ctx).unwrap();
        let first = &app.cloud_prototype.groups.0[0];
        let second = &app.cloud_prototype.groups.0[1];
        assert_position(first.position, [origin[0] + 24.0, origin[1] + 128.0]);
        assert_position(
            second.position,
            [first.position[0], first.overview_bounds().1[1] + 48.0],
        );
        let positions = [first.position, second.position];
        let overview = app.canvas_view;
        let saved = serde_json::to_vec(&app.cloud_prototype.groups).unwrap();
        app.cloud_prototype.groups = serde_json::from_slice(&saved).unwrap();
        for _ in 0..3 {
            app.cloud_prototype.groups.reconcile(&mut app.board);
            app.cloud_overview(&ctx);
            assert_eq!(app.canvas_view, overview);
            assert_position(app.cloud_prototype.groups.0[0].position, positions[0]);
            assert_position(app.cloud_prototype.groups.0[1].position, positions[1]);
        }
        assert!(app.cloud_prototype.production.runtimes.is_empty());
    }
}

fn assert_position(actual: [f32; 2], expected: [f32; 2]) {
    assert!(
        actual.into_iter().zip(expected).all(|(a, b)| (a - b).abs() < 0.001),
        "{actual:?} != {expected:?}"
    );
}
