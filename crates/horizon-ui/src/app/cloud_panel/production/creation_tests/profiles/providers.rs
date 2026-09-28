//! New cloud with more than one provider: the choice, and the fields each provider's
//! description shows or hides.
use super::*;

#[test]
fn hetzner_is_offered_beside_runpod_with_euro_prices_and_a_location_choice() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let output = tall_frame(&ctx, &mut app);
    assert!(
        !has_label(&output, "Provider"),
        "no provider choice without a Hetzner binding"
    );
    let catalog = serde_json::from_value(serde_json::json!({
        "offers": [
            {"server_type": "cx33", "location": "hel1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
             "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": false, "recommended": false},
            {"server_type": "cpx32", "location": "hel1", "cores": 4, "memory_gb": 8.0, "disk_gb": 160,
             "dedicated": false, "hourly_eur": 0.0569, "monthly_eur": 35.49, "available": true, "recommended": true},
            {"server_type": "cx33", "location": "nbg1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
             "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": true, "recommended": false},
            {"server_type": "cx23", "location": "nbg1", "cores": 2, "memory_gb": 4.0, "disk_gb": 40,
             "dedicated": false, "hourly_eur": 0.0088, "monthly_eur": 5.49, "available": true, "recommended": false}
        ],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5, "nbg1": 0.5},
        "ipv4_hour_eur": {"hel1": 0.0008, "nbg1": 0.0008}, "regions": {"hel1": "EUROPE", "nbg1": "EUROPE"},
    }))
    .unwrap();
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .answered_with_types(Some(catalog), &["cx23", "cx33", "cpx32"]);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Provider"));
    assert!(has_label(&output, "RunPod"), "the heading names the provider");
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    assert_eq!(
        app.cloud_prototype.production.provider.map(|provider| provider.id),
        Some("hetzner")
    );
    // The dialog grows to its new content on the next frame.
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Hetzner"), "the heading names the provider");
    assert!(
        has_label(&output, "8 GB"),
        "RunPod flavor families are not shown for Hetzner"
    );
    // cx23 is too small for 4 vCPU / 8 GB, so cx33 is the first configured type that fits.
    assert!(has_label(&output, "cx33 · 4 vCPU · 8 GB · €0.0136/h"));
    assert!(has_label(&output, "If it is sold out: cpx32 at €0.0569/h."));
    // 20 GB workspace volume by default: €1.14 kept while stopped. The cap is the shown
    // type's in the location deployment tries first, and "any" says another can cost more.
    assert!(has_label(
        &output,
        "On cx33 in hel1: at most €10.13 a month running, with the workspace volume and IPv4 address. €1.14 a month stopped: only the volume is kept."
    ));
    assert!(painted(&output).contains("Horizon tries this location first"));
    assert!(has_label(
        &output,
        "Hetzner lists this type as unavailable here; creation confirms whether it can be rented."
    ));
    assert!(
        !has_label(&output, "Region"),
        "RunPod regions are not shown for Hetzner"
    );
    // The provider choice itself names the other provider's billing; nothing else may.
    assert_absent(
        &output,
        &["$", "compute-optimized", "Secure Cloud", "Any region", "data center"],
        "US dollars",
    );
    click(
        &ctx,
        &mut app,
        label_rect(&output, "nbg1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["nbg1"]);
}

#[test]
fn a_hetzner_cloud_records_its_provider_and_location_and_switching_back_clears_it() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["hel1"]);
    // Switching back to RunPod clears the Hetzner location.
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "RunPod\nUS dollars · CPU and GPU").center(),
    );
    assert_eq!(
        app.cloud_prototype.production.provider.map(|provider| provider.id),
        Some("runpod")
    );
    assert!(app.cloud_prototype.production.placement.is_any());
}

#[test]
fn any_location_shows_the_offer_deployment_tries_first_in_the_settings_order() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let offer = |location: &str, hourly: f64| {
        serde_json::json!({"server_type": "cx33", "location": location, "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
            "dedicated": false, "hourly_eur": hourly, "monthly_eur": 8.49, "available": true, "recommended": false})
    };
    let catalog = serde_json::from_value(serde_json::json!({
        "offers": [offer("hel1", 0.0136), offer("nbg1", 0.02)],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5, "nbg1": 0.5},
        "ipv4_hour_eur": {"hel1": 0.0008, "nbg1": 0.0008}, "regions": {"hel1": "EUROPE", "nbg1": "EUROPE"},
    }))
    .unwrap();
    // The settings try nbg1 first, although hel1 is cheaper.
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .answered_with_policy(Some(catalog), &["cx33"], &["nbg1", "hel1"]);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "cx33 · 4 vCPU · 8 GB · €0.0200/h"));
    assert!(painted(&output).contains("On cx33 in nbg1"));
}

#[test]
fn a_hetzner_cloud_is_created_with_its_provider_and_location() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    super::finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "hetzner");
    assert_eq!(launch.placement.data_centers, ["hel1"]);
    assert_eq!(
        app.cloud_prototype
            .groups
            .0
            .last()
            .unwrap()
            .environment
            .provider
            .as_deref(),
        Some("hetzner")
    );
}

#[test]
fn hetzner_fields_stay_consistent_as_its_catalog_and_the_profile_change() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["hel1"]);
    // A refreshed catalog without the chosen location clears it, so creation never goes
    // somewhere other than the offer shown.
    let catalog = serde_json::from_value(serde_json::json!({
        "offers": [{"server_type": "cx33", "location": "nbg1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
            "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": true, "recommended": true}],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"nbg1": 0.5}, "ipv4_hour_eur": {"nbg1": 0.0008},
        "regions": {"nbg1": "EUROPE"},
    }))
    .unwrap();
    let production = &mut app.cloud_prototype.production;
    production.prices.hetzner.answered_with_types(Some(catalog), &["cx33"]);
    tall_frame(&ctx, &mut app);
    assert!(app.cloud_prototype.production.placement.is_any());
    // A size RunPod does not offer is Hetzner's to judge: its card shows the server type
    // that fits, and RunPod's size warning appears only once RunPod is chosen again.
    let production = &mut app.cloud_prototype.production;
    let profile = production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap();
    profile.cpu = 3;
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(
        has_label(&output, "cx33 · 4 vCPU · 8 GB · €0.0136/h"),
        "{}",
        painted(&output)
    );
    assert!(!painted(&output).contains("RunPod offers no CPU worker"));
    // Hetzner's sizes come from its configured server types, not RunPod's flavors: the
    // profile's own 3 vCPU and cx33's 4 vCPU, with 8 GB, and no RunPod-only size.
    for label in ["3 vCPU", "4 vCPU", "8 GB"] {
        assert!(has_label(&output, label), "{label}: {}", painted(&output));
    }
    assert!(!has_label(&output, "2 vCPU") && !has_label(&output, "16 vCPU"));
    click(
        &ctx,
        &mut app,
        label_rect(&output, "RunPod\nUS dollars · CPU and GPU").center(),
    );
    tall_frame(&ctx, &mut app);
    // RunPod offers larger sizes, and its own size warning names the 3 vCPU profile's.
    assert!(
        painted(&tall_frame(&ctx, &mut app))
            .contains("Choose a CPU and memory size that supports this container disk before starting.")
    );
    // A binding whose catalog could not be fetched still offers the choice, with the reason.
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .failed("Hetzner answered 503");
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(&output, "Hetzner prices unavailable: Hetzner answered 503"));
}

#[test]
fn a_launch_is_refused_when_the_chosen_provider_does_not_accept_the_profile() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let production = &app.cloud_prototype.production;
    let mut profile = production.profiles.as_ref().unwrap().profiles["development"].clone();
    // Hetzner accepts a 5,000 GB workspace volume; RunPod does not.
    profile.provider = "hetzner".to_owned();
    profile.storage.volume_gb = 5000;
    let refused = crate::app::cloud_panel::production::creation_job::launch_profile(
        &profile,
        Some(&horizon_core::cloud_runtime::provider::RUNPOD),
        None,
    );
    assert!(
        matches!(&refused, Err(error) if error.to_string().contains("cannot run on the chosen provider")),
        "{refused:?}"
    );
}

#[test]
fn a_hetzner_only_machine_offers_only_hetzner_and_never_moves_a_runpod_profile_on_its_own() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // As the price fetch finds it on a machine set up for Hetzner alone.
    app.cloud_prototype.production.prices.runpod_key_missing();
    hetzner_binding(&mut app);
    let groups = app.cloud_prototype.groups.0.len();
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    // The profile names RunPod, so the one usable provider is offered, not chosen.
    assert!(has_label(&output, "Provider"));
    assert!(painted(&output).contains("which this machine has no credentials for"));
    assert!(!has_label(&output, "RunPod\nUS dollars · CPU and GPU"));
    assert!(app.cloud_prototype.production.provider.is_none());
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    tall_frame(&ctx, &mut app);
    // Refused before anything is recorded or validated.
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        groups,
        "nothing is recorded for RunPod"
    );
    assert_eq!(
        app.cloud_prototype.error.take().as_deref(),
        Some(horizon_core::cloud_runtime::settings::RUNPOD_KEY_MISSING)
    );
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(!painted(&output).contains("which this machine has no credentials for"));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    super::finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "hetzner");
}

#[test]
fn a_runpod_cloud_waits_for_the_first_check_of_the_runpod_key() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // The dialog just opened: its first RunPod fetch has not answered.
    app.cloud_prototype.production.prices = super::super::super::prices::State::default();
    app.cloud_prototype.production.prices.runpod_checking();
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    tall_frame(&ctx, &mut app);
    assert!(app.cloud_prototype.production.launch.submitted, "the submission waits");
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    app.cloud_prototype.production.prices.runpod_answered();
    tall_frame(&ctx, &mut app);
    super::finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "runpod");
}

/// A Hetzner catalog as the running Horizon would have it with a binding.
fn hetzner_binding(app: &mut HorizonApp) {
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
}

/// Every painted text, joined, for checking that a provider's own terms never appear.
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

#[test]
fn runpod_shows_only_its_own_fields_and_hetzner_terms_never_appear() {
    use horizon_core::cloud_runtime::prices::{CpuFlavorPrice, DataCenter, PriceList, RUNPOD_STORAGE};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let list = PriceList {
        provider: "RunPod",
        cpu: vec![CpuFlavorPrice {
            id: "cpu3c".into(),
            name: "Compute-Optimized".into(),
            per_vcpu_hour: 0.03,
        }],
        gpus: Vec::new(),
        data_centers: vec![
            DataCenter {
                id: "EU-RO-1".into(),
                region: "EUROPE".into(),
                workspace_storage: true,
                high_performance_storage: false,
                cpus: Vec::new(),
                gpus: Vec::new(),
            },
            DataCenter {
                id: "US-MO-2".into(),
                region: "NORTH_AMERICA".into(),
                workspace_storage: true,
                high_performance_storage: false,
                cpus: Vec::new(),
                gpus: Vec::new(),
            },
        ],
        regions: std::collections::BTreeMap::new(),
        storage: RUNPOD_STORAGE,
    };
    let preferences = horizon_core::cloud_runtime::prices::Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    };
    let profile = app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["development"].clone();
    let stock = horizon_core::cloud_runtime::prices::SizeAvailability {
        centers: vec![(
            "EU-RO-1".into(),
            horizon_core::cloud_runtime::prices::Availability::High,
        )],
    };
    app.cloud_prototype
        .production
        .prices
        .answered(list, preferences, vec![(profile, stock)]);
    hetzner_binding(&mut app);
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    // Both providers are configured and support the CPU profile, so the choice appears,
    // with RunPod, the profile's own provider, selected.
    assert!(has_label(&output, "Provider"));
    assert!(has_label(&output, "RunPod"), "the heading names RunPod");
    assert!(has_label(&output, "Data center"), "RunPod's data center picker");
    assert!(
        painted(&output).contains("Compute-Optimized"),
        "RunPod's flavor families"
    );
    assert_absent(
        &output,
        &["Location", "€", "server type", "If it is sold out"],
        "euros, net of VAT",
    );
    // RunPod's data centers are offered only while RunPod is chosen.
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    assert_eq!(
        app.cloud_prototype.production.provider.map(|provider| provider.id),
        Some("hetzner")
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(&output, "Data center") && !has_label(&output, "EU-RO-1"));
}

#[test]
fn a_size_is_kept_by_the_chosen_providers_rules() {
    use crate::app::cloud_panel::production::creation::provider::sized;
    use horizon_core::cloud_runtime::provider::{HETZNER, RUNPOD};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let profile = app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["development"].clone();
    // 48 vCPU / 192 GB is a Hetzner server size but no RunPod flavor.
    let hetzner = sized(&HETZNER, &profile, Some((48, 192))).unwrap();
    assert_eq!(
        (hetzner.provider.as_str(), hetzner.cpu, hetzner.memory_gb),
        ("hetzner", 48, 192)
    );
    assert!(sized(&RUNPOD, &profile, Some((48, 192))).is_err());
}

#[test]
fn the_provider_choice_needs_two_providers_that_support_the_profile() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // Without a Hetzner binding only RunPod is configured.
    tall_frame(&ctx, &mut app);
    assert!(!has_label(&tall_frame(&ctx, &mut app), "Provider"));
    // A GPU profile runs only on RunPod, even with a Hetzner binding, and a Hetzner choice
    // made while the profile was CPU is dropped with its location.
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "hel1 · Europe\ncx33 · €0.0136/h").center(),
    );
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["hel1"]);
    let production = &mut app.cloud_prototype.production;
    let profile = production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap();
    profile.gpu = true;
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(&output, "Provider"));
    assert!(!has_label(&output, "Location"));
    let production = &app.cloud_prototype.production;
    assert!(production.provider.is_none() && production.placement.is_any());
}

/// Asserts that no painted line, other than those starting with `allowed`, contains any of `terms`.
fn assert_absent(output: &egui::FullOutput, terms: &[&str], allowed: &str) {
    let text = painted(output);
    for term in terms {
        let shown = text
            .lines()
            .filter(|line| !line.starts_with(allowed))
            .any(|line| line.contains(term));
        assert!(!shown, "{term} appears:\n{text}");
    }
}

#[test]
fn incompatible_hetzner_system_disk_blocks_launch_until_corrected() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let form = &mut app.cloud_prototype.production;
    form.provider = Some(&horizon_core::cloud_runtime::provider::HETZNER);
    form.placement.data_centers = vec!["hel1".into()];
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .container_gb = 81;
    let output = tall_frame(&ctx, &mut app);
    assert!(has_label(
        &output,
        "Choose a system disk and CPU size supported by a server in the selected location."
    ));
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    assert!(app.cloud_creation_open());
    assert!(!app.cloud_prototype.production.launch.submitted);
    let form = &mut app.cloud_prototype.production;
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .container_gb = 80;
    let output = tall_frame(&ctx, &mut app);
    assert!(!has_label(
        &output,
        "Choose a system disk and CPU size supported by a server in the selected location."
    ));
    assert!(app.cloud_prototype.production.placement.is_any());
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    assert!(!app.cloud_creation_open());
}

#[test]
fn a_failed_hetzner_refresh_keeps_its_last_catalog_on_show_with_the_reason() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Hetzner\neuros, net of VAT · CPU").center(),
    );
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .refresh_failed("Hetzner answered 503");
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(
        text.contains("Could not refresh Hetzner prices (Hetzner answered 503). Showing prices from "),
        "{text}"
    );
    assert!(has_label(&output, "cx33 · 4 vCPU · 8 GB · €0.0136/h"), "{text}");
    assert!(!text.contains("Hetzner prices unavailable"));
}
