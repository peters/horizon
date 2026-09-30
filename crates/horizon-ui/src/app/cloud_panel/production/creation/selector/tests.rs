use super::super::{Actions, can_submit, submit_reason, watch};
use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::{
    cloud_panel::{CloudConfig, Placement},
    cloud_runtime::prices::{
        Availability, CpuFlavorPrice, DataCenter, GpuPrice, Preferences, PriceList, RUNPOD_STORAGE, StorageTier,
    },
};
use std::time::Instant;

const CONFIG: &str = "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n  gpu:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n    min_gpu_memory_gb: 24\n";

fn center(id: &str, fast: bool, cpus: &[(&str, Availability)], gpus: &[(&str, Availability)]) -> DataCenter {
    DataCenter {
        id: id.into(),
        region: if id.starts_with("EU") {
            "EUROPE"
        } else {
            "NORTH_AMERICA"
        }
        .into(),
        workspace_storage: true,
        high_performance_storage: fast,
        gpus: gpus.iter().map(|&(id, level)| (id.into(), level)).collect(),
        cpus: cpus.iter().map(|&(id, level)| (id.into(), level)).collect(),
    }
}

fn list(a5000: Option<(Availability, f64)>) -> PriceList {
    let gpu = |id: &str, memory_gb, hourly| GpuPrice {
        id: id.into(),
        name: id.to_uppercase(),
        memory_gb,
        hourly,
    };
    let (a5000_stock, a5000_price) = a5000.unwrap_or((Availability::None, 0.27));
    PriceList {
        provider: "RunPod",
        cpu: vec![
            CpuFlavorPrice {
                id: "cpu3c".into(),
                name: "Compute-Optimized".into(),
                per_vcpu_hour: 0.03,
            },
            CpuFlavorPrice {
                id: "cpu3g".into(),
                name: "General Purpose".into(),
                per_vcpu_hour: 0.04,
            },
        ],
        gpus: vec![
            gpu("small", 16, 0.2),
            gpu("a5000", 24, a5000_price),
            gpu("a6000", 48, 0.49),
            gpu("h100", 80, 2.5),
        ],
        data_centers: vec![
            center(
                "EU-1",
                false,
                &[("cpu3c", Availability::High), ("cpu3g", Availability::Low)],
                &[("a5000", a5000_stock), ("a6000", Availability::High)],
            ),
            center(
                "US-1",
                true,
                &[("cpu3c", Availability::None)],
                &[("h100", Availability::Low), ("small", Availability::High)],
            ),
        ],
        regions: std::collections::BTreeMap::new(),
        storage: RUNPOD_STORAGE,
    }
}

fn preferences() -> Preferences {
    Preferences {
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: Vec::new(),
    }
}

fn form(profile: &str) -> Production {
    let mut form = Production {
        title: "Selector fixture".into(),
        profiles: Some(CloudConfig::parse(CONFIG).unwrap()),
        selected_profile: profile.into(),
        ..Production::default()
    };
    form.launch.accounts_checked = true;
    form.prices.answered(list(None), preferences(), Vec::new());
    form.prices.hetzner.answered(None);
    form
}

#[test]
fn refresh_keeps_browsing_choices_but_removes_complete_rankings_until_current() {
    let mut form = form("cpu");
    let before = catalog(&form).unwrap();
    assert!(before.complete && before.picks.cheapest.is_some());
    form.prices.refresh();
    let during = catalog(&form).unwrap();
    assert_eq!(during.offers, before.offers);
    assert!(!during.complete && during.picks.cheapest.is_none());
    form.prices.answered(list(None), preferences(), Vec::new());
    assert!(
        !catalog(&form).unwrap().complete,
        "Hetzner binding still needs rechecking"
    );
    form.prices.hetzner.answered(None);
    assert!(catalog(&form).unwrap().complete);
    if let Some(at) = Instant::now().checked_sub(std::time::Duration::from_hours(2)) {
        form.prices.list.as_mut().unwrap().at = at;
        assert!(!catalog(&form).unwrap().complete);
    }
}

#[test]
fn refreshing_rankings_does_not_move_filter_controls() {
    let positions = |form: &mut Production| {
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(1000.0);
                section(ui, form);
            })
            .discard_textures()
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::epaint::Shape::Text(text)
                    if matches!(
                        text.galley.job.text.as_str(),
                        "In stock only" | "Show workers below requirements"
                    ) =>
                {
                    Some((text.galley.job.text.clone(), text.pos))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut form = form("cpu");
    let before = positions(&mut form);
    assert_eq!(before.len(), 2);
    form.prices.refresh();
    assert_eq!(positions(&mut form), before);
}

/// Draws the selector and its summary once, returning every visible text.
fn render(form: &mut Production) -> Vec<String> {
    let mut actions = Actions::default();
    egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(1000.0);
            section(ui, form);
            summary::show(ui, form);
            summary::footer(ui, form, &mut actions);
        })
        .discard_textures()
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn provider_and_exchange_failures_only_appear_in_the_comparison_scope() {
    let failed = |profile| {
        let mut form = form(profile);
        form.prices.hetzner.failed("Synthetic Hetzner failure");
        form.prices.exchange.error = Some("Synthetic exchange failure".into());
        form.prices.list_error = Some("Synthetic RunPod failure".into());
        form
    };
    let contains = |labels: &[String], text: &str| labels.iter().any(|label| label.contains(text));
    let mut all = failed("cpu");
    let labels = render(&mut all);
    for error in [
        "Synthetic Hetzner failure",
        "Synthetic exchange failure",
        "Synthetic RunPod failure",
    ] {
        assert!(contains(&labels, error), "missing {error}");
    }
    for profile in ["cpu", "gpu"] {
        let mut runpod = failed(profile);
        runpod.launch.selector.provider_filter = Some("RunPod".into());
        let labels = render(&mut runpod);
        assert!(contains(&labels, "Synthetic RunPod failure"));
        assert!(!contains(&labels, "Synthetic Hetzner failure"));
        assert!(!contains(&labels, "Synthetic exchange failure"));
    }
    let labels = render(&mut failed("gpu"));
    assert!(!contains(&labels, "Synthetic Hetzner failure"));
    assert!(!contains(&labels, "Synthetic exchange failure"));
    let mut hetzner = failed("cpu");
    hetzner.launch.selector.provider_filter = Some("Hetzner".into());
    let labels = render(&mut hetzner);
    assert!(contains(&labels, "Synthetic Hetzner failure"));
    assert!(!contains(&labels, "Synthetic RunPod failure"));
    assert!(!contains(&labels, "Synthetic exchange failure"));
    assert!(labels.iter().any(|label| label == "Refresh"));
    let mut unconfigured = form("cpu");
    unconfigured.prices.exchange.error = Some("Synthetic exchange failure".into());
    assert!(!contains(&render(&mut unconfigured), "Synthetic exchange failure"));
}

#[test]
fn cpu_workers_below_the_profile_are_hidden_and_picks_span_the_rest() {
    let mut form = form("cpu");
    let shown = catalog(&form).unwrap();
    assert!(!shown.offers.is_empty());
    assert!(
        shown
            .offers
            .iter()
            .take(shown.matching)
            .all(|offer| offer.vcpu.unwrap() >= 4 && offer.memory_gb.unwrap() >= 8)
    );
    // The profile's own size is the cheapest and starts selected.
    let cheapest = &shown.offers[shown.picks.cheapest.unwrap()];
    assert_eq!((cheapest.vcpu, cheapest.memory_gb), (Some(4), Some(8)));
    assert_eq!(shown.selected, shown.picks.cheapest);
    let powerful = shown.offers[shown.picks.powerful.unwrap()].clone();
    assert!(powerful.vcpu.unwrap() > 4);
    choose(&mut form, false, &powerful);
    assert_eq!(form.size, Some((powerful.vcpu.unwrap(), powerful.memory_gb.unwrap())));
    let chosen = catalog(&form).unwrap();
    assert_eq!(chosen.offers[chosen.selected.unwrap()], powerful);
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "CHEAPEST"));
    assert!(
        labels
            .iter()
            .any(|label| label == "Profile cpu requires at least 4 vCPU and 8 GB memory")
    );
}

#[test]
fn filters_are_discoverable_and_do_not_change_the_selection() {
    let mut form = form("cpu");
    assert!(form.launch.selector.in_stock_only);
    assert!(!form.launch.selector.show_below_minimums);
    let shown = catalog(&form).unwrap();
    assert!(shown.matching < shown.offers.len());
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "In stock only"));
    assert!(labels.iter().any(|label| label == "Show workers below requirements"));
    assert!(
        labels
            .iter()
            .any(|label| label.starts_with("Showing ") && label.contains("below requirements hidden"))
    );
    assert!(labels.iter().any(|label| label == "EU-1"));
    assert!(labels.iter().any(|label| label == "US-1"));
    let before = (form.size, form.placement.clone());
    form.launch.selector.search = "No such worker".into();
    form.launch.selector.in_stock_only = false;
    form.launch.selector.show_below_minimums = true;
    form.launch.selector.wait_for_stock = true;
    let labels = render(&mut form);
    assert!(
        labels
            .iter()
            .any(|label| label.starts_with("No worker matches these filters."))
    );
    assert_eq!((form.size, form.placement.clone()), before);
    form.launch.selector.profile_changed();
    assert!(form.launch.selector.search.is_empty());
    assert!(form.launch.selector.in_stock_only && !form.launch.selector.show_below_minimums);
    assert!(!form.launch.selector.wait_for_stock);
}

#[test]
fn stock_filter_defaults_to_hiding_sold_out_workers_without_changing_the_selection() {
    let mut form = form("gpu");
    let _ = render(&mut form);
    let before = (form.size, form.placement.clone());
    let labels = render(&mut form);
    assert!(!labels.iter().any(|label| label == "A5000"));
    assert!(labels.iter().any(|label| label == "A6000"));
    form.launch.selector.in_stock_only = false;
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "A5000"));
    assert_eq!((form.size, form.placement.clone()), before);
}

#[test]
fn browsing_a_worker_below_requirements_never_selects_or_enables_it() {
    let mut form = form("cpu");
    form.launch.selector.show_below_minimums = true;
    form.launch.selector.search = "2 vCPU".into();
    let ctx = egui::Context::default();
    let frame = |form: &mut Production, events| {
        let catalog = catalog(form).unwrap();
        let mut chosen = None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    ..egui::RawInput::default()
                },
                |ui| {
                    ui.set_width(1000.0);
                    chosen = cards::all(ui, &catalog, form);
                },
            )
            .discard_textures();
        assert!(chosen.is_none());
        output
    };
    let output = frame(&mut form, Vec::new());
    let text = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "2 vCPU · 4 GB" => Some(text),
            _ => None,
        })
        .unwrap();
    let at = egui::Rect::from_min_size(text.pos, text.galley.size()).center();
    for pressed in [true, false] {
        let _ = frame(
            &mut form,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(form.size.is_none());
    form.size = Some((2, 4));
    assert!(catalog(&form).unwrap().selected.is_none());
    assert!(!can_submit(&form));
}

#[test]
fn an_empty_matching_catalog_still_exposes_filters_and_excluded_workers() {
    let mut form = form("gpu");
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("gpu")
        .unwrap()
        .min_gpu_memory_gb = Some(1000);
    let shown = catalog(&form).unwrap();
    assert_eq!(shown.matching, 0);
    assert!(!shown.offers.is_empty());
    form.launch.selector.show_below_minimums = true;
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "In stock only"));
    assert!(labels.iter().any(|label| label == "Below requirements"));
    assert!(form.placement.gpu_types.is_empty());
    assert!(!can_submit(&form));
}

#[test]
fn datacenter_stock_never_counts_a_preferred_gpu_below_the_profile_floor() {
    let mut form = form("gpu");
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("gpu")
        .unwrap()
        .min_gpu_memory_gb = Some(1000);
    let mut preferred = preferences();
    preferred.gpu_types = vec!["small".into()];
    form.prices.answered(list(None), preferred, Vec::new());
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label.contains("stock unknown")));
    assert!(!labels.iter().any(|label| label.contains("1 in stock")));
    assert!(!can_submit(&form));
}

#[test]
fn a_gpu_profile_requests_the_cheapest_in_stock_type_that_meets_its_memory_floor() {
    let mut form = form("gpu");
    let _ = render(&mut form);
    // "small" is cheaper and in stock, but below the 24 GB floor; A5000 is sold out.
    assert_eq!(form.placement.gpu_types, ["a6000"]);
    let shown = catalog(&form).unwrap();
    assert!(
        shown.offers[..shown.matching]
            .iter()
            .all(|offer| offer.gpu_memory_gb.unwrap() >= 24)
    );
    // Sold-out types stay listed.
    assert!(shown.offers.iter().any(|offer| offer.id == "a5000"));
}

#[test]
fn high_performance_storage_offers_only_data_centers_that_hold_it() {
    let mut form = form("cpu");
    let profiles = &mut form.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("cpu").unwrap().storage.volume_tier = StorageTier::HighPerformance;
    let shown = catalog(&form).unwrap();
    assert!(!shown.offers.is_empty());
    // Only US-1 holds the volume; offers whose flavor families it lacks have nowhere to go.
    assert!(shown.places.iter().flatten().all(|place| place.id == "US-1"));
    assert!(shown.places.iter().any(|places| !places.is_empty()));
    let labels = render(&mut form);
    assert!(
        labels
            .iter()
            .any(|label| label.starts_with("High-performance storage is priced per data center"))
    );
}

#[test]
fn the_wait_checkbox_appears_only_for_a_sold_out_selection() {
    let mut form = form("gpu");
    form.placement.gpu_types = vec!["a6000".into()];
    let labels = render(&mut form);
    assert!(!labels.iter().any(|label| label == "Start new cloud once available"));
    assert!(labels.iter().any(|label| label == "Start cloud"));
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: vec!["a5000".into()],
    };
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "Start new cloud once available"));
    form.launch.selector.wait_for_stock = true;
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label == "Start when available"));
    assert!(
        labels
            .iter()
            .any(|label| label == "Waits for this exact worker and data center, never another.")
    );
}

#[test]
fn a_watch_needs_one_data_center_and_never_starts_above_the_price_shown() {
    let mut form = form("gpu");
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into(), "EU-2".into()],
        gpu_types: vec!["a5000".into()],
    };
    assert!(watch::selection(&form).is_err(), "a region is never watched as a whole");
    form.placement.data_centers = vec!["EU-1".into()];
    watch::arm(&mut form);
    assert_eq!(form.launch.watch_quote, Some(0.27));
    watch::poll(&mut form);
    assert!(form.launch.watch.is_some() && !form.launch.submitted);
    // Stock returns at a higher price: it waits for a person.
    form.prices
        .answered(list(Some((Availability::High, 0.35))), preferences(), Vec::new());
    watch::poll(&mut form);
    assert_eq!(form.launch.price_rose, Some(0.35));
    assert!(form.launch.watch.is_some() && !form.launch.submitted);
    // At the price shown, it starts once.
    form.prices
        .answered(list(Some((Availability::Low, 0.27))), preferences(), Vec::new());
    watch::poll(&mut form);
    assert!(form.launch.watch.is_none() && form.launch.submitted);
    assert_eq!(form.launch.price_rose, None);
}

#[test]
fn prices_older_than_an_hour_cannot_start_a_cloud() {
    let mut form = form("cpu");
    assert!(can_submit(&form));
    let fetched = form.prices.list.as_mut().unwrap();
    fetched.at = Instant::now().checked_sub(std::time::Duration::from_hours(2)).unwrap();
    assert!(!can_submit(&form));
    assert_eq!(
        submit_reason(&form),
        Some("Prices are over an hour old. Refresh them before starting.")
    );
    let labels = render(&mut form);
    assert!(labels.iter().any(|label| label.starts_with("Prices are 2 h old.")));
}

#[test]
fn ages_read_in_the_largest_whole_unit() {
    use std::time::Duration;
    assert_eq!(ago(Duration::from_secs(9)), "9 s");
    assert_eq!(ago(Duration::from_secs(125)), "2 min");
    assert_eq!(ago(Duration::from_secs(7300)), "2 h");
}

#[test]
fn old_runpod_prices_never_hold_back_another_provider() {
    let mut form = form("cpu");
    let fetched = form.prices.list.as_mut().unwrap();
    fetched.at = Instant::now().checked_sub(std::time::Duration::from_hours(2)).unwrap();
    assert!(!can_submit(&form));
    form.provider = Some(&horizon_core::cloud_runtime::provider::HETZNER);
    assert_eq!(submit_reason(&form), Some("Choose a Hetzner worker and location."));
    assert!(!can_submit(&form));
}

#[test]
fn a_watch_never_arms_without_a_price_to_hold_it_to() {
    let mut form = form("gpu");
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: vec!["a5000".into()],
    };
    let mut unpriced = list(None);
    unpriced.gpus.retain(|gpu| gpu.id != "a5000");
    form.prices.answered(unpriced, preferences(), Vec::new());
    assert!(watch::selection(&form).is_ok());
    assert!(watch::armable(&form).is_err());
    watch::arm(&mut form);
    assert!(form.launch.watch.is_none());
    // A type the catalog no longer prices is not offered, so Start says to choose another.
    assert_eq!(
        submit_reason(&form),
        Some("Choose a GPU type the catalog offers for this profile.")
    );
}

#[test]
fn a_data_center_without_the_chosen_volume_blocks_start_instead_of_moving() {
    let mut form = form("cpu");
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: Vec::new(),
    };
    assert!(can_submit(&form));
    let profiles = &mut form.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("cpu").unwrap().storage.volume_tier = StorageTier::HighPerformance;
    assert!(!can_submit(&form));
    assert!(submit_reason(&form).is_some_and(|reason| reason.starts_with("The chosen data center cannot hold")));
    assert_eq!(
        form.placement.data_centers,
        ["EU-1"],
        "the place is never changed on its own"
    );
    form.placement.data_centers = vec!["US-1".into()];
    assert!(can_submit(&form));
}

#[test]
fn a_chosen_gpu_type_is_never_replaced_once_it_is_no_longer_offered() {
    let mut form = form("gpu");
    form.placement.gpu_types = vec!["retired".into()];
    let labels = render(&mut form);
    assert_eq!(form.placement.gpu_types, ["retired"]);
    assert!(
        labels
            .iter()
            .any(|label| label == "retired is no longer offered here. Choose another GPU type.")
    );
}

#[test]
fn only_stock_is_waited_for_and_a_new_profile_clears_the_search() {
    let mut form = form("cpu");
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: Vec::new(),
    };
    let profiles = &mut form.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("cpu").unwrap().storage.volume_tier = StorageTier::HighPerformance;
    let labels = render(&mut form);
    assert!(!labels.iter().any(|label| label == "Start new cloud once available"));
    form.launch.selector.search = "16 vCPU".into();
    form.launch.selector.wait_for_stock = true;
    form.launch.selector.profile_changed();
    assert!(form.launch.selector.search.is_empty() && !form.launch.selector.wait_for_stock);
}

#[test]
fn a_catalog_with_no_match_blocks_start_and_high_performance_totals_leave_the_volume_out() {
    let mut gpu = form("gpu");
    let profiles = &mut gpu.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("gpu").unwrap().min_gpu_memory_gb = Some(1000);
    assert!(!can_submit(&gpu));
    assert_eq!(
        submit_reason(&gpu),
        Some("No worker the provider lists meets this profile's minimums.")
    );
    let mut cpu = form("cpu");
    cpu.placement.data_centers = vec!["US-1".into()];
    let profiles = &mut cpu.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("cpu").unwrap().storage.volume_tier = StorageTier::HighPerformance;
    let labels = render(&mut cpu);
    assert!(labels.iter().any(|label| label == "Running all month, without it"));
    assert!(labels.iter().any(|label| label == "Price not published"));
}

#[test]
fn worker_cards_are_chosen_from_the_keyboard() {
    let mut form = form("cpu");
    let ctx = egui::Context::default();
    let key = |key: egui::Key, pressed: bool| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |form: &mut Production, events: Vec<egui::Event>| {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..egui::RawInput::default()
            },
            |ui| {
                ui.set_width(1000.0);
                section(ui, form);
            },
        )
        .discard_textures()
        .platform_output
        .events
    };
    let _ = frame(&mut form, Vec::new());
    // Tab through the dialog until a card other than the chosen one has focus.
    let mut target = None;
    for _ in 0..40 {
        let events = frame(&mut form, vec![key(egui::Key::Tab, true), key(egui::Key::Tab, false)]);
        target = events.into_iter().find_map(|event| match event {
            egui::output::OutputEvent::FocusGained(info) => info
                .label
                .filter(|label| label.contains(" vCPU · ") && !label.starts_with("4 vCPU · 8 GB")),
            _ => None,
        });
        if target.is_some() {
            break;
        }
    }
    let label = target.expect("a worker card takes keyboard focus");
    assert!(
        label.contains("RunPod"),
        "the accessible name identifies its provider: {label}"
    );
    let _ = frame(
        &mut form,
        vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
    );
    let size: Vec<u16> = label
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .take(2)
        .map(|part| part.parse().unwrap())
        .collect();
    assert_eq!(form.size, Some((size[0], size[1])), "{label}");
}

#[test]
fn a_cpu_size_the_catalog_no_longer_offers_blocks_start() {
    let mut form = form("cpu");
    assert!(can_submit(&form));
    // The preferred flavor loses its price, so the 4 vCPU / 8 GB size drops out.
    let mut unpriced = list(None);
    unpriced.cpu.retain(|flavor| flavor.id != "cpu3c");
    form.prices.answered(unpriced, preferences(), Vec::new());
    assert!(catalog(&form).unwrap().selected.is_none());
    assert!(!can_submit(&form));
    assert_eq!(
        submit_reason(&form),
        Some("Choose a CPU size the catalog offers for this profile.")
    );
}

#[test]
fn a_gpu_cloud_needs_one_chosen_type_and_an_empty_catalog_starts_nothing() {
    // Before any catalog arrives, a GPU cloud never falls back to the machine's preferences.
    let mut before = form("gpu");
    before.prices = crate::app::cloud_panel::production::prices::State::default();
    assert!(!can_submit(&before));
    assert_eq!(submit_reason(&before), Some("Choose a GPU type for this cloud."));
    // A catalog that lists nothing at all offers nothing to start.
    let mut empty = form("cpu");
    let mut nothing = list(None);
    nothing.cpu.clear();
    nothing.gpus.clear();
    empty.prices.answered(nothing, preferences(), Vec::new());
    assert!(!can_submit(&empty));
    assert_eq!(
        submit_reason(&empty),
        Some("No worker the provider lists meets this profile's minimums.")
    );
}

#[test]
fn only_the_chosen_cpu_size_reads_exact_stock_and_the_rest_say_likely() {
    use horizon_core::cloud_runtime::prices::SizeAvailability;
    let mut form = form("cpu");
    let profile = form.profiles.as_ref().unwrap().profiles["cpu"].clone();
    // The family is in stock in EU-1, but the exact 4 vCPU / 8 GB size is sold out.
    let sold_out = SizeAvailability {
        centers: vec![("EU-1".into(), Availability::None)],
    };
    form.prices
        .answered(list(None), preferences(), vec![(profile, sold_out)]);
    let shown = catalog(&form).unwrap();
    let chosen = shown.selected.unwrap();
    assert_eq!(
        shown.stock(chosen, &form),
        Some(Stock {
            level: Availability::None,
            exact: true
        })
    );
    let other = (0..shown.offers.len())
        .find(|&index| {
            index != chosen
                && shown
                    .stock(index, &form)
                    .is_some_and(|stock| stock.level == Availability::High)
        })
        .unwrap();
    assert_eq!(widgets::stock(shown.stock(other, &form)).0, "Likely in stock");
    assert_eq!(widgets::stock(shown.stock(chosen, &form)).0, "Out of stock");
    assert_ne!(shown.picks.cheapest, Some(chosen));
    for index in [shown.picks.cheapest, shown.picks.balanced, shown.picks.powerful]
        .into_iter()
        .flatten()
    {
        assert!(
            shown
                .stock(index, &form)
                .is_some_and(|stock| stock.level != Availability::None)
        );
    }
    form.launch.selector.in_stock_only = false;
    let unfiltered = catalog(&form).unwrap();
    assert_eq!(unfiltered.picks.cheapest, Some(chosen));
}

#[test]
fn stock_only_recommendations_are_empty_when_every_worker_is_unavailable() {
    let mut form = form("gpu");
    let mut sold_out = list(None);
    for center in &mut sold_out.data_centers {
        for (_, stock) in &mut center.gpus {
            *stock = Availability::None;
        }
    }
    form.prices.answered(sold_out, preferences(), Vec::new());
    let shown = catalog(&form).unwrap();
    assert!(shown.complete);
    assert_eq!(shown.picks, Picks::default());
    form.launch.selector.in_stock_only = false;
    let unfiltered = catalog(&form).unwrap();
    assert!(unfiltered.picks.cheapest.is_some());
}

#[test]
fn start_arms_the_watch_for_a_checked_sold_out_worker_whether_clicked_or_entered() {
    let mut form = form("gpu");
    form.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: vec!["a5000".into()],
    };
    form.launch.selector.wait_for_stock = true;
    assert!(summary::plan(&form).wait);
    summary::start(&mut form);
    assert!(
        form.launch.watch.is_some() && !form.launch.submitted,
        "it waits instead of starting"
    );
    // A worker in stock starts at once, checked box or not.
    let mut ready = self::form("gpu");
    ready.placement.gpu_types = vec!["a6000".into()];
    ready.launch.selector.wait_for_stock = true;
    summary::start(&mut ready);
    assert!(ready.launch.submitted && ready.launch.watch.is_none());
}

#[test]
fn a_vanished_data_center_or_price_ends_what_depends_on_it() {
    let mut gone = form("cpu");
    gone.placement.data_centers = vec!["GONE-1".into()];
    assert_eq!(
        submit_reason(&gone),
        Some("The chosen data center is no longer offered. Choose another data center.")
    );
    // A watched GPU the catalog stops pricing ends the watch instead of waiting forever.
    let mut watched = form("gpu");
    watched.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: vec!["a5000".into()],
    };
    watch::arm(&mut watched);
    assert!(watched.launch.watch.is_some());
    let mut unpriced = list(None);
    unpriced.gpus.retain(|gpu| gpu.id != "a5000");
    watched.prices.answered(unpriced, preferences(), Vec::new());
    watch::poll(&mut watched);
    assert!(watched.launch.watch.is_none() && !watched.launch.submitted);
    // Its summary still names the chosen type rather than CPU dimensions.
    let labels = render(&mut watched);
    assert!(labels.iter().any(|label| label == "a5000 (not offered)"));
}

#[test]
fn an_old_catalog_keeps_a_watch_waiting_and_a_volume_nobody_holds_is_named() {
    let mut watched = form("gpu");
    watched.placement = Placement {
        cpu_types: Vec::new(),
        region: Some("Europe".into()),
        data_centers: vec!["EU-1".into()],
        gpu_types: vec!["a5000".into()],
    };
    watch::arm(&mut watched);
    let fetched = watched.prices.list.as_mut().unwrap();
    fetched.at = Instant::now().checked_sub(std::time::Duration::from_hours(2)).unwrap();
    watch::poll(&mut watched);
    assert!(
        watched.launch.watch.is_some(),
        "an old catalog makes the watch wait, not end"
    );
    watched
        .prices
        .answered(list(Some((Availability::High, 0.27))), preferences(), Vec::new());
    watch::poll(&mut watched);
    assert!(watched.launch.submitted);
    // No allowed data center holds a high-performance volume for these flavors.
    let mut fast = form("cpu");
    let mut standard_only = list(None);
    for center in &mut standard_only.data_centers {
        center.high_performance_storage = false;
    }
    fast.prices.answered(standard_only, preferences(), Vec::new());
    let profiles = &mut fast.profiles.as_mut().unwrap().profiles;
    profiles.get_mut("cpu").unwrap().storage.volume_tier = StorageTier::HighPerformance;
    assert_eq!(
        submit_reason(&fast),
        Some("No allowed data center can hold this kind of workspace volume. Choose another storage type.")
    );
}

#[test]
fn retained_region_selection_requires_every_center_to_hold_the_storage_tier() {
    let mut form = form("cpu");
    let second = &mut form.prices.list.as_mut().unwrap().value.0.data_centers[1];
    second.id = "EU-2".into();
    second.region = "EUROPE".into();
    form.placement.data_centers = vec!["EU-1".into(), "EU-2".into()];
    form.placement.region = Some("Europe".into());
    assert!(can_submit(&form));
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("cpu")
        .unwrap()
        .storage
        .volume_tier = StorageTier::HighPerformance;
    assert!(!can_submit(&form));
    assert!(submit_reason(&form).unwrap().contains("cannot hold"));
    form.placement.data_centers = vec!["EU-2".into()];
    assert!(can_submit(&form));
    form.placement.data_centers.push("not-offered".into());
    assert!(!can_submit(&form));
    assert!(submit_reason(&form).unwrap().contains("no longer offered"));
}
