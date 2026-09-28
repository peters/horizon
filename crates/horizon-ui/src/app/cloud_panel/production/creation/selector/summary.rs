//! The configuration summary beside the catalog: the chosen worker, where it runs, its
//! storage and cost, and the one action that starts it, now or once it is in stock.
use super::super::{Actions, Production, can_submit, costs, placement, provider, submit_reason, watch};
use super::{catalog, profile, sized, widgets};
use crate::theme;
use egui::{Button, DragValue, Frame, RichText, Stroke, Ui, Vec2};
use horizon_core::cloud_runtime::{
    prices::{self, Profile, StorageTier},
    provider::{Choice, Kind, Placement as ProviderPlacement},
};

pub(in super::super) fn show(ui: &mut Ui, form: &mut Production) {
    Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new("Summary").size(15.0).strong().color(theme::FG()));
            let editable = form.pending_creation.is_none() && !form.launch.submitted && form.launch.watch.is_none();
            if let Some(profile) = profile(form).cloned() {
                let provider = provider::current(form.provider, &profile);
                if provider.placement == ProviderPlacement::DataCenters {
                    worker(ui, form, &profile);
                    ui.add_enabled_ui(editable, |ui| storage(ui, form));
                    cost(ui, form);
                } else {
                    // Providers without listed offers price their own location card.
                    located(ui, form, &profile, provider.label);
                }
            } else {
                widgets::note(ui, "Read the repository profile to choose a worker.");
            }
            if form.launch.watch.is_some() {
                ui.add_space(8.0);
                watching(ui, form);
            }
        });
}

/// A worker whose provider places it by location, named in that provider's terms.
fn located(ui: &mut Ui, form: &Production, profile: &Profile, provider: &str) {
    let sized = sized(form, profile);
    ui.label(
        RichText::new(format!("{} vCPU · {} GB", sized.cpu, sized.memory_gb))
            .size(20.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new(format!("{provider} · profile {}", form.selected_profile))
            .size(12.5)
            .color(theme::FG_SOFT()),
    );
    let place = match form.placement.data_centers.as_slice() {
        [] => "Any location".to_owned(),
        places => places.join(", "),
    };
    ui.label(RichText::new(place).size(12.5).color(theme::FG_SOFT()));
    widgets::note(ui, "Prices and the server type are on the location card.");
}

fn worker(ui: &mut Ui, form: &Production, profile: &Profile) {
    let sized = sized(form, profile);
    let chosen = catalog(form).and_then(|catalog| {
        let index = catalog.selected?;
        Some(catalog.offers[index].clone())
    });
    let name = match &chosen {
        Some(offer) if offer.kind == "gpu" => offer.name.clone(),
        _ => format!("{} vCPU · {} GB", sized.cpu, sized.memory_gb),
    };
    ui.label(RichText::new(name).size(20.0).strong().color(theme::FG()));
    let kind = if profile.gpu { "GPU" } else { "CPU" };
    let detail = match &chosen {
        Some(offer) if offer.kind == "gpu" => format!(
            "{} GB GPU memory · {kind} profile {}",
            offer.gpu_memory_gb.unwrap_or(0),
            form.selected_profile
        ),
        Some(offer) => format!(
            "{} · {kind} profile {}",
            offer.name.split(" · ").next().unwrap_or_default(),
            form.selected_profile
        ),
        None => format!("{kind} profile {}", form.selected_profile),
    };
    ui.label(RichText::new(detail).size(12.5).color(theme::FG_SOFT()));
    // GPU stock is the catalog's level; a CPU size's is its own exact check.
    let level = catalog(form).and_then(|catalog| catalog.stock(catalog.selected?, form));
    let (stock, color) = match (profile.gpu, placement::in_stock(&form.prices, &sized, &form.placement)) {
        (true, _) if level.is_some() => widgets::stock(level),
        (_, Some(true)) => ("In stock", theme::PALETTE_GREEN()),
        (_, Some(false)) => ("Out of stock", theme::PALETTE_RED()),
        (_, None) => ("Checking stock", theme::FG_DIM()),
    };
    ui.horizontal(|ui| {
        widgets::pill(ui, stock, color);
        let place = match (form.placement.data_centers.as_slice(), form.placement.region.as_deref()) {
            ([], _) => "any data center".to_owned(),
            ([one], _) => one.clone(),
            (_, Some(region)) => format!("any in {region}"),
            (_, None) => "the chosen data centers".to_owned(),
        };
        ui.label(RichText::new(place).size(12.5).color(theme::FG_SOFT()));
    });
}

/// The workspace volume's type and size, edited on this dialog's copy of the profile.
fn storage(ui: &mut Ui, form: &mut Production) {
    let chosen = form.provider;
    let Some(profile) = form
        .profiles
        .as_mut()
        .and_then(|config| config.profiles.get_mut(&form.selected_profile))
    else {
        return;
    };
    let provider = provider::current(chosen, profile);
    widgets::caption(ui, "STORAGE");
    if provider.offers(Choice::VolumeTiers) && !profile.gpu {
        ui.horizontal(|ui| {
            for (tier, label) in [
                (StorageTier::Standard, "Standard"),
                (StorageTier::HighPerformance, "High-performance"),
            ] {
                let selected = profile.storage.volume_tier == tier;
                if ui
                    .add(
                        Button::new(RichText::new(label).size(12.5))
                            .selected(selected)
                            .corner_radius(8),
                    )
                    .clicked()
                {
                    profile.storage.volume_tier = tier;
                }
            }
        });
    }
    let (min, max) = if profile.gpu {
        (1, u32::from(u16::MAX))
    } else {
        provider.cpu_volume_gb
    };
    ui.horizontal(|ui| {
        let label = ui.label(
            RichText::new(if profile.gpu { "Pod volume" } else { "Workspace volume" })
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
        ui.add(
            DragValue::new(&mut profile.storage.volume_gb)
                .range(f64::from(min)..=f64::from(max))
                .suffix(" GB"),
        )
        .labelled_by(label.id);
    });
    let note = if profile.gpu {
        "Files survive a stop and are deleted with the pod."
    } else if profile.storage.standard_tier() {
        "Kept when the worker stops, in its data center."
    } else {
        "Faster disk for builds, offered in fewer data centers."
    };
    ui.label(RichText::new(note).size(11.5).color(theme::FG_DIM()));
}

fn cost(ui: &mut Ui, form: &Production) {
    let Some(profile) = profile(form) else {
        return;
    };
    let Some(fetched) = &form.prices.list else {
        return;
    };
    let (list, preferences) = &fetched.value;
    let sized = sized(form, profile);
    let hourly = if sized.gpu {
        let gpu = form.placement.gpu_types.first();
        gpu.and_then(|gpu| list.gpu(gpu)).map(|gpu| (gpu.hourly, gpu.hourly))
    } else {
        list.cpu_hourly(&prices::requested_flavors(&sized, preferences), sized.cpu)
    };
    widgets::caption(ui, "ESTIMATED COST");
    let Some((low, high)) = hourly else {
        widgets::note(ui, "Price unavailable for this worker.");
        return;
    };
    let storage = costs::storage(list, &sized);
    widgets::line(ui, "Compute", &format!("{}/hr", costs::range(low, high)), false);
    for item in &storage {
        let label = format!("{} · {} GB", item.kind, item.gb);
        if item.quoted {
            widgets::line(ui, &label, &format!("{}/mo", costs::money(item.running)), false);
        } else {
            // A long label and an unpublished price would collide on one row.
            ui.label(RichText::new(label).size(13.0).color(theme::FG_SOFT()));
            ui.label(RichText::new("Price not published").size(11.5).color(theme::FG_DIM()));
        }
    }
    match costs::monthly((low, high), &storage) {
        Some(((low, high), stopped)) => {
            widgets::line(
                ui,
                "Running all month",
                &format!("{}/mo", costs::range(low, high)),
                true,
            );
            widgets::line(ui, "Stopped", &format!("{}/mo", costs::money(stopped)), false);
        }
        None => widgets::note(
            ui,
            "High-performance storage is priced per data center and not published; the totals leave it out.",
        ),
    }
}

/// The running watch: what it waits for, how long, and at what price.
fn watching(ui: &mut Ui, form: &Production) {
    let Some(watch) = &form.launch.watch else {
        return;
    };
    let place = watch.placement.data_centers.join(", ");
    let quote = form
        .launch
        .watch_quote
        .map(|quote| format!(" at up to ${quote:.2}/hr"))
        .unwrap_or_default();
    widgets::status(
        ui,
        theme::PALETTE_YELLOW(),
        &format!("Waiting for stock in {place}"),
        &format!(
            "Starts this cloud once, automatically{quote}, when stock returns. Checks every 15 seconds while this dialog stays open; closing it or Stop watching ends the wait."
        ),
    );
    if let Some(now) = form.launch.price_rose {
        ui.colored_label(
            theme::PALETTE_RED(),
            format!("The price is now ${now:.2}/hr, above the limit, so it waits until the price falls back or you stop watching."),
        );
    }
}

/// The dialog's action bar: why Start is unavailable, the wait checkbox for a sold-out
/// selection, and Cancel beside Start, or Stop watching while a watch runs.
pub(in super::super) fn footer(ui: &mut Ui, form: &mut Production, actions: &mut Actions) {
    let starting = form.pending_creation.is_some() || form.launch.submitted;
    let watching = form.launch.watch.is_some();
    let sold_out = profile(form)
        .map(|profile| sized(form, profile))
        .is_some_and(|sized| placement::in_stock(&form.prices, &sized, &form.placement) == Some(false));
    // Only stock can be waited for: another reason Start is unavailable comes first.
    let watchable = sold_out
        && super::super::storage::launch_reason(form).is_none()
        && !starting
        && !watching
        && profile(form).is_some_and(|profile| provider::current(form.provider, profile).kind == Kind::RunPod);
    let wait = watchable && form.launch.selector.wait_for_stock;
    let enabled = can_submit(form) && !(wait && watch::armable(form).is_err());
    let reason = submit_reason(form);
    let watch_reason = if wait { watch::armable(form).err() } else { None };
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 330.0).max(160.0));
            if watchable {
                widgets::checkbox(
                    ui,
                    &mut form.launch.selector.wait_for_stock,
                    "Start new cloud once available",
                );
                let hint = watch_reason.unwrap_or(if wait {
                    "Waits for this exact worker and data center, never another."
                } else {
                    "Out of stock where this cloud may go. Starting now will likely fail."
                });
                ui.label(RichText::new(hint).size(11.5).color(theme::FG_DIM()));
            }
            if let Some(reason) = reason {
                ui.label(RichText::new(reason).size(13.0).color(theme::FG()));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if watching {
                if ui
                    .add(button(Button::new(RichText::new("Stop watching").size(14.0))))
                    .clicked()
                {
                    form.launch.watch = None;
                    form.launch.watch_quote = None;
                    form.launch.price_rose = None;
                }
            } else {
                let label = if starting {
                    "Starting cloud…"
                } else if form.launch.selector.wait_for_stock && watchable {
                    "Start when available"
                } else {
                    "Start cloud"
                };
                let primary =
                    Button::new(RichText::new(label).size(14.0).strong().color(theme::BG())).fill(theme::ACCENT());
                if ui.add_enabled(enabled, button(primary)).clicked() {
                    if form.launch.selector.wait_for_stock && watchable {
                        watch::arm(form);
                    } else {
                        actions.create = true;
                    }
                }
            }
            actions.cancel |= ui
                .add(button(Button::new(RichText::new("Cancel").size(14.0))))
                .clicked();
        });
    });
}

fn button(button: Button<'_>) -> Button<'_> {
    button.min_size(Vec2::new(120.0, 40.0)).corner_radius(10)
}
