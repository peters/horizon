//! The saved Docker hosts as starting points beside the rented workers: no rent, and
//! what each has free now. Choosing one runs the cloud there at the profile's size.
use super::{Production, cards, widgets};
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_runtime::{docker_host::Capacity, prices::Profile, provider};

/// A saved host as New cloud offers it.
pub(super) struct Card {
    pub name: String,
    size: (u16, u16),
    capacity: Result<Capacity, String>,
    /// Why the host cannot run the profile, whatever its capacity.
    refusal: Option<&'static str>,
    /// The reading is older than its freshness limit and is being taken again.
    outdated: bool,
    selected: bool,
}

impl Card {
    fn fits(&self) -> bool {
        self.refusal.is_none()
            && !self.outdated
            && self
                .capacity
                .as_ref()
                .is_ok_and(|capacity| capacity.native() && capacity.fits(self.size.0, self.size.1))
    }
}

/// Why no Docker host can run `profile`, naming what to change, or `None` when one can.
pub(in crate::app::cloud_panel::production::creation) fn refusal(profile: &Profile) -> Option<&'static str> {
    if profile.gpu {
        Some("A Docker host runs CPU clouds only; this profile asks for a GPU.")
    } else if profile.build.is_some() {
        Some("This profile builds its image; a Docker host runs public images only.")
    } else if !profile.storage.standard_tier() {
        Some("This profile asks for high-performance storage; choose standard storage for a Docker host.")
    } else if profile.capabilities.browserstack.is_some() {
        Some("This profile has hosted devices; a Docker host has none.")
    } else {
        (!provider::DOCKER_HOST.supports(profile)).then_some("A Docker host cannot run this profile.")
    }
}

/// The saved hosts for `profile`; none for a GPU profile, which no host can run, and
/// each with the reason it cannot run the profile, if any.
pub(super) fn cards(form: &Production, profile: &Profile) -> Vec<Card> {
    if profile.gpu {
        return Vec::new();
    }
    let on_host = super::super::provider::current(form.provider, profile).kind == provider::Kind::DockerHost;
    let refusal = refusal(profile);
    form.prices
        .hosts
        .readings()
        .iter()
        .map(|reading| Card {
            name: reading.host.name.clone(),
            size: (profile.cpu, profile.memory_gb),
            capacity: reading.capacity.clone(),
            refusal,
            outdated: !form.prices.hosts.current(),
            selected: on_host && form.placement.data_centers == [reading.host.name.as_str()],
        })
        .collect()
}

pub(super) fn card(ui: &mut Ui, card: &Card) -> bool {
    let (word, color) = match (&card.capacity, card.refusal) {
        (_, Some(_)) => ("Cannot run this profile", theme::PALETTE_YELLOW()),
        (Err(_), None) => ("Not reachable", theme::PALETTE_RED()),
        (Ok(_), None) if card.outdated => ("Reading again…", theme::PALETTE_YELLOW()),
        (Ok(_), None) if card.fits() => ("Fits", theme::PALETTE_GREEN()),
        (Ok(capacity), None) if !capacity.native() => ("Architecture not supported", theme::PALETTE_RED()),
        (Ok(_), None) => ("Too little free now", theme::PALETTE_YELLOW()),
    };
    let free = match &card.capacity {
        Ok(capacity) => capacity.free(),
        Err(error) => error.clone(),
    };
    let size = format!("{} vCPU · {} GB", card.size.0, card.size.1);
    let label = format!("{size}, Docker host {}, no rent, {free}, {word}", card.name);
    let id = ui.id().with(("docker-host-card", &card.name));
    let response = ui.add_enabled_ui(card.fits() || card.selected, |ui| {
        cards::framed(ui, id, card.selected, &label, |ui| {
            ui.label(RichText::new(&size).size(15.0).strong().color(theme::FG()));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                super::super::provider::mark(ui, &provider::DOCKER_HOST, 13.0);
                let text = format!("Docker host · {}", card.name);
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(theme::FG_SOFT())).truncate());
            });
            ui.add_space(4.0);
            ui.label(RichText::new("No rent").size(19.0).strong().color(theme::FG()));
            ui.add(egui::Label::new(RichText::new(&free).size(12.0).color(theme::FG_SOFT())).wrap());
            ui.horizontal(|ui| widgets::pill(ui, word, color));
            // New cloud is a tooltip-order modal, so hover text would draw under it.
            if let Some(reason) = card.refusal {
                ui.add(egui::Label::new(RichText::new(reason).size(12.0).color(theme::FG_SOFT())).wrap());
            }
        })
    });
    response.inner
}

/// Runs the new cloud on the saved host `name`, at the profile's own size. No size is
/// kept for it, so a profile read again with other resources is checked and launched
/// at its new size.
pub(super) fn choose(form: &mut Production, profile: &Profile, name: &str) {
    let choice = horizon_core::cloud_panel::WorkerChoice {
        provider: provider::DOCKER_HOST.id.into(),
        id: name.into(),
        kind: "cpu".into(),
        vcpu: Some(profile.cpu),
        memory_gb: Some(profile.memory_gb),
        gpu_memory_gb: None,
        location: None,
    };
    if let Ok(chosen) = choice.for_profile(profile) {
        form.provider = Some(chosen.provider);
        form.size = None;
        form.placement = chosen.placement;
    }
}

/// The hosts alone, for a profile written for a Docker host, a machine that rents
/// nothing, or while rented prices are on their way. Unless rented workers may still
/// come, one that fits is chosen when the person chose none.
pub(super) fn only(ui: &mut Ui, form: &mut Production, profile: &Profile, hosts: &[Card]) {
    let auto =
        profile.provider == provider::DOCKER_HOST.id || !form.prices.runpod_bound() && !form.prices.hetzner.bound();
    if hosts.is_empty() {
        // Rented prices may still be on their way; only a host profile needs a host.
        if profile.provider == provider::DOCKER_HOST.id {
            widgets::note(
                ui,
                if form.prices.hosts.first_reading() {
                    "Reading your Docker hosts…"
                } else {
                    "Save a Docker host in Cloud settings to run this cloud on it."
                },
            );
        }
        return;
    }
    if let Some(cards::Picked::Host(name)) = cards::picks(ui, None, form, hosts) {
        choose(form, profile, &name);
    } else if auto
        && !host_chosen(form, profile)
        && let Some(host) = hosts.iter().find(|host| host.fits())
    {
        choose(form, profile, &host.name);
    }
}

/// Whether the person or New cloud already chose a host, also one that is no longer
/// offered: that choice stays until the person makes another, so Start refuses it rather
/// than run the cloud on a different machine.
fn host_chosen(form: &Production, profile: &Profile) -> bool {
    super::super::provider::current(form.provider, profile).kind == provider::Kind::DockerHost
        && !form.placement.data_centers.is_empty()
}

/// The summary of a cloud on a saved host: its size, the host and what stays there.
pub(in crate::app::cloud_panel::production) fn summary(ui: &mut Ui, form: &Production, profile: &Profile) {
    let sized = super::sized(form, profile);
    ui.label(
        RichText::new(format!("{} vCPU · {} GB", sized.cpu, sized.memory_gb))
            .size(20.0)
            .strong()
            .color(theme::FG()),
    );
    let host = form
        .placement
        .data_centers
        .first()
        .map_or("Choose a host", String::as_str);
    ui.label(
        RichText::new(format!("Docker host {host} · profile {}", form.selected_profile))
            .size(12.5)
            .color(theme::FG_SOFT()),
    );
    widgets::line(ui, "Compute", "No rent", true);
    widgets::line(ui, "Stopped cloud", "Volume kept", false);
    widgets::note(
        ui,
        "Runs on a machine you already have. Stopped, its container and volume stay on the host. Its CPUs and memory are shared and not reserved; the volume uses the host's disk.",
    );
}
