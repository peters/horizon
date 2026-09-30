use super::*;
use crate::test_egui::DiscardTextures;
use egui::{PointerButton, Pos2, Rect, epaint::Shape};

mod providers;

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
            // A two-line choice, such as a profile with its size, is found by its first line.
            Shape::Text(text)
                if text.galley.job.text == label || text.galley.job.text.split('\n').next() == Some(label) =>
            {
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
    let size = ctx.content_rect().size();
    for pressed in [true, false] {
        let mut input = raw_input([size.x, size.y], None);
        input.events = vec![
            Event::PointerMoved(position),
            Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ];
        let _ = ctx
            .run_ui(input, |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
    }
}

fn open_options(ctx: &egui::Context, app: &mut HorizonApp) {
    let output = dialog_frame(ctx, app, Vec::new());
    click(ctx, app, label_rect(&output, "Options").center());
    for _ in 0..3 {
        dialog_frame(ctx, app, Vec::new());
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
    app.cloud_prototype.production.prices.runpod_answered();
    app.add_mock_cloud(ctx);
    for _ in 0..3 {
        dialog_frame(ctx, app, Vec::new());
    }
}

#[test]
fn finishing_a_code_edit_does_not_type_its_last_character_into_the_hidden_title() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let repository = app.cloud_prototype.production.repository.clone();
    let mut prefix = repository.clone();
    let last = prefix.pop().unwrap();
    let form = &mut app.cloud_prototype.production;
    form.title.clear();
    form.focus_title_on_open = true;
    form.source.edit_for_test(&prefix);
    ctx.memory_mut(|memory| memory.request_focus(Id::new("cloud-source")));
    dialog_frame(&ctx, &mut app, vec![Event::Text(last.to_string())]);
    assert_eq!(ctx.memory(egui::Memory::focused), Some(Id::new("cloud-source")));
    open_options(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.source.input(), repository);
    assert!(app.cloud_prototype.production.title.is_empty());
    assert_ne!(ctx.memory(egui::Memory::focused), Some(Id::new("cloud-title")));
}

#[test]
fn pointer_selects_prebuilt_and_creates_it_inside_a_short_viewport() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    open_options(&ctx, &mut app);
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

/// A `RunPod` catalog with compute-optimized and general-purpose CPU flavors in stock in
/// one European data center.
fn answer_cpu_catalog(app: &mut HorizonApp) {
    use horizon_core::cloud_runtime::prices::{
        Availability, CpuFlavorPrice, DataCenter, Preferences, PriceList, RUNPOD_STORAGE,
    };
    let flavor = |id: &str, name: &str, per_vcpu_hour| CpuFlavorPrice {
        id: id.into(),
        name: name.into(),
        per_vcpu_hour,
    };
    let list = PriceList {
        provider: "RunPod",
        cpu: vec![
            flavor("cpu3c", "Compute-Optimized", 0.03),
            flavor("cpu3g", "General Purpose", 0.04),
        ],
        gpus: Vec::new(),
        data_centers: vec![DataCenter {
            id: "EU-RO-1".into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            high_performance_storage: false,
            cpus: vec![
                ("cpu3c".into(), Availability::High),
                ("cpu3g".into(), Availability::High),
            ],
            gpus: Vec::new(),
        }],
        regions: std::collections::BTreeMap::new(),
        storage: RUNPOD_STORAGE,
    };
    let preferences = Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    };
    app.cloud_prototype
        .production
        .prices
        .answered(list, preferences, Vec::new());
}

#[test]
fn chosen_size_starts_the_cloud_at_that_size() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    answer_cpu_catalog(&mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(
        &output,
        "Profile development requires at least 4 vCPU and 8 GB memory"
    ));
    assert!(has_label(&output, "CHEAPEST") && has_label(&output, "MOST POWERFUL"));
    assert!(
        !has_label(&output, "2 vCPU · 4 GB"),
        "sizes below the profile are never offered"
    );
    assert_eq!(
        app.cloud_prototype.production.size, None,
        "the profile's size is the default"
    );
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Search, e.g. 4090 or 16 vCPU").center(),
    );
    let mut input = raw_input([1000.0, 2400.0], None);
    input.events = vec![Event::Text("16 vCPU".into())];
    let _ = ctx
        .run_ui(input, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures();
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "16 vCPU · 64 GB").center());
    assert_eq!(app.cloud_prototype.production.size, Some((16, 64)));
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    assert!(!app.cloud_creation_open());
    assert_eq!(created_size(&app), (16, 64));
    let launch = app.cloud_prototype.groups.0[0].remote.as_ref().unwrap();
    assert_eq!(launch.profile_name, "development");
    assert_eq!(launch.profile.image, "example.invalid/worker");
}

#[test]
fn chosen_region_places_the_cloud_there_and_sold_out_regions_remain_selectable() {
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
        high_performance_storage: false,
        cpus: Vec::new(),
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
        regions: std::collections::BTreeMap::new(),
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
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Data center"));
    assert!(has_label(&output, "Any data center\n1 in stock"));
    assert!(has_label(
        &output,
        "Horizon picks a data center with stock. The workspace stays there, and a stopped cloud resumes there."
    ));
    click(
        &ctx,
        &mut app,
        label_rect(&output, "North America\nnone in stock").center(),
    );
    assert_eq!(
        app.cloud_prototype.production.placement,
        horizon_core::cloud_panel::Placement {
            cpu_types: Vec::new(),
            region: Some("North America".into()),
            data_centers: vec!["US-MO-2".into()],
            gpu_types: Vec::new(),
        },
        "a sold-out region remains selectable"
    );
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Europe\n1 in stock").center());
    let europe = horizon_core::cloud_panel::Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
        gpu_types: Vec::new(),
    };
    assert_eq!(app.cloud_prototype.production.placement, europe);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(
        &output,
        "The workspace stays in Europe, and a stopped cloud resumes there."
    ));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.placement, europe, "the cloud keeps where it may be placed");
}

/// A GPU profile whose price list has only the RTX A5000 in stock, with `preferred`
/// as the machine's GPU preferences.
fn gpu_dialog(preferred: &[&str]) -> (tempfile::TempDir, egui::Context, HorizonApp) {
    use horizon_core::cloud_runtime::prices::{
        Availability, DataCenter, GpuPrice, Preferences, PriceList, RUNPOD_STORAGE,
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let production = &mut app.cloud_prototype.production;
    let config = production.profiles.as_mut().unwrap();
    config.profiles.get_mut("development").unwrap().gpu = true;
    let gpu = |id: &str, name: &str, hourly| GpuPrice {
        id: id.into(),
        name: name.into(),
        memory_gb: 24,
        hourly,
    };
    let list = PriceList {
        provider: "RunPod",
        cpu: Vec::new(),
        gpus: vec![
            gpu("NVIDIA RTX A6000", "RTX A6000", 0.53),
            gpu("NVIDIA RTX A5000", "RTX A5000", 0.27),
        ],
        data_centers: vec![DataCenter {
            id: "EU-RO-1".into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            high_performance_storage: false,
            cpus: Vec::new(),
            gpus: vec![("NVIDIA RTX A5000".into(), Availability::High)],
        }],
        regions: std::collections::BTreeMap::new(),
        storage: RUNPOD_STORAGE,
    };
    let preferences = Preferences {
        cpu_flavors: Vec::new(),
        gpu_types: preferred.iter().map(|&gpu| gpu.to_owned()).collect(),
    };
    production.prices.answered(list, preferences, Vec::new());
    (temp, ctx, app)
}

/// A tall window keeps the price card inside the dialog's scroll area.
fn tall_frame(ctx: &egui::Context, app: &mut HorizonApp) -> egui::FullOutput {
    for _ in 0..3 {
        let _ = ctx
            .run_ui(raw_input([1000.0, 2400.0], None), |ui| {
                app.render_cloud_creation(ui.ctx());
            })
            .discard_textures();
    }
    ctx.run_ui(raw_input([1000.0, 2400.0], None), |ui| {
        app.render_cloud_creation(ui.ctx());
    })
    .discard_textures()
}

#[test]
fn a_sold_out_gpu_preference_gives_way_to_one_in_stock_for_this_cloud() {
    let (_temp, ctx, mut app) = gpu_dialog(&["NVIDIA RTX A6000"]);
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    // The cheapest type in stock is requested explicitly, for this cloud only.
    assert_eq!(app.cloud_prototype.production.placement.gpu_types, ["NVIDIA RTX A5000"]);
    assert!(has_label(&output, "RTX A5000"));
    // The sold-out preference can be revealed and chosen without changing the default.
    assert!(!has_label(&output, "RTX A6000"));
    click(&ctx, &mut app, label_rect(&output, "In stock only").center());
    let output = tall_frame(&ctx, &mut app);
    let rows: Vec<_> = output
        .shapes
        .iter()
        .filter(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == "RTX A6000"))
        .collect();
    assert!(!rows.is_empty());
    click(&ctx, &mut app, label_rect(&output, "RTX A6000").center());
    assert_eq!(app.cloud_prototype.production.placement.gpu_types, ["NVIDIA RTX A6000"]);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Start new cloud once available"));
    click(&ctx, &mut app, label_rect(&output, "RTX A5000").center());
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(&output, "Start new cloud once available"));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.placement.gpu_types, ["NVIDIA RTX A5000"]);
}

#[test]
fn a_gpu_in_stock_is_chosen_without_preferences_and_dropped_for_a_cpu_profile() {
    let (_temp, ctx, mut app) = gpu_dialog(&[]);
    tall_frame(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.placement.gpu_types, ["NVIDIA RTX A5000"]);
    // Reread as a CPU profile, the GPU choice no longer applies.
    let config = app.cloud_prototype.production.profiles.as_mut().unwrap();
    config.profiles.get_mut("development").unwrap().gpu = false;
    tall_frame(&ctx, &mut app);
    assert!(app.cloud_prototype.production.placement.gpu_types.is_empty());
}

#[test]
fn size_choice_resets_with_the_profile_or_repository_and_gpu_minimums_can_change() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    open_options(&ctx, &mut app);
    let config = app.cloud_prototype.production.profiles.as_mut().unwrap();
    let mut accelerated = config.profiles["development"].clone();
    accelerated.gpu = true;
    config.profiles.insert("accelerated".into(), accelerated);
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    assert!(has_label(&output, "Profile"));
    assert!(has_label(&output, "accelerated\nGPU worker"));
    app.cloud_prototype.production.size = Some((16, 32));
    click(&ctx, &mut app, label_rect(&output, "development").center());
    assert_eq!(app.cloud_prototype.production.size, Some((16, 32)));
    let output = dialog_frame(&ctx, &mut app, Vec::new());
    click(&ctx, &mut app, label_rect(&output, "accelerated").center());
    assert_eq!(app.cloud_prototype.production.selected_profile, "accelerated");
    assert_eq!(app.cloud_prototype.production.size, None);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "GPU workers for the accelerated profile"));
    assert!(!has_label(
        &output,
        "Profile accelerated requires at least 4 vCPU and 8 GB memory"
    ));
    app.cloud_prototype.production.size = Some((3, 17));
    let profile = &app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["accelerated"];
    let sized =
        creation::provider::sized(&horizon_core::cloud_runtime::provider::RUNPOD, profile, Some((3, 17))).unwrap();
    assert_eq!((sized.cpu, sized.memory_gb), (3, 17));
    app.set_cloud_repository(&temp.path().join("another-repository"));
    assert_eq!(app.cloud_prototype.production.size, None);
}

#[test]
fn keyboard_selects_prebuilt_without_reclaiming_cleared_focus() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    open_options(&ctx, &mut app);
    assert_eq!(ctx.memory(egui::Memory::focused), Some(Id::new("cloud-title")));
    ctx.memory_mut(|memory| memory.surrender_focus(Id::new("cloud-title")));
    dialog_frame(&ctx, &mut app, Vec::new());
    assert_eq!(ctx.memory(egui::Memory::focused), None);
    ctx.memory_mut(|memory| memory.request_focus(Id::new("cloud-title")));
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
    for _ in 0..2 {
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

#[test]
fn disk_edits_preserve_gpu_placement_and_survive_launch_capture() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // As the dialog first opens, with no GPU offers drawn over the disk fields; this
    // test creates the cloud directly rather than through a queued submission.
    app.cloud_prototype.production.prices = super::super::prices::State::default();
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .gpu = true;
    let selected = horizon_core::cloud_panel::Placement {
        cpu_types: Vec::new(),
        region: Some("Chosen region".into()),
        data_centers: vec!["EU-RO-1".into()],
        gpu_types: vec!["chosen-gpu".into()],
    };
    app.cloud_prototype.production.placement = selected.clone();
    let mut render = |events: Vec<Event>| {
        let mut input = raw_input([1200.0, 1200.0], None);
        input.events = events;
        ctx.run_ui(input, |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures()
    };
    let mut output = render(Vec::new());
    for _ in 0..3 {
        output = render(Vec::new());
    }
    let at = label_rect(&output, "20").center();
    for _ in 0..2 {
        for pressed in [true, false] {
            render(vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
    }
    render(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    render(vec![Event::Text("160".into())]);
    render(vec![Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(
        app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["development"]
            .storage
            .volume_gb,
        160
    );
    assert_eq!(app.cloud_prototype.production.placement, selected);
    app.cloud_prototype.production.prices.refresh();
    app.create_production_cloud(&ctx).unwrap();
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.storage.volume_gb, 160);
    assert_eq!(launch.placement, selected);
    let encoded = serde_json::to_vec(launch).unwrap();
    let restored: horizon_core::cloud_panel::CloudLaunch = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored.profile.storage, launch.profile.storage);
    assert_eq!(restored.placement, selected);
}

#[test]
fn container_edit_requires_a_compatible_cpu_size_before_starting() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .volume_gb = 25;
    let placement = horizon_core::cloud_panel::Placement {
        data_centers: vec!["EU-RO-1".into()],
        ..Default::default()
    };
    app.cloud_prototype.production.placement = placement.clone();
    answer_cpu_catalog(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Options").center());
    let output = tall_frame(&ctx, &mut app);
    let at = label_rect(&output, "20").center();
    click(&ctx, &mut app, at);
    click(&ctx, &mut app, at);
    let mut input = raw_input([1000.0, 2400.0], None);
    input.events = vec![
        Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        },
        Event::Text("61".into()),
        Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        },
    ];
    let _ = ctx
        .run_ui(input, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures();
    let output = tall_frame(&ctx, &mut app);
    let reason = "Choose a CPU and memory size that supports this container disk before starting.";
    assert!(has_label(&output, reason));
    assert_eq!(
        app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["development"]
            .storage
            .container_gb,
        61
    );
    assert_eq!(app.cloud_prototype.production.placement, placement);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    assert!(app.cloud_creation_open());
    assert!(!app.cloud_prototype.production.launch.submitted);
    assert!(app.cloud_prototype.groups.0.is_empty());
    // Sizes that cannot hold the disk are no longer offered; the cheapest left can.
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "8 vCPU · 16 GB").center());
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(&output, reason));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!((launch.profile.cpu, launch.profile.memory_gb), (8, 16));
    assert_eq!(launch.profile.storage.container_gb, 61);
    assert_eq!(launch.placement, placement);
}
