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

fn has_label(output: &egui::FullOutput, label: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == label))
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
    app.cloud_prototype.production.launch.accounts_checked = true;
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
    click(&ctx, &mut app, label_rect(&output, "Advanced").center());
    for _ in 0..8 {
        dialog_frame(&ctx, &mut app, Vec::new());
    }
    dialog_frame(
        &ctx,
        &mut app,
        vec![
            Event::PointerMoved(egui::pos2(450.0, 300.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, -240.0),
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..8 {
        dialog_frame(&ctx, &mut app, Vec::new());
    }
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    let profile = label_rect(&output, "prebuilt");
    let create = label_rect(&output, "Start cloud");
    assert!(Rect::from_min_max(Pos2::ZERO, egui::pos2(900.0, 600.0)).contains_rect(create));
    click(&ctx, &mut app, profile.center());
    assert!(app.cloud_creation_open());
    assert_eq!(app.cloud_prototype.production.selected_profile, "prebuilt");
    app.cloud_prototype.production.launch.accounts_checked = true;
    click(&ctx, &mut app, create.center());
    finish_creation(&ctx, &mut app);
    assert!(!app.cloud_creation_open());
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert_eq!(
        app.cloud_prototype
            .groups
            .0
            .last()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .revision,
        String::from_utf8(commit.stdout).unwrap().trim()
    );
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
    assert_eq!(
        created_size(&app),
        (4, 8),
        "an unchanged choice keeps the profile's size"
    );
}

fn scroll(ctx: &egui::Context, app: &mut HorizonApp, delta: f32) {
    dialog_frame(
        ctx,
        app,
        vec![
            Event::PointerMoved(egui::pos2(450.0, 300.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, delta),
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..8 {
        dialog_frame(ctx, app, Vec::new());
    }
}

fn created_size(app: &HorizonApp) -> (u16, u16) {
    let profile = &app
        .cloud_prototype
        .groups
        .0
        .last()
        .unwrap()
        .remote
        .as_ref()
        .unwrap()
        .profile;
    (profile.cpu, profile.memory_gb)
}

#[test]
fn chosen_size_starts_the_cloud_at_that_size() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(
        has_label(&output, "Size"),
        "the size row is outside the collapsed Advanced section"
    );
    assert!(has_label(&output, "development · 4 vCPU · 8 GB"));
    assert!(has_label(&output, "8 GB · compute-optimized"));
    // Tooltips would draw below this modal, so the offer rule is shown inline.
    assert!(has_label(
        &output,
        "RunPod CPU sizes offered with this profile's 20 GB container disk."
    ));
    assert!(
        !has_label(&output, "1 vCPU"),
        "RunPod CPU pods need a power of two from 2 vCPU"
    );
    assert_eq!(
        app.cloud_prototype.production.size, None,
        "the profile's size is the default"
    );
    click(&ctx, &mut app, label_rect(&output, "16 vCPU").center());
    assert_eq!(
        app.cloud_prototype.production.size,
        Some((16, 32)),
        "vCPU changes keep the memory family"
    );
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "development · 16 vCPU · 32 GB"));
    assert!(
        !has_label(&output, "8 GB · compute-optimized"),
        "memory choices follow the vCPU count"
    );
    click(&ctx, &mut app, label_rect(&output, "64 GB · general purpose").center());
    assert_eq!(app.cloud_prototype.production.size, Some((16, 64)));
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "development · 16 vCPU · 64 GB"));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    assert!(!app.cloud_creation_open());
    assert_eq!(created_size(&app), (16, 64));
    let launch = app.cloud_prototype.groups.0[0].remote.as_ref().unwrap();
    assert_eq!(launch.profile_name, "development");
    assert_eq!(launch.profile.image, "example.invalid/worker");
}

#[test]
fn chosen_region_places_the_cloud_there_and_sold_out_regions_cannot_be_chosen() {
    use horizon_core::cloud_runtime::prices::{
        Availability, CpuFlavorPrice, DataCenter, PriceList, RUNPOD_STORAGE, SizeAvailability,
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let center = |id: &str, region: &str| DataCenter {
        id: id.into(),
        region: region.into(),
        workspace_storage: true,
        gpus: Vec::new(),
    };
    let list = PriceList {
        provider: "RunPod",
        cpu: vec![CpuFlavorPrice {
            id: "cpu3c".into(),
            name: "Compute-Optimized".into(),
            per_vcpu_hour: 0.03,
        }],
        gpus: Vec::new(),
        data_centers: vec![
            center("EU-RO-1", "EUROPE"),
            center("EUR-IS-1", "EUROPE"),
            center("US-MO-2", "NORTH_AMERICA"),
        ],
        storage: RUNPOD_STORAGE,
    };
    let production = &mut app.cloud_prototype.production;
    let profile = production.profiles.as_ref().unwrap().profiles["development"].clone();
    let preferences = horizon_core::cloud_runtime::prices::Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    };
    let stock = SizeAvailability {
        centers: vec![("EU-RO-1".into(), Availability::High)],
    };
    production.prices.answered(list, preferences, vec![(profile, stock)]);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "Region"));
    assert!(has_label(&output, "Any region\n1 in stock"));
    assert!(has_label(
        &output,
        "Horizon picks a data center with stock. The workspace stays there, and a stopped cloud resumes there."
    ));
    click(
        &ctx,
        &mut app,
        label_rect(&output, "North America\nnone in stock").center(),
    );
    assert!(
        app.cloud_prototype.production.placement.is_any(),
        "a sold-out region cannot be chosen"
    );
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "Europe\n1 in stock").center());
    let europe = horizon_core::cloud_panel::Placement {
        region: Some("Europe".into()),
        data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
    };
    assert_eq!(app.cloud_prototype.production.placement, europe);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(
        &output,
        "The workspace stays in Europe, and a stopped cloud resumes there."
    ));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.placement, europe, "the cloud keeps where it may be placed");
}

#[test]
fn size_choice_resets_with_the_profile_or_repository_and_gpu_size_is_fixed() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let config = app.cloud_prototype.production.profiles.as_mut().unwrap();
    let mut accelerated = config.profiles["development"].clone();
    accelerated.gpu = true;
    config.profiles.insert("accelerated".into(), accelerated);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "16 vCPU").center());
    assert_eq!(app.cloud_prototype.production.size, Some((16, 32)));
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "Advanced").center());
    for _ in 0..8 {
        dialog_frame(&ctx, &mut app, Vec::new());
    }
    scroll(&ctx, &mut app, -240.0);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "16 vCPU · 32 GB memory · CPU only"));
    click(&ctx, &mut app, label_rect(&output, "development").center());
    assert_eq!(
        app.cloud_prototype.production.size,
        Some((16, 32)),
        "reselecting the same profile keeps its size"
    );
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "accelerated").center());
    assert_eq!(app.cloud_prototype.production.selected_profile, "accelerated");
    assert_eq!(app.cloud_prototype.production.size, None);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "4 vCPU · 8 GB memory · GPU"));
    // Offscreen labels are not painted, so return to the size row before checking absences.
    scroll(&ctx, &mut app, 480.0);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "Size"));
    assert!(has_label(&output, "4 vCPU · 8 GB · GPU"), "GPU size is shown as text");
    assert!(has_label(&output, "GPU workers use the size set by their profile."));
    for choice in ["2 vCPU", "4 vCPU", "16 vCPU", "8 GB · compute-optimized"] {
        assert!(!has_label(&output, choice), "GPU profiles offer no {choice} choice");
    }
    app.cloud_prototype.production.selected_profile = "development".into();
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "32 vCPU").center());
    assert_eq!(app.cloud_prototype.production.size, Some((32, 64)));
    app.set_cloud_repository(&temp.path().join("another-repository"));
    assert_eq!(app.cloud_prototype.production.size, None);
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
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "Advanced").center());
    for _ in 0..8 {
        dialog_frame(&ctx, &mut app, Vec::new());
    }
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
    finish_creation(&ctx, &mut app);
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
        app.cloud_prototype.production.creating = true;
        app.create_production_cloud(&ctx).unwrap();
        finish_creation(&ctx, &mut app);
        assert_position(
            app.cloud_prototype.groups.0[0].position,
            [origin[0] + 24.0, origin[1] + 128.0],
        );
        let initial_view = app.canvas_view;
        app.cloud_prototype.groups.reconcile(&mut app.board);
        app.cloud_overview(&ctx);
        assert_eq!(app.canvas_view, initial_view);
        app.cloud_prototype.production.creating = true;
        app.create_production_cloud(&ctx).unwrap();
        finish_creation(&ctx, &mut app);
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

fn finish_creation(ctx: &egui::Context, app: &mut HorizonApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.cloud_prototype.production.pending_creation.is_some() {
        assert!(Instant::now() < deadline, "repository validation did not complete");
        dialog_frame(ctx, app, Vec::new());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
}
