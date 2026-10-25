//! Azure as a provider choice in the New cloud heading, and its prices as informational
//! rows: the machine's `azure` settings decide the VM sizes and regions a cloud may use.
use super::providers::{painted, provider_filter, selected};
use super::*;
use crate::app::cloud_panel::production::creation::selector;

fn azure_catalog() -> horizon_core::cloud_runtime::prices::AzureCatalog {
    let offer = |size: &str, region: &str, vcpu: u16, memory_gb: u16, hourly: f64| serde_json::json!({"size": size, "region": region, "vcpu": vcpu, "memory_gb": memory_gb, "hourly_usd": hourly});
    let disks = serde_json::json!({"32": 5.28, "64": 10.21, "128": 19.71});
    serde_json::from_value(serde_json::json!({
        "offers": [offer("Standard_D2s_v3", "northeurope", 2, 8, 0.107),
            offer("Standard_D4s_v3", "northeurope", 4, 16, 0.214),
            offer("Standard_D8s_v3", "northeurope", 8, 32, 0.428),
            offer("Standard_D4s_v3", "westeurope", 4, 16, 0.2)],
        "disk_month_usd": {"northeurope": disks, "westeurope": disks},
        "static_ip_hour_usd": {"northeurope": 0.005, "westeurope": 0.005},
        "regions": {"northeurope": "EUROPE", "westeurope": "EUROPE"},
    }))
    .unwrap()
}

/// An `azure` section that allows two sizes in one region, and current Azure prices.
fn azure_binding(app: &mut HorizonApp) {
    app.cloud_prototype.production.prices.azure.answered_with_policy(
        azure_catalog(),
        &["Standard_D4s_v3", "Standard_D8s_v3"],
        &["northeurope"],
    );
}

#[test]
fn azure_is_offered_only_with_azure_settings_and_only_what_they_allow() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let text = painted(&tall_frame(&ctx, &mut app));
    assert_eq!(
        provider_labels(&tall_frame(&ctx, &mut app), "Azure"),
        1,
        "no azure section, one Azure entry in the heading and no filter: {text}"
    );
    assert!(!text.contains("Azure · Standard_"), "no Azure row: {text}");
    assert!(azure_offers_are_never_rentable(&app));

    app.cloud_prototype
        .production
        .prices
        .azure
        .unreadable("The Azure token file must hold one printable token");
    let text = painted(&tall_frame(&ctx, &mut app));
    assert!(
        text.contains("Azure workers are not offered: The Azure token file must hold one printable token"),
        "{text}"
    );
    assert!(
        !text.contains("Azure · Standard_"),
        "no row without a readable token: {text}"
    );

    azure_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, provider_filter(&output, "Azure").center());
    let text = painted(&tall_frame(&ctx, &mut app));
    assert!(text.contains("Azure · Standard_D4s_v3 · northeurope"), "{text}");
    assert!(text.contains("Azure · Standard_D8s_v3 · northeurope"), "{text}");
    assert!(text.contains("Quota checked at start"), "{text}");
    assert!(
        !text.contains("Standard_D2s_v3"),
        "a size the settings leave out: {text}"
    );
    assert!(!text.contains("westeurope"), "a region the settings leave out: {text}");
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete);
    assert_eq!(shown.currency, "USD");
}

#[test]
fn an_azure_row_is_priced_in_dollars_and_starts_on_its_size_and_region() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    azure_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, provider_filter(&output, "Azure").center());
    let output = tall_frame(&ctx, &mut app);
    click(
        &ctx,
        &mut app,
        label_rect(&output, "Azure · Standard_D4s_v3 · northeurope").center(),
    );
    let text = painted(&tall_frame(&ctx, &mut app));
    let form = &app.cloud_prototype.production;
    assert_eq!(form.provider, Some(&horizon_core::cloud_runtime::provider::AZURE));
    assert_eq!(form.placement.cpu_types, ["Standard_D4s_v3"]);
    assert_eq!(form.placement.data_centers, ["northeurope"]);
    assert_eq!(selected(&app).id, "Standard_D4s_v3");
    for shown in [
        "VM size",
        "Estimated run, disks + static address",
        "$0.21/hr",
        "Azure retail prices in US dollars, before tax",
        "OS disk",
        "no other is tried",
    ] {
        assert!(text.contains(shown), "{shown}: {text}");
    }
    assert!(!text.contains("€"), "{text}");
    assert!(!text.contains("Hetzner prices exclude VAT"), "{text}");
    let refused = "Azure needs an azure section with a readable token in the cloud settings.";
    assert!(!text.contains(refused), "{text}");

    // Settings that no longer have a readable token refuse Start before any work.
    app.cloud_prototype
        .production
        .prices
        .azure
        .unreadable("Credential bindings must use absolute machine-local paths");
    let text = painted(&tall_frame(&ctx, &mut app));
    assert!(text.contains(refused), "{text}");
    azure_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "azure");
    assert_eq!((launch.profile.cpu, launch.profile.memory_gb), (4, 16));
    assert_eq!(launch.placement.cpu_types, ["Standard_D4s_v3"]);
    assert_eq!(launch.placement.data_centers, ["northeurope"]);
}

/// How many visible labels read exactly `label`: the heading's provider entry and the
/// provider filter carry the same name.
fn provider_labels(output: &egui::FullOutput, label: &str) -> usize {
    output
        .shapes
        .iter()
        .filter(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == label))
        .count()
}

/// Whether every Azure row of the catalog is informational: the machine's `azure` settings
/// decide the VM sizes and regions, so no Azure price is a rentable row.
fn azure_offers_are_never_rentable(app: &HorizonApp) -> bool {
    selector::catalog(&app.cloud_prototype.production)
        .unwrap()
        .offers
        .iter()
        .filter(|offer| offer.provider == "Azure")
        .all(|offer| !offer.rentable)
}

/// The heading's Azure entry. The provider filter has the same name and is drawn later.
fn azure_entry(output: &egui::FullOutput) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Azure" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing Azure entry in the heading"))
}

#[test]
fn azure_is_a_provider_choice_that_says_what_it_needs_without_azure_settings() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, azure_entry(&output).center());
    let text = painted(&tall_frame(&ctx, &mut app));
    let form = &app.cloud_prototype.production;
    assert_eq!(
        form.provider.map(|provider| provider.id),
        Some("azure"),
        "the heading chooses the provider, not a row: {text}"
    );
    assert!(
        text.contains(
            "No Azure settings on this computer yet. Save Cloud settings once, then add an azure section to its settings.json with token_file, subscription_id, vm_sizes and regions."
        ),
        "{text}"
    );
    // No section, no prices: nothing is offered as a rentable Azure worker.
    assert!(!text.contains("Azure · Standard_"), "{text}");
    assert!(azure_offers_are_never_rentable(&app));
}

#[test]
fn the_heading_starts_an_azure_cloud_on_the_size_and_region_the_settings_allow() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    azure_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    assert_eq!(provider_labels(&output, "Azure"), 2, "the heading and the filter");
    click(&ctx, &mut app, azure_entry(&output).center());
    let text = painted(&tall_frame(&ctx, &mut app));
    let form = &app.cloud_prototype.production;
    assert_eq!(form.provider, Some(&horizon_core::cloud_runtime::provider::AZURE));
    // The cheapest worker the settings allow, as a profile written for Azure reaches it.
    assert_eq!(form.placement.cpu_types, ["Standard_D4s_v3"]);
    assert_eq!(form.placement.data_centers, ["northeurope"]);
    assert!(azure_offers_are_never_rentable(&app));
    assert!(!text.contains("No Azure settings on this computer yet"), "{text}");

    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "azure");
    assert_eq!(launch.placement.cpu_types, ["Standard_D4s_v3"]);
    assert_eq!(launch.placement.data_centers, ["northeurope"]);
}

#[test]
fn an_azure_choice_settles_on_the_settings_when_its_prices_arrive_later() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    // The person chooses Azure while the machine has read no prices yet.
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, azure_entry(&output).center());
    assert_eq!(
        app.cloud_prototype.production.provider,
        Some(&horizon_core::cloud_runtime::provider::AZURE)
    );
    assert!(app.cloud_prototype.production.placement.cpu_types.is_empty());

    // The settings section and its prices arrive after the choice.
    azure_binding(&mut app);
    let text = painted(&tall_frame(&ctx, &mut app));
    let form = &app.cloud_prototype.production;
    assert_eq!(
        form.placement.cpu_types,
        ["Standard_D4s_v3"],
        "the cheapest worker the settings allow: {text}"
    );
    assert_eq!(form.placement.data_centers, ["northeurope"], "{text}");
    assert!(!text.contains("Choose an Azure VM size and region."), "{text}");
    assert!(azure_offers_are_never_rentable(&app));

    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "azure");
    assert_eq!(launch.placement.cpu_types, ["Standard_D4s_v3"]);
    assert_eq!(launch.placement.data_centers, ["northeurope"]);
}
