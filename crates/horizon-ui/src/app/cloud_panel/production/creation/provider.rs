//! The provider a new CPU cloud runs on, and Hetzner's offers for it. Hetzner is offered
//! beside `RunPod` when this machine has a Hetzner binding; nothing moves a cloud between
//! providers on its own.
use super::{
    super::{
        Production,
        prices::{State, region_name},
    },
    pricing::option,
};
use crate::theme;
use egui::{Frame, Margin, RichText, Stroke, Ui};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::{offers::HETZNER_DEPLOYABLE, prices::Profile},
};

pub(in crate::app::cloud_panel) const RUNPOD: &str = "runpod";
pub(in crate::app::cloud_panel) const HETZNER: &str = "hetzner";

/// The provider a new cloud of `profile` uses: the one chosen in the dialog, or else the
/// profile's own.
pub(super) fn current(chosen: Option<&'static str>, profile: &Profile) -> &'static str {
    chosen.unwrap_or(if profile.provider == HETZNER { HETZNER } else { RUNPOD })
}

/// The provider's name for the dialog heading.
pub(super) fn label(form: &Production) -> &'static str {
    let profile = form
        .profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile));
    match profile.map(|profile| current(form.provider, profile)) {
        Some(HETZNER) => "Hetzner",
        _ => "RunPod",
    }
}

/// Whether to offer a provider choice for `profile`: CPU profiles, when this machine has a
/// Hetzner binding or the profile already names Hetzner.
pub(super) fn offered(prices: &State, profile: &Profile) -> bool {
    !profile.gpu
        && (profile.provider == HETZNER || prices.hetzner.fresh().is_some_and(|fetched| fetched.value.is_some()))
}

/// `RunPod` or Hetzner. Returns a newly chosen provider.
pub(super) fn choice(ui: &mut Ui, current: &str) -> Option<&'static str> {
    ui.label(RichText::new("Provider").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for (provider, label, detail) in [
            (RUNPOD, "RunPod", "US dollars · CPU and GPU"),
            (HETZNER, "Hetzner", "euros, net of VAT · CPU"),
        ] {
            if option(ui, label, current == provider, Some(detail), None) && current != provider {
                chosen = Some(provider);
            }
        }
    });
    chosen
}

/// One location's offer for `profile`: the first configured server type that fits it,
/// which is the one Horizon requests first there.
pub(super) struct LocationOffer {
    pub location: String,
    pub region: Option<String>,
    pub server_type: String,
    pub cores: u32,
    pub memory_gb: f64,
    pub hourly: f64,
    /// Compute, workspace volume and IPv4 address for a whole month of running.
    pub running_month: f64,
    /// The workspace volume alone, which a stopped cloud keeps.
    pub stopped_month: f64,
    /// Hetzner's advisory flag; an unlisted type can still be created.
    pub listed: bool,
    /// Further configured types that fit, tried in order when the first is sold out.
    pub fallbacks: Vec<String>,
}

/// Offers per location for `profile`, in the catalog's location order, from the server
/// types this machine's settings try. Empty without a catalog.
pub(super) fn location_offers(prices: &State, profile: &Profile) -> Vec<LocationOffer> {
    let hetzner = &prices.hetzner;
    let Some(catalog) = hetzner.fresh().and_then(|fetched| fetched.value.as_ref()) else {
        return Vec::new();
    };
    let stopped_month = catalog.volume_gb_month_eur * f64::from(profile.storage.volume_gb);
    let mut locations: Vec<&str> = catalog.offers.iter().map(|offer| offer.location.as_str()).collect();
    locations.sort_unstable();
    locations.dedup();
    locations
        .into_iter()
        .filter_map(|location| {
            let mut fitting = hetzner.server_types().iter().filter_map(|server_type| {
                catalog.offers.iter().find(|offer| {
                    offer.server_type == *server_type
                        && offer.location == location
                        && offer.cores >= u32::from(profile.cpu)
                        && offer.memory_gb >= f64::from(profile.memory_gb)
                        && offer.disk_gb >= u32::from(profile.storage.container_gb)
                })
            });
            let first = fitting.next()?;
            // Every server has an IPv4 address; a location that cannot price it is left
            // out rather than shown as cheaper than it is.
            let ipv4 = catalog.ipv4_month_eur.get(location)?;
            Some(LocationOffer {
                location: location.to_owned(),
                region: catalog.regions.get(location).map(|region| region_name(region)),
                server_type: first.server_type.clone(),
                cores: first.cores,
                memory_gb: first.memory_gb,
                hourly: first.hourly_eur,
                running_month: first.monthly_eur + stopped_month + ipv4,
                stopped_month,
                listed: first.available,
                fallbacks: fitting.map(|offer| offer.server_type.clone()).collect(),
            })
        })
        .collect()
}

/// Euros with cents, or four decimals below a cent.
pub(super) fn euros(value: f64) -> String {
    if value < 0.1 {
        format!("€{value:.4}")
    } else {
        format!("€{value:.2}")
    }
}

/// Hetzner's offers for `profile` and the location choice. Returns a newly chosen placement.
pub(super) fn card(ui: &mut Ui, prices: &State, profile: &Profile, placement: &Placement) -> Option<Placement> {
    let mut chosen = None;
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.06))
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let hetzner = &prices.hetzner;
            if let Some(error) = hetzner.error() {
                ui.colored_label(theme::PALETTE_RED(), format!("Hetzner prices unavailable: {error}"));
                return;
            }
            let Some(fetched) = hetzner.fresh() else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking Hetzner prices…");
                });
                return;
            };
            if fetched.value.is_none() {
                ui.label("Add Hetzner in Cloud settings to create clouds there.");
                return;
            }
            let offers = location_offers(prices, profile);
            if offers.is_empty() {
                ui.colored_label(
                    theme::PALETTE_RED(),
                    "No configured Hetzner server type has this size in an allowed location.",
                );
                return;
            }
            ui.label(RichText::new("Location").size(14.0).strong().color(theme::FG()));
            ui.horizontal_wrapped(|ui| {
                if option(ui, "Any allowed location", placement.data_centers.is_empty(), None, None) {
                    chosen = Some(Placement::default());
                }
                for offer in &offers {
                    let label = offer
                        .region
                        .as_deref()
                        .map_or_else(|| offer.location.clone(), |region| format!("{} · {region}", offer.location));
                    let detail = format!("{} · {}/h", offer.server_type, euros(offer.hourly));
                    let selected = placement.data_centers == [offer.location.clone()];
                    if option(ui, &label, selected, Some(&detail), None) && !selected {
                        chosen = Some(Placement {
                            data_centers: vec![offer.location.clone()],
                            ..Placement::default()
                        });
                    }
                }
            });
            // The cheapest location stands for "any"; a chosen one shows its own offer.
            let shown = offers
                .iter()
                .find(|offer| placement.data_centers == [offer.location.clone()])
                .or_else(|| offers.iter().min_by(|a, b| a.hourly.total_cmp(&b.hourly)));
            if let Some(offer) = shown {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{} · {} vCPU · {:.0} GB · {}/h",
                        offer.server_type,
                        offer.cores,
                        offer.memory_gb,
                        euros(offer.hourly)
                    ))
                    .size(15.0)
                    .strong()
                    .color(theme::FG()),
                );
                ui.label(format!(
                    "At most {} a month running, with the workspace volume and IPv4 address. {} a month stopped: only the volume is kept.",
                    euros(offer.running_month),
                    euros(offer.stopped_month)
                ));
                if !offer.fallbacks.is_empty() {
                    ui.small(format!("If it is sold out: {}.", offer.fallbacks.join(", ")));
                }
                if !offer.listed {
                    ui.small("Hetzner lists this type as unavailable here; creation confirms whether it can be rented.");
                }
            }
            ui.small("Billed per started hour, capped per calendar month. Prices are euros, net of VAT.");
            if !HETZNER_DEPLOYABLE {
                ui.colored_label(
                    theme::PALETTE_YELLOW(),
                    "Creating clouds on Hetzner arrives in a coming update; these prices are for comparison.",
                );
            }
        });
    chosen
}
