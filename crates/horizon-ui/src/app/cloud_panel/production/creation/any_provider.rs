//! "Any provider with this size": New cloud rents from whichever configured provider
//! ranks first for the chosen size, instead of the one the person picks. The ranking
//! is `horizon-cloud`'s; this module shows it and reports the provider it chose.
use super::super::prices::State;
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::{
    offers::{self, Candidate, HetznerSource, Sources},
    prices::Profile,
    provider::{Description, HETZNER, Kind},
};

/// Who picks the provider of a new cloud.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app::cloud_panel) enum Mode {
    /// The person, or the profile when there is nothing to choose.
    #[default]
    Chosen,
    /// Horizon: whichever configured provider ranks first for the chosen size.
    AnyProvider,
}

/// What the checkbox decides for this frame.
pub(super) enum Outcome {
    /// Unchecked, or not offered for this profile: the person picks the provider.
    Off,
    /// Checked, with no candidate yet while prices arrive or none has this size.
    Waiting,
    /// Checked: New cloud creates on this provider.
    Top(&'static Description),
}

/// The checkbox and, while it is checked, the order Horizon tries providers in for
/// `profile` at its chosen size. Offered only when more than one configured provider
/// can run the profile, as listed in `choices`.
pub(super) fn field(
    ui: &mut Ui,
    mode: &mut Mode,
    prices: &State,
    profile: &Profile,
    choices: &[&Description],
) -> Outcome {
    if choices.len() < 2 {
        return Outcome::Off;
    }
    let mut checked = *mode == Mode::AnyProvider;
    ui.checkbox(&mut checked, RichText::new("Any provider with this size").strong());
    *mode = if checked { Mode::AnyProvider } else { Mode::Chosen };
    if !checked {
        return Outcome::Off;
    }
    let note = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).size(12.0).color(theme::FG_SOFT()));
    // A provider whose prices have not arrived could rank first once they do.
    if pending(prices, choices) {
        note(ui, "Checking which providers have this size…");
        return Outcome::Waiting;
    }
    // No exchange rate is fetched yet, so prices in different currencies are not
    // compared.
    let ranked = offers::candidates(profile, &sources(prices), None);
    let Some(top) = ranked.first() else {
        note(
            ui,
            if prices.loading() || prices.hetzner.worth_waiting_for() {
                "Checking which providers have this size…"
            } else {
                "No configured provider has a worker of this size."
            },
        );
        return Outcome::Waiting;
    };
    ui.label(
        RichText::new(format!("Creates on {}", describe(top)))
            .size(13.0)
            .color(theme::FG()),
    );
    // Only the first is tried: a sold-out one does not move the cloud to the next yet.
    if ranked.len() > 1 {
        let others: Vec<String> = ranked[1..].iter().map(describe).collect();
        note(ui, &format!("Also has this size: {}.", others.join(", ")));
        note(
            ui,
            "Prices in different currencies are not compared yet, so this profile's own provider comes first.",
        );
    }
    Outcome::Top(top.provider)
}

/// Why New cloud cannot start yet while the checkbox is checked: a provider's prices
/// have not arrived, so the first one is not known, or no configured provider has a
/// worker of the chosen size.
pub(super) fn reason(form: &super::super::Production) -> Option<&'static str> {
    if form.provider_mode != Mode::AnyProvider {
        return None;
    }
    let profile = form.profiles.as_ref()?.profiles.get(&form.selected_profile)?;
    let choices = super::provider::choices(&form.prices, profile);
    if choices.len() < 2 {
        return None;
    }
    if pending(&form.prices, &choices) {
        return Some("Checking which providers have this size before starting.");
    }
    let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
    let sized = Profile {
        cpu,
        memory_gb,
        ..profile.clone()
    };
    offers::candidates(&sized, &sources(&form.prices), None)
        .is_empty()
        .then_some("No configured provider has a worker of this size yet. Choose another size, or clear Any provider.")
}

/// Whether a provider in `choices` has neither answered nor failed yet.
fn pending(prices: &State, choices: &[&Description]) -> bool {
    choices.iter().any(|provider| match provider.kind {
        Kind::RunPod => prices.runpod_pending(),
        Kind::Hetzner => prices.hetzner.latest().is_none() && prices.hetzner.error().is_none(),
    })
}

/// Each configured provider's current prices.
fn sources(prices: &State) -> Sources<'_> {
    let runpod = prices
        .runpod_bound()
        .then_some(prices.list.as_ref())
        .flatten()
        .map(|fetched| (&fetched.value.0, &fetched.value.1));
    let hetzner = &prices.hetzner;
    // The last catalog, so a refresh never changes the order while it runs.
    let catalog = hetzner
        .bound()
        .then(|| hetzner.latest())
        .flatten()
        .and_then(|fetched| fetched.value.as_ref());
    Sources {
        runpod,
        hetzner: catalog.map(|catalog| HetznerSource {
            catalog,
            server_types: hetzner.server_types(),
            locations: hetzner.locations(),
        }),
    }
}

/// One candidate as the dialog names it, such as "Hetzner cx53 in hel1 at €0.0473/h".
fn describe(candidate: &Candidate) -> String {
    let price = if candidate.currency == HETZNER.currency {
        super::provider::euros(candidate.hourly)
    } else {
        super::costs::money(candidate.hourly)
    };
    let place = candidate
        .location
        .as_deref()
        .map(|location| format!(" in {location}"))
        .unwrap_or_default();
    format!("{} {}{place} at {price}/h", candidate.provider.label, candidate.name)
}
