//! The wide worker selector for providers whose offers Horizon lists: three starting
//! points, every worker the repository profile allows, and where it runs. A choice edits
//! only this dialog's size and placement; nothing is rented until the cloud starts.
use super::{Production, placement};
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::{
    offers::{self, Offer, Picks, Place, Requirements},
    prices::{Availability, Profile},
};
use std::time::Duration;

mod cards;
pub(super) mod summary;
mod widgets;

/// How the catalog is being browsed; none of it is part of the cloud.
#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    show_all: bool,
    search: String,
    in_stock_only: bool,
    /// The person checked "Start new cloud once available".
    pub wait_for_stock: bool,
}

impl State {
    /// A search for one kind of worker means nothing for another profile's.
    pub(super) fn profile_changed(&mut self) {
        self.search.clear();
        self.wait_for_stock = false;
    }
}

/// A worker's stock where the cloud may go, and whether it is exact for that worker
/// rather than its CPU flavor family's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stock {
    pub level: Availability,
    pub exact: bool,
}

/// The workers a repository profile allows, from the latest catalog, current or not.
pub(super) struct Catalog {
    pub offers: Vec<Offer>,
    /// Where each offer can run, in the same order.
    pub places: Vec<Vec<Place>>,
    pub picks: Picks,
    pub selected: Option<usize>,
    pub gpu: bool,
}

impl Catalog {
    /// Best stock among the places the placement allows; `None` where none is allowed.
    ///
    /// GPU stock is the type's own. A CPU offer's catalog stock is its flavor family's,
    /// so only the chosen size, once its exact check has answered, reads as exact.
    pub fn stock(&self, index: usize, form: &Production) -> Option<Stock> {
        if !self.gpu
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
            .filter(|place| form.placement.is_any() || form.placement.data_centers.contains(&place.id))
            .map(|place| place.availability)
            .min()
            .map(|level| Stock { level, exact: self.gpu })
    }
}

/// The repository profile being launched, before a size is chosen for it.
pub(super) fn profile(form: &Production) -> Option<&Profile> {
    form.profiles.as_ref()?.profiles.get(&form.selected_profile)
}

pub(super) fn catalog(form: &Production) -> Option<Catalog> {
    let profile = profile(form)?;
    let (list, preferences) = &form.prices.list.as_ref()?.value;
    let tier = profile.storage.volume_tier;
    // CPU sizes request only flavors that hold the profile's container disk, as a
    // deployment of that size would.
    let offers = offers::catalog(
        list,
        preferences,
        &Requirements::for_profile(profile),
        (tier, profile.storage.container_gb),
    );
    let places: Vec<Vec<Place>> = offers.iter().map(|offer| offers::places(list, offer, tier)).collect();
    // Offer IDs are unique within one kind of worker.
    let stocked: std::collections::HashSet<&str> = offers
        .iter()
        .zip(&places)
        .filter(|(_, places)| {
            places.iter().any(|place| {
                place.availability != Availability::None
                    && (form.placement.is_any() || form.placement.data_centers.contains(&place.id))
            })
        })
        .map(|(offer, _)| offer.id.as_str())
        .collect();
    let picks = offers::picks(&offers, |offer| stocked.contains(offer.id.as_str()));
    let selected = offers.iter().position(|offer| is_selected(form, profile, offer));
    Some(Catalog {
        offers,
        places,
        picks,
        selected,
        gpu: profile.gpu,
    })
}

fn is_selected(form: &Production, profile: &Profile, offer: &Offer) -> bool {
    if profile.gpu {
        form.placement.gpu_types.first() == Some(&offer.id)
    } else {
        let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
        offer.vcpu == Some(cpu) && offer.memory_gb == Some(memory_gb)
    }
}

/// Requests `offer` for the new cloud: its GPU type alone, or its CPU size.
fn choose(form: &mut Production, gpu: bool, offer: &Offer) {
    if gpu {
        form.placement.gpu_types = vec![offer.id.clone()];
    } else if let (Some(cpu), Some(memory_gb)) = (offer.vcpu, offer.memory_gb) {
        form.size = Some((cpu, memory_gb));
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
    let Some(catalog) = catalog(form) else {
        return;
    };
    if catalog.offers.is_empty() {
        widgets::note(
            ui,
            "No worker the provider lists meets this profile's minimums in the allowed data centers.",
        );
        return;
    }
    // A GPU profile always requests one explicit type, the cheapest to start with. A
    // type the person chose is never replaced, even once it is no longer offered.
    if catalog.gpu && form.placement.gpu_types.is_empty() {
        if let Some(index) = catalog.picks.cheapest {
            choose(form, true, &catalog.offers[index]);
        }
        return;
    }
    if catalog.gpu && catalog.selected.is_none() {
        widgets::note(
            ui,
            &format!(
                "{} is no longer offered here. Choose another GPU type.",
                form.placement.gpu_types.join(", ")
            ),
        );
    }
    let mut chosen = cards::picks(ui, &catalog, form);
    ui.add_space(4.0);
    chosen = cards::all(ui, &catalog, form).or(chosen);
    if let Some(index) = chosen {
        choose(form, catalog.gpu, &catalog.offers[index]);
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
            "At least {} vCPU and {} GB memory, for the {name} profile",
            profile.cpu, profile.memory_gb
        )
    }
}

/// How old the prices on show are, and why they are not current.
fn freshness(ui: &mut Ui, form: &mut Production) {
    let prices = &form.prices;
    let Some(list) = &prices.list else {
        if let Some(error) = &prices.list_error {
            widgets::note(ui, &format!("Prices are unavailable: {error}"));
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Fetching prices and stock…").color(theme::FG_SOFT()));
            });
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
