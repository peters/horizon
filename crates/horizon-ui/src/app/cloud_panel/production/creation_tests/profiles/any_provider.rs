//! New cloud's "Any provider with this size": when it is offered, the order it shows,
//! and the provider a cloud created with it records.
use super::*;
use horizon_core::cloud_runtime::prices::{CpuFlavorPrice, DataCenter, Preferences, PriceList, RUNPOD_STORAGE};

const CHECKBOX: &str = "Any provider with this size";

/// `RunPod` prices a 4 vCPU compute-optimized worker at $0.12 an hour, and Hetzner's
/// cx33 in hel1 has the same size at €0.0136.
fn both_providers(app: &mut HorizonApp) {
    let list = PriceList {
        provider: "RunPod",
        cpu: vec![CpuFlavorPrice {
            id: "cpu3c".into(),
            name: "Compute-Optimized".into(),
            per_vcpu_hour: 0.03,
        }],
        gpus: Vec::new(),
        data_centers: vec![DataCenter {
            id: "EU-RO-1".into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            gpus: Vec::new(),
        }],
        regions: std::collections::BTreeMap::new(),
        storage: RUNPOD_STORAGE,
    };
    let preferences = Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    };
    let prices = &mut app.cloud_prototype.production.prices;
    prices.answered(list, preferences, Vec::new());
    let catalog = serde_json::from_value(serde_json::json!({
        "offers": [{"server_type": "cx33", "location": "hel1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
            "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": true, "recommended": true}],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5}, "ipv4_hour_eur": {"hel1": 0.0008},
        "regions": {"hel1": "EUROPE"},
    }))
    .unwrap();
    prices.hetzner.answered_with_types(Some(catalog), &["cx33"]);
}

fn painted(output: &egui::FullOutput) -> String {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn set_profile(app: &mut HorizonApp, change: impl FnOnce(&mut horizon_core::cloud_runtime::prices::Profile)) {
    let config = app.cloud_prototype.production.profiles.as_mut().unwrap();
    change(config.profiles.get_mut("development").unwrap());
}

#[test]
fn the_checkbox_is_offered_only_when_another_provider_can_run_the_profile() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    assert!(
        !has_label(&tall_frame(&ctx, &mut app), CHECKBOX),
        "not offered with RunPod alone"
    );
    both_providers(&mut app);
    assert!(has_label(&tall_frame(&ctx, &mut app), CHECKBOX));
    // Hetzner has no GPUs, so a GPU profile has one provider to run on.
    set_profile(&mut app, |profile| profile.gpu = true);
    assert!(!has_label(&tall_frame(&ctx, &mut app), CHECKBOX));
}

#[test]
fn checking_it_replaces_the_provider_choice_with_the_order_providers_are_tried_in() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Provider"));
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(&output, "Provider"), "the choice is Horizon's now");
    assert!(has_label(&output, "Creates on RunPod 4 vCPU · 8 GB at $0.12/h"));
    assert!(has_label(
        &output,
        "Also has this size: Hetzner cx33 in hel1 at €0.0136/h."
    ));
    assert!(painted(&output).contains("this profile's own provider comes first"));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "runpod");
}

#[test]
fn a_hetzner_profile_is_created_on_hetzner_in_any_allowed_location() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    set_profile(&mut app, |profile| profile.provider = "hetzner".into());
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Creates on Hetzner cx33 in hel1 at €0.0136/h"));
    assert!(has_label(
        &output,
        "Also has this size: RunPod 4 vCPU · 8 GB at $0.12/h."
    ));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "hetzner");
    assert!(
        launch.placement.is_any(),
        "every allowed location stays open, so a sold-out one falls through"
    );
}

#[test]
fn a_size_no_provider_has_cannot_be_started() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    // Compute-optimized workers have 2 GB per vCPU, and cx33 has 8 GB.
    app.cloud_prototype.production.size = Some((4, 16));
    let output = tall_frame(&ctx, &mut app);
    assert!(painted(&output).contains("No configured provider has a worker of this size"));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_creation_open());
}

#[test]
fn another_provider_is_used_when_the_profiles_own_has_no_worker_of_this_size() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    set_profile(&mut app, |profile| profile.provider = "hetzner".into());
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    // No configured Hetzner type has 8 vCPU.
    app.cloud_prototype.production.size = Some((8, 16));
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Creates on RunPod 8 vCPU · 16 GB at $0.24/h"));
    assert!(
        !painted(&output).contains("own provider comes first"),
        "the profile's own provider has no worker of this size"
    );
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "runpod");
    assert_eq!((launch.profile.cpu, launch.profile.memory_gb), (8, 16));
}

#[test]
fn nothing_is_chosen_while_a_providers_prices_are_still_arriving() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // Hetzner has answered; RunPod has not yet.
    app.cloud_prototype.production.prices.list = None;
    let catalog = serde_json::from_value(serde_json::json!({
        "offers": [{"server_type": "cx33", "location": "hel1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
            "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": true, "recommended": true}],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5}, "ipv4_hour_eur": {"hel1": 0.0008},
        "regions": {"hel1": "EUROPE"},
    }))
    .unwrap();
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .answered_with_types(Some(catalog), &["cx33"]);
    assert!(app.cloud_prototype.production.prices.runpod_pending());
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Checking which providers have this size…"));
    assert!(
        !painted(&output).contains("Hetzner cx33"),
        "no order before every provider answers"
    );
    assert_eq!(app.cloud_prototype.production.provider, None);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    // Once RunPod answers, the profile's own provider comes first.
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Creates on RunPod 4 vCPU · 8 GB at $0.12/h"));
}

#[test]
fn turning_it_on_reopens_every_allowed_location() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    set_profile(&mut app, |profile| profile.provider = "hetzner".into());
    both_providers(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["hel1"]);
    let output = tall_frame(&ctx, &mut app);
    // Hetzner ranks first, the provider already in use.
    click(&ctx, &mut app, label_rect(&output, CHECKBOX).center());
    let output = tall_frame(&ctx, &mut app);
    assert!(app.cloud_prototype.production.placement.is_any());
    // While it is on, the location choice cannot narrow the place again.
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert!(app.cloud_prototype.production.placement.is_any());
}

#[test]
fn a_new_cloud_starts_with_the_person_choosing_the_provider() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let workspace = app.board.create_workspace("another cloud");
    app.cloud_prototype.production.provider_mode =
        crate::app::cloud_panel::production::creation::any_provider::Mode::AnyProvider;
    app.open_workspace_cloud(&ctx, workspace);
    assert_eq!(
        app.cloud_prototype.production.provider_mode,
        crate::app::cloud_panel::production::creation::any_provider::Mode::Chosen
    );
}
