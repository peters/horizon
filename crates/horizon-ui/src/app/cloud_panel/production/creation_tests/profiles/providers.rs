//! Shared provider comparison, selection and launch persistence.
use super::*;
use crate::app::cloud_panel::production::creation::selector;
use horizon_core::cloud_runtime::offers::exchange::{OffsetDateTime, Rates};
use std::collections::BTreeMap;

fn catalog(available: bool) -> horizon_core::cloud_runtime::prices::HetznerCatalog {
    serde_json::from_value(serde_json::json!({
        "offers":[
          {"server_type":"cx33","location":"hel1","cores":4,"memory_gb":8.0,"disk_gb":80,
           "dedicated":false,"hourly_eur":0.0136,"monthly_eur":8.49,"available":available,"recommended":false},
          {"server_type":"cpx32","location":"hel1","cores":4,"memory_gb":8.0,"disk_gb":160,
           "dedicated":false,"hourly_eur":0.0569,"monthly_eur":35.49,"available":true,"recommended":true}],
        "volume_gb_month_eur":0.0572,"ipv4_month_eur":{"hel1":0.5},"ipv4_hour_eur":{"hel1":0.0008},
        "regions":{"hel1":"EUROPE"}
    }))
    .unwrap()
}

fn hetzner_binding(app: &mut HorizonApp) {
    app.cloud_prototype.production.prices.hetzner.answered_with_policy(
        Some(catalog(true)),
        &["cx33", "cpx32"],
        &["hel1"],
    );
    app.cloud_prototype
        .production
        .prices
        .exchange
        .answered(super::super::super::prices::Fetched {
            at: std::time::Instant::now(),
            value: Rates {
                date: OffsetDateTime::now_utc().date().to_string(),
                usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
            },
        });
}

fn selected(app: &HorizonApp) -> horizon_core::cloud_runtime::offers::Offer {
    let catalog = selector::catalog(&app.cloud_prototype.production).unwrap();
    catalog.offers[catalog.selected.unwrap()].clone()
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

#[test]
fn both_providers_are_listed_and_cheapest_chooses_an_exact_hetzner_worker() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(
        text.contains("All providers") && text.contains("RunPod") && text.contains("Hetzner"),
        "{text}"
    );
    assert!(text.contains("ECB rates dated") && text.contains("€"), "{text}");
    let catalog = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(catalog.complete);
    let offer = &catalog.offers[catalog.picks.cheapest.unwrap()];
    assert_eq!(
        (offer.provider, offer.id.as_str(), offer.location.as_deref()),
        ("Hetzner", "cx33", Some("hel1"))
    );
    click(&ctx, &mut app, label_rect(&output, "Hetzner · cx33 · hel1").center());
    tall_frame(&ctx, &mut app);
    assert_eq!(selected(&app).provider, "Hetzner");
    assert_eq!(app.cloud_prototype.production.placement.cpu_types, ["cx33"]);
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["hel1"]);
    assert!(painted(&tall_frame(&ctx, &mut app)).contains("no fallback is rented"));
}

#[test]
fn in_stock_only_keeps_unlisted_hetzner_workers_visible_and_eligible_for_picks() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.hetzner.answered_with_policy(
        Some(catalog(false)),
        &["cx33", "cpx32"],
        &["hel1"],
    );
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(
        text.contains("In stock only") && text.contains("Unlisted · advisory"),
        "{text}"
    );
    let catalog = selector::catalog(&app.cloud_prototype.production).unwrap();
    let cheapest = &catalog.offers[catalog.picks.cheapest.unwrap()];
    assert_eq!(
        (cheapest.provider, cheapest.id.as_str(), cheapest.availability),
        ("Hetzner", "cx33", "unlisted")
    );
    assert_eq!(selected(&app).id, "cx33");
    let shown = |text: &str| {
        text.lines()
            .find(|line| line.starts_with("Showing "))
            .map(str::to_owned)
            .unwrap()
    };
    let filtered = shown(&text);
    // Clearing In stock only reveals no Hetzner worker that the filter hid.
    click(&ctx, &mut app, label_rect(&output, "In stock only").center());
    tall_frame(&ctx, &mut app);
    assert_eq!(shown(&painted(&tall_frame(&ctx, &mut app))), filtered);
}

#[test]
fn launch_captures_type_location_and_provider_and_runpod_selection_clears_them() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    let runpod = shown
        .offers
        .iter()
        .take(shown.matching)
        .find(|offer| offer.provider == "RunPod")
        .unwrap();
    selector::choose(&mut app.cloud_prototype.production, false, runpod);
    assert!(app.cloud_prototype.production.placement.cpu_types.is_empty());
    assert!(app.cloud_prototype.production.placement.data_centers.is_empty());
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "hetzner");
    assert_eq!(launch.placement.cpu_types, ["cx33"]);
    assert_eq!(launch.placement.data_centers, ["hel1"]);
    let roundtrip: horizon_core::cloud_panel::Placement =
        serde_json::from_value(serde_json::to_value(&launch.placement).unwrap()).unwrap();
    assert_eq!(roundtrip, launch.placement);
}

#[test]
fn a_refresh_never_substitutes_a_different_worker_for_a_removed_choice() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    let previous = app.cloud_prototype.production.placement.clone();
    let mut removed = catalog(true);
    removed.offers.retain(|offer| offer.server_type != "cx33");
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .answered_with_types(Some(removed), &["cx33", "cpx32"]);
    let output = tall_frame(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.placement, previous);
    assert!(painted(&output).contains("selected worker is no longer offered"));
    assert!(!crate::app::cloud_panel::production::creation::can_submit(
        &app.cloud_prototype.production
    ));
}

#[test]
fn missing_or_stale_rates_keep_native_offers_without_a_cheapest_claim() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.exchange.refresh();
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(!shown.complete && shown.picks.cheapest.is_none());
    assert!(shown.offers.iter().any(|offer| offer.provider == "RunPod"));
    assert!(shown.offers.iter().any(|offer| offer.provider == "Hetzner"));
    assert!(painted(&tall_frame(&ctx, &mut app)).contains("Comparison incomplete"));
    hetzner_binding(&mut app);
    let exchange = &mut app.cloud_prototype.production.prices.exchange;
    let mut rates = exchange.fresh().unwrap().clone();
    rates.date = "2000-01-01".into();
    exchange.answered(super::super::super::prices::Fetched {
        value: rates,
        at: std::time::Instant::now(),
    });
    assert!(!selector::catalog(&app.cloud_prototype.production).unwrap().complete);
}

#[test]
fn an_expired_quote_blocks_global_ranking_but_keeps_native_provider_ranking() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let exchange = &mut app.cloud_prototype.production.prices.exchange;
    let rates = exchange.fresh().unwrap().clone();
    exchange.answered(super::super::super::prices::Fetched {
        value: rates,
        at: std::time::Instant::now()
            .checked_sub(std::time::Duration::from_hours(7))
            .unwrap(),
    });
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(!shown.complete && shown.picks.cheapest.is_none());
    let output = tall_frame(&ctx, &mut app);
    assert!(!painted(&output).contains("ECB rates dated"));
    click(&ctx, &mut app, label_rect(&output, "Hetzner").center());
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete && shown.picks.cheapest.is_some());
    assert_eq!(shown.currency, "EUR");
}

#[test]
fn a_hetzner_only_machine_selects_the_cheapest_matching_worker() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.runpod_key_missing();
    app.cloud_prototype.production.prices.exchange.refresh();
    tall_frame(&ctx, &mut app);
    tall_frame(&ctx, &mut app);
    assert_eq!(selected(&app).provider, "Hetzner");
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.offers.iter().all(|offer| offer.provider == "Hetzner"));
    assert!(shown.complete && shown.picks.cheapest.is_some());
    assert_eq!(shown.currency, "EUR");
}

#[test]
fn unsupported_providers_do_not_block_gpu_comparisons() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.hetzner.failed("unreachable");
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .gpu = true;
    tall_frame(&ctx, &mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete && shown.picks.cheapest.is_some());
    assert!(shown.offers.iter().all(|offer| offer.provider == "RunPod"));
}

#[test]
fn container_disk_minimums_disable_an_incompatible_chosen_server() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .container_gb = 81;
    tall_frame(&ctx, &mut app);
    assert!(!crate::app::cloud_panel::production::creation::can_submit(
        &app.cloud_prototype.production
    ));
    assert_eq!(app.cloud_prototype.production.placement.cpu_types, ["cx33"]);
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .container_gb = 80;
    tall_frame(&ctx, &mut app);
    assert!(crate::app::cloud_panel::production::creation::can_submit(
        &app.cloud_prototype.production
    ));
}

#[test]
fn failed_refresh_keeps_choices_visible_and_selection_unchanged() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    let previous = app.cloud_prototype.production.placement.clone();
    app.cloud_prototype
        .production
        .prices
        .hetzner
        .refresh_failed("unreachable");
    tall_frame(&ctx, &mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(!shown.complete && shown.offers.iter().any(|offer| offer.provider == "Hetzner"));
    assert_eq!(app.cloud_prototype.production.placement, previous);
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

#[test]
fn runpod_filter_can_rank_without_foreign_exchange_rates() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.exchange.refresh();
    let output = tall_frame(&ctx, &mut app);
    assert!(!selector::catalog(&app.cloud_prototype.production).unwrap().complete);
    let provider_button = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "RunPod" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .nth(1)
        .unwrap(); // The first RunPod label is the dialog heading.
    click(&ctx, &mut app, provider_button.center());
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete);
    assert_eq!(shown.offers[shown.picks.cheapest.unwrap()].provider, "RunPod");
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Hetzner").center());
    assert!(selector::catalog(&app.cloud_prototype.production).unwrap().complete);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.picks.cheapest.is_some());
    assert_eq!(shown.currency, "EUR");
    let output = tall_frame(&ctx, &mut app);
    assert!(painted(&output).contains("Estimated totals in EUR"));
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete);
    assert_eq!(shown.offers[shown.picks.cheapest.unwrap()].provider, "Hetzner");
}

#[test]
fn runpod_storage_requirements_are_not_silently_changed_for_hetzner() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let form = &mut app.cloud_prototype.production;
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .storage
        .volume_tier = horizon_core::cloud_runtime::prices::StorageTier::HighPerformance;
    let shown = selector::catalog(form).unwrap();
    assert!(!shown.complete);
    assert!(shown.offers.iter().all(|offer| offer.provider == "RunPod"));
    assert!(shown.picks.cheapest.is_none());
    assert_eq!(
        form.profiles.as_ref().unwrap().profiles["development"]
            .storage
            .volume_tier,
        horizon_core::cloud_runtime::prices::StorageTier::HighPerformance
    );
}

#[test]
fn exchange_status_follows_matching_currencies_even_with_both_providers_configured() {
    for only_hetzner_matches in [false, true] {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        prepare(&mut app, &ctx, temp.path());
        hetzner_binding(&mut app);
        let form = &mut app.cloud_prototype.production;
        if only_hetzner_matches {
            form.prices.list.as_mut().unwrap().value.0.cpu.clear();
        } else {
            form.profiles
                .as_mut()
                .unwrap()
                .profiles
                .get_mut("development")
                .unwrap()
                .cpu = 8;
        }
        let shown = selector::catalog(form).unwrap();
        assert!(shown.complete && shown.picks.cheapest.is_some());
        assert_eq!(shown.currency, if only_hetzner_matches { "EUR" } else { "USD" });
        assert!(!painted(&tall_frame(&ctx, &mut app)).contains("ECB rates dated"));
        app.cloud_prototype.production.prices.exchange.refresh();
        app.cloud_prototype.production.prices.exchange.error = Some("Synthetic exchange failure".into());
        let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
        assert!(shown.complete && shown.picks.cheapest.is_some());
        let text = painted(&tall_frame(&ctx, &mut app));
        assert!(
            !text.contains("Synthetic exchange failure") && !text.contains("ECB rates dated"),
            "{text}"
        );
    }
}

#[test]
fn matching_foreign_offers_still_report_exchange_failure() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    app.cloud_prototype.production.prices.exchange.refresh();
    app.cloud_prototype.production.prices.exchange.error = Some("Synthetic exchange failure".into());
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(!shown.complete && shown.picks.cheapest.is_none());
    assert!(painted(&tall_frame(&ctx, &mut app)).contains("Synthetic exchange failure"));
}

#[test]
fn provider_filter_counts_only_its_own_matching_and_excluded_workers() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    let filter = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Hetzner" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .nth(1)
        .unwrap(); // The first Hetzner label identifies the selected worker's provider.
    click(&ctx, &mut app, filter.center());
    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(
        text.contains("Showing 2 of 2 workers · 0 below requirements hidden"),
        "{text}"
    );
    assert!(!text.contains("Show workers below requirements"));
    let mut below_minimum = catalog(true);
    below_minimum.offers[0].cores = 2;
    app.cloud_prototype.production.prices.hetzner.answered_with_policy(
        Some(below_minimum),
        &["cx33", "cpx32"],
        &["hel1"],
    );
    let text = painted(&tall_frame(&ctx, &mut app));
    assert!(
        text.contains("Showing 1 of 2 workers · 1 below requirements hidden"),
        "{text}"
    );
    assert!(text.contains("Show workers below requirements"));
}

#[test]
fn hetzner_summary_owns_the_system_disk_editor_with_more_options_open() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    selector::choose(
        &mut app.cloud_prototype.production,
        false,
        &shown.offers[shown.picks.cheapest.unwrap()],
    );
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "More options").center());
    let text = painted(&tall_frame(&ctx, &mut app));
    assert!(text.contains("Committed base revision"), "{text}");
    assert_eq!(text.matches("System disk requirement").count(), 1, "{text}");
    assert!(!text.contains("Container disk"), "{text}");
}
