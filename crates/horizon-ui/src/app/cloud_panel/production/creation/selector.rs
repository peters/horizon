//! The wide worker selector for providers whose offers Horizon lists: three starting
//! points, every worker the repository profile allows, and where it runs. A choice edits
//! only this dialog's size and placement; nothing is rented until the cloud starts.
use super::{Production, placement};
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::{
    offers::{self, Offer, Picks, Place},
    prices::{Availability, Profile},
};
use std::time::Duration;

mod cards;
pub(super) mod summary;
pub(super) mod widgets;

/// How the catalog is being browsed; none of it is part of the cloud.
pub(in crate::app::cloud_panel::production) struct State {
    search: String,
    provider_filter: Option<String>,
    hours: f64,
    in_stock_only: bool,
    show_below_minimums: bool,
    /// The person checked "Start new cloud once available".
    pub wait_for_stock: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            search: String::new(),
            provider_filter: None,
            hours: 1.0,
            in_stock_only: true,
            show_below_minimums: false,
            wait_for_stock: false,
        }
    }
}

impl State {
    /// A search for one kind of worker means nothing for another profile's.
    pub(in crate::app::cloud_panel::production) fn profile_changed(&mut self) {
        *self = Self::default();
    }
}

/// A worker's stock where the cloud may go, and whether it is exact for that worker
/// rather than its CPU flavor family's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct Stock {
    pub level: Availability,
    pub exact: bool,
}

/// The workers a repository profile allows, from the latest catalog, current or not.
pub(in crate::app::cloud_panel::production) struct Catalog {
    pub offers: Vec<Offer>,
    /// Matching workers come first; remaining rows are for inspection only.
    pub matching: usize,
    /// Where each offer can run, in the same order.
    pub places: Vec<Vec<Place>>,
    pub picks: Picks,
    pub selected: Option<usize>,
    pub gpu: bool,
    pub complete: bool,
    pub currency: &'static str,
}

impl Catalog {
    pub fn total(&self, offer: &Offer, form: &Production) -> Option<f64> {
        offers::comparison::in_currency(
            offer.estimated_total,
            offer.currency,
            self.currency,
            form.prices.exchange.fresh(),
        )
    }
    /// Best stock among the places the placement allows; `None` where none is allowed.
    ///
    /// GPU stock is the type's own. A CPU offer's catalog stock is its flavor family's,
    /// so only the chosen size, once its exact check has answered, reads as exact.
    pub fn stock(&self, index: usize, form: &Production) -> Option<Stock> {
        if self.offers[index].location.is_none()
            && !self.gpu
            && self.selected == Some(index)
            && let Some(profile) = profile(form)
        {
            let sized = sized(form, profile);
            if let Some(Ok(size)) = form.prices.displayed_size(&sized, (sized.cpu, sized.memory_gb)) {
                let level = size.best(&form.placement.data_centers);
                return Some(Stock { level, exact: true });
            }
        }
        self.places[index]
            .iter()
            .filter(|place| {
                self.offers[index].location.is_some()
                    || profile(form).is_some_and(|profile| {
                        super::provider::current(form.provider, profile).label != self.offers[index].provider
                    })
                    || form.placement.is_any()
                    || form.placement.data_centers.contains(&place.id)
            })
            .map(|place| place.availability)
            .min()
            .map(|level| Stock { level, exact: self.gpu })
    }
}

/// The repository profile being launched, before a size is chosen for it.
pub(super) fn profile(form: &Production) -> Option<&Profile> {
    form.profiles.as_ref()?.profiles.get(&form.selected_profile)
}

pub(in crate::app::cloud_panel::production) fn catalog(form: &Production) -> Option<Catalog> {
    let profile = profile(form)?;
    let runpod = form
        .prices
        .list
        .as_ref()
        .filter(|_| form.prices.runpod_bound())
        .map(|fetched| &fetched.value);
    let hetzner = form
        .prices
        .hetzner
        .displayed()
        .and_then(|fetched| fetched.value.as_ref());
    if runpod.is_none() && hetzner.is_none() {
        return None;
    }
    let workers = offers::workers(profile, form.launch.selector.hours, runpod, hetzner);
    let offers::Workers {
        offers,
        matching,
        places,
    } = workers;
    let interested = |provider| interested(form, profile, provider);
    let runpod_interested = interested(&horizon_core::cloud_runtime::provider::RUNPOD) && form.prices.runpod_bound();
    let hetzner_interested = interested(&horizon_core::cloud_runtime::provider::HETZNER) && form.prices.hetzner.bound();
    let currency = comparison_currency(runpod_interested, hetzner_interested);
    let cost = |offer: &Offer| {
        offers::comparison::in_currency(
            offer.estimated_total,
            offer.currency,
            currency,
            form.prices.exchange.fresh(),
        )
    };
    let complete = (!runpod_interested || form.prices.fresh_list().is_some() && form.prices.list_error.is_none())
        && (!hetzner_interested || form.prices.hetzner.fresh().is_some() && form.prices.hetzner.error().is_none())
        && (!interested(&horizon_core::cloud_runtime::provider::HETZNER) || form.prices.hetzner.fresh().is_some())
        && offers
            .iter()
            .take(matching)
            .filter(|offer| {
                form.launch
                    .selector
                    .provider_filter
                    .as_ref()
                    .is_none_or(|filter| filter == offer.provider)
            })
            .all(|offer| cost(offer).is_some())
        && (profile.gpu
            || !runpod_interested
            || profile.storage.volume_tier == horizon_core::cloud_runtime::prices::StorageTier::Standard);
    let stocked: std::collections::HashSet<_> = offers
        .iter()
        .zip(&places)
        .filter(|(offer, places)| {
            places
                .iter()
                .any(|place| place.availability != Availability::None && place_allowed(offer, place, form, profile))
        })
        .map(|(offer, _)| (offer.provider, offer.id.as_str(), offer.location.as_deref()))
        .collect();
    let picks = if complete {
        offers::picks_matching(
            &offers[..matching],
            |offer| {
                form.launch
                    .selector
                    .provider_filter
                    .as_ref()
                    .is_none_or(|provider| provider == offer.provider)
            },
            |offer| {
                form.launch.selector.in_stock_only
                    && stocked.contains(&(offer.provider, offer.id.as_str(), offer.location.as_deref()))
            },
            |offer| cost(offer).unwrap_or(f64::INFINITY),
        )
    } else {
        Picks::default()
    };
    let selected = offers
        .iter()
        .take(matching)
        .position(|offer| is_selected(form, profile, offer));
    Some(Catalog {
        offers,
        matching,
        places,
        picks,
        selected,
        gpu: profile.gpu,
        complete,
        currency,
    })
}

fn interested(
    form: &Production,
    profile: &Profile,
    provider: &horizon_core::cloud_runtime::provider::Description,
) -> bool {
    provider.supports(profile)
        && form
            .launch
            .selector
            .provider_filter
            .as_ref()
            .is_none_or(|filter| filter == provider.label)
}

fn comparison_currency(runpod: bool, hetzner: bool) -> &'static str {
    if hetzner && !runpod {
        horizon_core::cloud_runtime::provider::HETZNER.currency
    } else {
        horizon_core::cloud_runtime::provider::RUNPOD.currency
    }
}

fn place_allowed(offer: &Offer, place: &Place, form: &Production, profile: &Profile) -> bool {
    offer.location.is_some()
        || super::provider::current(form.provider, profile).label != offer.provider
        || form.placement.is_any()
        || form.placement.data_centers.contains(&place.id)
}

fn is_selected(form: &Production, profile: &Profile, offer: &Offer) -> bool {
    let provider = super::provider::current(form.provider, profile);
    if provider.label != offer.provider {
        return false;
    }
    if offer.location.is_some() {
        return form.placement.cpu_types.first() == Some(&offer.id)
            && matches!(form.placement.data_centers.as_slice(), [location] if Some(location) == offer.location.as_ref());
    }
    if profile.gpu {
        form.placement.gpu_types.first() == Some(&offer.id)
    } else {
        let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
        offer.vcpu == Some(cpu) && offer.memory_gb == Some(memory_gb)
    }
}

/// Requests `offer` for the new cloud: its GPU type alone, or its CPU size.
pub(in crate::app::cloud_panel::production) fn choose(form: &mut Production, _gpu: bool, offer: &Offer) {
    let Some(profile) = profile(form) else {
        return;
    };
    let Ok(chosen) = horizon_core::cloud_panel::WorkerChoice::from(offer).for_profile(profile) else {
        return;
    };
    let keep_places = super::provider::current(form.provider, profile) == chosen.provider && offer.location.is_none();
    let old_places = std::mem::take(&mut form.placement);
    form.provider = Some(chosen.provider);
    form.size = chosen.size;
    form.placement = chosen.placement;
    if keep_places {
        form.placement.data_centers = old_places.data_centers;
        form.placement.region = old_places.region;
    }
}

/// The machine choice: what the repository needs, three starting points, every match,
/// and the data center.
pub(super) fn section(ui: &mut Ui, form: &mut Production) {
    let Some(profile) = profile(form).cloned() else {
        return;
    };
    widgets::heading(ui, "Machine", &requirement(&profile, &form.selected_profile));
    freshness(ui, form);
    ui.horizontal_wrapped(|ui| {
        ui.label("Compare for");
        ui.add(
            egui::DragValue::new(&mut form.launch.selector.hours)
                .range(1.0..=8760.0)
                .suffix(" hours"),
        );
        ui.selectable_value(&mut form.launch.selector.provider_filter, None, "All providers");
        for provider in super::provider::choices(&form.prices, &profile) {
            ui.selectable_value(
                &mut form.launch.selector.provider_filter,
                Some(provider.label.to_owned()),
                provider.label,
            );
        }
    });
    let Some(mut catalog) = catalog(form) else {
        return;
    };
    cards::filters(ui, &catalog, form);
    ui.add_space(4.0);
    if catalog.matching == 0 {
        widgets::note(ui, super::storage::empty_catalog_reason(form, &profile));
    }
    // A GPU profile always requests one explicit type, the cheapest to start with. A
    // type the person chose is never replaced, even once it is no longer offered.
    if (catalog.gpu && form.placement.gpu_types.is_empty()
        || form.provider.is_none()
            && form.size.is_none()
            && form.placement.is_default()
            && catalog.selected != catalog.picks.cheapest)
        && let Some(index) = catalog.picks.cheapest
    {
        choose(form, catalog.gpu, &catalog.offers[index]);
        catalog.selected = Some(index);
    }
    if catalog.gpu && catalog.selected.is_none() && !form.placement.gpu_types.is_empty() {
        widgets::note(
            ui,
            &format!(
                "{} is no longer offered here. Choose another GPU type.",
                form.placement.gpu_types.join(", ")
            ),
        );
    }
    if !catalog.complete {
        widgets::note(
            ui,
            "Comparison incomplete: waiting for current provider prices and exchange rates. Choose a worker explicitly.",
        );
    }
    if catalog.currency != horizon_core::cloud_runtime::provider::RUNPOD.currency {
        widgets::note(
            ui,
            &format!(
                "Estimated totals in {} · provider billing currency retained",
                catalog.currency
            ),
        );
    } else if let Some(rates) = form.prices.exchange.fresh() {
        widgets::note(
            ui,
            &format!(
                "Estimated totals in USD · ECB rates dated {} · provider billing currency retained",
                rates.date
            ),
        );
    }
    let mut chosen = cards::picks(ui, &catalog, form);
    ui.add_space(4.0);
    chosen = cards::all(ui, &catalog, form).or(chosen);
    if let Some(index) = chosen.filter(|&index| index < catalog.matching) {
        choose(form, catalog.gpu, &catalog.offers[index]);
    }
    if super::provider::current(form.provider, &profile).placement
        != horizon_core::cloud_runtime::provider::Placement::DataCenters
    {
        widgets::note(
            ui,
            "Hetzner availability is advisory. Choosing a row fixes that server type and location; no fallback is rented.",
        );
        return;
    }
    ui.add_space(10.0);
    widgets::heading(ui, "Data center", "");
    let sized = sized(form, &profile);
    if let Some(placement) = placement::field(ui, &form.prices, &sized, &form.placement) {
        form.placement = placement;
    }
}

/// The profile at the size chosen for it.
pub(super) fn sized(form: &Production, profile: &Profile) -> Profile {
    let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
    Profile {
        cpu,
        memory_gb,
        ..profile.clone()
    }
}

fn requirement(profile: &Profile, name: &str) -> String {
    if profile.gpu {
        match profile.min_gpu_memory_gb {
            Some(memory) => format!("GPU workers with at least {memory} GB GPU memory, for the {name} profile"),
            None => format!("GPU workers for the {name} profile"),
        }
    } else {
        format!(
            "Profile {name} requires at least {} vCPU and {} GB memory",
            profile.cpu, profile.memory_gb
        )
    }
}

/// How old the prices on show are, and why they are not current.
fn freshness(ui: &mut Ui, form: &mut Production) {
    let prices = &form.prices;
    if let Some(error) = prices.hetzner.error() {
        widgets::note(
            ui,
            &format!("Hetzner prices unavailable: {error}. Retaining the last catalog for inspection."),
        );
    }
    if let Some(error) = &prices.exchange.error {
        widgets::note(ui, error);
    }
    if prices.runpod_bound()
        && profile(form).is_some_and(|profile| {
            interested(form, profile, &horizon_core::cloud_runtime::provider::RUNPOD)
                && !profile.gpu
                && !profile.storage.standard_tier()
        })
    {
        widgets::note(
            ui,
            "High-performance storage prices are unpublished. Choose standard storage to compare complete totals.",
        );
    }
    let Some(list) = &prices.list else {
        if let Some(error) = &prices.list_error {
            widgets::note(ui, &format!("Prices are unavailable: {error}"));
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Fetching prices and stock…").color(theme::FG_SOFT()));
            });
        }
        if ui.small_button("Refresh").clicked() {
            form.prices.refresh();
        }
        return;
    };
    let age = list.at.elapsed();
    let (text, color) = if prices.too_old() {
        (
            format!(
                "Prices are {} old. Start needs current prices; refresh first.",
                ago(age)
            ),
            theme::PALETTE_RED(),
        )
    } else if let Some(error) = &prices.list_error {
        (
            format!("Could not refresh ({error}). Showing prices from {} ago.", ago(age)),
            theme::PALETTE_YELLOW(),
        )
    } else if prices.loading() {
        (format!("Updated {} ago · checking again…", ago(age)), theme::FG_DIM())
    } else {
        (format!("Updated {} ago", ago(age)), theme::FG_DIM())
    };
    let refresh = ui
        .horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
            ui.add_enabled(
                !prices.loading(),
                egui::Button::new(RichText::new("Refresh").size(12.0)).small(),
            )
            .clicked()
        })
        .inner;
    if refresh {
        form.prices.refresh();
    }
}

pub(super) fn ago(age: Duration) -> String {
    match age.as_secs() {
        0..60 => format!("{} s", age.as_secs()),
        60..3600 => format!("{} min", age.as_secs() / 60),
        seconds => format!("{} h", seconds / 3600),
    }
}

#[cfg(all(test, unix))]
mod tests;
