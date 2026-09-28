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
    form
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
fn cpu_workers_below_the_profile_are_hidden_and_picks_span_the_rest() {
    let mut form = form("cpu");
    let shown = catalog(&form).unwrap();
    assert!(!shown.offers.is_empty());
    assert!(
        shown
            .offers
            .iter()
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
            .any(|label| label == "At least 4 vCPU and 8 GB memory, for the cpu profile")
    );
}

#[test]
fn a_gpu_profile_requests_the_cheapest_in_stock_type_that_meets_its_memory_floor() {
    let mut form = form("gpu");
    let _ = render(&mut form);
    // "small" is cheaper and in stock, but below the 24 GB floor; A5000 is sold out.
    assert_eq!(form.placement.gpu_types, ["a6000"]);
    let shown = catalog(&form).unwrap();
    assert!(shown.offers.iter().all(|offer| offer.gpu_memory_gb.unwrap() >= 24));
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
