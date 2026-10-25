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
fn a_large_profile_selects_a_catalog_worker_outside_hetzner_fallback_preferences() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    let form = &mut app.cloud_prototype.production;
    let profile = form.profiles.as_mut().unwrap().profiles.get_mut("development").unwrap();
    profile.cpu = 8;
    profile.memory_gb = 32;
    form.provider = None;
    form.size = None;
    form.placement = horizon_core::cloud_panel::Placement::default();
    let mut current = catalog(true);
    let mut larger = current.offers[0].clone();
    larger.server_type = "cx53".into();
    larger.cores = 16;
    larger.memory_gb = 32.0;
    larger.disk_gb = 320;
    larger.hourly_eur = 0.0412;
    larger.monthly_eur = 25.49;
    current.offers.push(larger);
    form.prices
        .hetzner
        .answered_with_policy(Some(current), &["cx43", "cx33", "cpx42"], &["hel1"]);

    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(text.contains("Profile development requires at least 8 vCPU and 32 GB memory"));
    assert!(text.contains("Hetzner · cx53 · hel1"), "{text}");
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete);
    let cheapest = &shown.offers[shown.picks.cheapest.unwrap()];
    assert_eq!(
        (cheapest.provider, cheapest.id.as_str(), cheapest.location.as_deref()),
        ("Hetzner", "cx53", Some("hel1"))
    );
    assert!(
        shown.offers[..shown.matching]
            .iter()
            .all(|offer| { offer.provider != "Hetzner" || offer.vcpu.unwrap() >= 8 && offer.memory_gb.unwrap() >= 32 })
    );
    click(&ctx, &mut app, label_rect(&output, "Hetzner · cx53 · hel1").center());
    let output = tall_frame(&ctx, &mut app);
    assert_eq!(selected(&app).id, "cx53");
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, "hetzner");
    assert_eq!((launch.profile.cpu, launch.profile.memory_gb), (16, 32));
    assert_eq!(launch.placement.cpu_types, ["cx53"]);
    assert_eq!(launch.placement.data_centers, ["hel1"]);
    let restored: horizon_core::cloud_panel::CloudLaunch =
        serde_json::from_slice(&serde_json::to_vec(launch).unwrap()).unwrap();
    assert_eq!(restored.placement, launch.placement);
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
fn the_worker_list_follows_the_converted_total_and_each_row_shows_it() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    tall_frame(&ctx, &mut app);
    let text = painted(&tall_frame(&ctx, &mut app));
    let form = &app.cloud_prototype.production;
    let catalog = selector::catalog(form).unwrap();
    let matching = &catalog.offers[..catalog.matching];
    let totals: Vec<f64> = matching
        .iter()
        .map(|offer| catalog.total(offer, form).unwrap())
        .collect();
    assert!(totals.is_sorted(), "{totals:?}");
    assert_eq!((matching[0].provider, matching[0].id.as_str()), ("Hetzner", "cx33"));
    // The list paints the cheapest worker first, with its converted total.
    let list = &text[text.find("Showing ").unwrap()..];
    let hetzner = list.find("Hetzner · cx33 · hel1").unwrap();
    assert!(list.find("RunPod · ").is_none_or(|runpod| hetzner < runpod), "{list}");
    assert!(list.contains(&format!("≈ ${:.3} total", totals[0])), "{list}");
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
    click(&ctx, &mut app, provider_filter(&output, "Hetzner").center());
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

fn saved_host(app: &mut HorizonApp, free_cpus: u64) {
    use horizon_core::cloud_runtime::docker_host::{Capacity, hosts::Reading};
    app.cloud_prototype.production.prices.hosts.answered(vec![Reading {
        host: serde_json::from_value(serde_json::json!({"name": "build-box", "access": "this-pc"})).unwrap(),
        capacity: Ok(Capacity {
            cpus: 16,
            memory_bytes: 64 << 30,
            free_nano_cpus: free_cpus * 1_000_000_000,
            free_memory_bytes: 48 << 30,
            architecture: "x86_64".into(),
            version: "28.5.1".into(),
        }),
    }]);
}

#[test]
fn a_saved_docker_host_is_a_starting_point_at_no_rent() {
    use crate::app::cloud_panel::production::creation::can_submit;
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    saved_host(&mut app, 12);
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    let text = painted(&output);
    assert!(
        text.contains("YOUR HOST") && text.contains("No rent") && text.contains("Fits"),
        "{text}"
    );
    // The cheapest rented worker stays the default; the host is one click away.
    assert_eq!(selected(&app).provider, "Hetzner");
    click(&ctx, &mut app, label_rect(&output, "Docker host · build-box").center());
    let output = tall_frame(&ctx, &mut app);
    let form = &app.cloud_prototype.production;
    assert_eq!(form.provider.map(|provider| provider.id), Some("docker-host"));
    assert_eq!(form.placement.data_centers, ["build-box"]);
    assert!(can_submit(form));
    let text = painted(&output);
    assert!(
        text.contains("Docker host build-box · profile") && !text.contains("Hetzner prices exclude VAT"),
        "{text}"
    );
    // A profile written for a host offers only hosts, and the host that fits is chosen.
    let form = &mut app.cloud_prototype.production;
    form.provider = None;
    form.placement = horizon_core::cloud_panel::Placement::default();
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
        .provider = "docker-host".into();
    let output = tall_frame(&ctx, &mut app);
    assert!(!painted(&output).contains("CHEAPEST"));
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["build-box"]);
    assert!(can_submit(&app.cloud_prototype.production));
    // A host with too little free is shown but never chosen.
    app.cloud_prototype.production.placement = horizon_core::cloud_panel::Placement::default();
    saved_host(&mut app, 1);
    let output = tall_frame(&ctx, &mut app);
    assert!(painted(&output).contains("Too little free now"));
    assert!(app.cloud_prototype.production.placement.data_centers.is_empty());
    assert!(!can_submit(&app.cloud_prototype.production));
}

fn development(app: &mut HorizonApp) -> &mut horizon_core::cloud_runtime::prices::Profile {
    app.cloud_prototype
        .production
        .profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("development")
        .unwrap()
}

#[test]
fn a_chosen_docker_host_stays_chosen_and_start_waits_for_a_current_reading() {
    use crate::app::cloud_panel::production::creation::can_submit;
    use horizon_core::cloud_runtime::docker_host::{Capacity, hosts::Reading};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    hetzner_binding(&mut app);
    saved_host(&mut app, 12);
    tall_frame(&ctx, &mut app);
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Docker host · build-box").center());
    tall_frame(&ctx, &mut app);
    assert!(can_submit(&app.cloud_prototype.production));
    // A host keeps no size of its own: a profile read again with more than the host has
    // free is checked at its new size.
    assert_eq!(app.cloud_prototype.production.size, None);
    let cpu = development(&mut app).cpu;
    development(&mut app).cpu = 16;
    tall_frame(&ctx, &mut app);
    assert!(!can_submit(&app.cloud_prototype.production));
    development(&mut app).cpu = cpu;
    tall_frame(&ctx, &mut app);
    assert!(can_submit(&app.cloud_prototype.production));
    // A profile a host cannot run says what to change on the card.
    development(&mut app).storage.volume_tier = horizon_core::cloud_runtime::prices::StorageTier::HighPerformance;
    let output = tall_frame(&ctx, &mut app);
    assert!(
        painted(&output).contains("choose standard storage"),
        "{}",
        painted(&output)
    );
    assert!(!can_submit(&app.cloud_prototype.production));
    development(&mut app).storage.volume_tier = horizon_core::cloud_runtime::prices::StorageTier::Standard;
    tall_frame(&ctx, &mut app);
    assert!(can_submit(&app.cloud_prototype.production));
    // A reading past its freshness limit keeps the choice, but Start waits for the next one.
    app.cloud_prototype.production.prices.hosts.outdated();
    let output = tall_frame(&ctx, &mut app);
    assert!(painted(&output).contains("Reading again"), "{}", painted(&output));
    assert!(!can_submit(&app.cloud_prototype.production));
    // Hosts removed or switched off: the choice stays and Start refuses it, never renting
    // a worker in its place.
    app.cloud_prototype.production.prices.hosts.answered(Vec::new());
    let output = tall_frame(&ctx, &mut app);
    assert!(painted(&output).contains("turn it on"), "{}", painted(&output));
    let form = &app.cloud_prototype.production;
    assert_eq!(form.provider.map(|provider| provider.id), Some("docker-host"));
    assert_eq!(form.placement.data_centers, ["build-box"]);
    assert!(!can_submit(form));
    // For a profile written for a host, another host does not replace the missing one.
    development(&mut app).provider = "docker-host".into();
    app.cloud_prototype.production.prices.hosts.answered(vec![Reading {
        host: serde_json::from_value(serde_json::json!({"name": "spare-box", "access": "this-pc"})).unwrap(),
        capacity: Ok(Capacity {
            cpus: 16,
            memory_bytes: 64 << 30,
            free_nano_cpus: 12_000_000_000,
            free_memory_bytes: 48 << 30,
            architecture: "x86_64".into(),
            version: "28.5.1".into(),
        }),
    }]);
    let output = tall_frame(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.placement.data_centers, ["build-box"]);
    assert!(!can_submit(&app.cloud_prototype.production));
    assert!(painted(&output).contains("no longer saved"), "{}", painted(&output));
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
    let provider_button = provider_filter(&output, "RunPod");
    click(&ctx, &mut app, provider_button.center());
    let shown = selector::catalog(&app.cloud_prototype.production).unwrap();
    assert!(shown.complete);
    assert_eq!(shown.offers[shown.picks.cheapest.unwrap()].provider, "RunPod");
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, provider_filter(&output, "Hetzner").center());
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
    let filter = provider_filter(&output, "Hetzner");
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
    assert!(!text.contains("Azure"), "no azure section, no Azure: {text}");
    assert!(
        selector::catalog(&app.cloud_prototype.production)
            .unwrap()
            .offers
            .iter()
            .all(|offer| offer.provider != "Azure")
    );

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

/// The provider filter button named `label`. The heading has a chip per provider with
/// the same name, so the filter is the last exact match.
fn provider_filter(output: &egui::FullOutput, label: &str) -> Rect {
    output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing provider filter: {label}"))
}
