//! The provider a new cloud runs on, read from each provider's description, and the
//! offers of providers that place workers in named locations. The heading lists a provider
//! the selected profile can use once this machine can use it, and one whose VM sizes and
//! regions come from a settings section even when it has none, with what to add. A click
//! moves this cloud to that provider; nothing moves a cloud between providers on its own.
use super::super::{Production, machine_size::Size, prices::State};
use super::selector;
use crate::theme;
use egui::{RichText, Stroke, Ui};
use horizon_core::cloud_runtime::{
    flavors,
    prices::Profile,
    provider::{self, Description},
};

/// The providers this machine can use: `RunPod` unless a fetch found no API key for it,
/// and each other provider once a fetch finds its binding or a saved host, including
/// while its catalog is refreshed or could not be fetched.
fn configured(prices: &State) -> Vec<&'static Description> {
    let mut configured = Vec::new();
    if prices.runpod_bound() {
        configured.push(&provider::RUNPOD);
    }
    if prices.hetzner.bound() {
        configured.push(&provider::HETZNER);
    }
    if prices.hosts.bound() {
        configured.push(&provider::DOCKER_HOST);
    }
    if prices.azure.bound() {
        configured.push(&provider::AZURE);
    }
    configured
}

/// The providers a new cloud of `profile` can choose from. A choice is shown only when
/// there is more than one.
pub(super) fn choices(prices: &State, profile: &Profile) -> Vec<&'static Description> {
    // A profile written for a Docker host runs on the saved host; New cloud does not offer
    // to move it to a rented provider.
    if profile.provider == provider::DOCKER_HOST.id {
        return vec![&provider::DOCKER_HOST];
    }
    Description::choices(profile, &configured(prices))
}

/// The provider a new cloud of `profile` uses: the one chosen in the dialog, or else the
/// profile's own.
pub(in crate::app::cloud_panel) fn current(
    chosen: Option<&'static Description>,
    profile: &Profile,
) -> &'static Description {
    chosen
        .or_else(|| provider::by_id(&profile.provider))
        .unwrap_or(&provider::RUNPOD)
}

/// `profile` on `provider` at `size`, or at its own size, by that provider's rules:
/// providers that price flavors need a flavor with the size, and others only the
/// profile's own rules, since server types are checked when the cloud is placed.
pub(in crate::app::cloud_panel) fn sized(
    provider: &Description,
    profile: &Profile,
    size: Option<Size>,
) -> horizon_core::cloud_runtime::Result<Profile> {
    let profile = Profile {
        provider: provider.id.to_owned(),
        ..profile.clone()
    };
    if provider.pricing == provider::Pricing::Flavors {
        return Ok(match size {
            Some(size) => flavors::sized(&profile, size)?,
            // A GPU profile specifies minimum host resources; CPU sizes must be offered.
            None if profile.gpu => profile,
            None => flavors::sized(&profile, (profile.cpu, profile.memory_gb))?,
        });
    }
    let (cpu, memory_gb) = size.unwrap_or((profile.cpu, profile.memory_gb));
    let sized = Profile {
        cpu,
        memory_gb,
        ..profile
    };
    sized
        .validate(false)
        .map_err(|_| horizon_core::cloud_runtime::Error::Invalid("This profile cannot run on the chosen provider"))?;
    Ok(sized)
}

/// The providers the dialog lists in its heading: every provider the selected profile can
/// use that this machine is set up for, or every one it is set up for before a profile is
/// read. A provider whose VM sizes and regions are a settings section is listed without
/// one, since no key form here makes it: the entry says what to add instead of vanishing.
/// With nothing to list, the provider the profile names, as the heading named it before.
pub(super) fn entries(form: &Production) -> Vec<Entry> {
    let prices = &form.prices;
    let profile = form
        .profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile));
    let current = profile.map(|profile| current(form.provider, profile));
    let ready = |provider: &'static Description| Entry {
        provider,
        unavailable: None,
        selected: current == Some(provider),
    };
    let mut entries: Vec<Entry> = match profile {
        Some(profile) => choices(prices, profile).into_iter().map(ready).collect(),
        None => configured(prices).into_iter().map(ready).collect(),
    };
    // A profile written for a Docker host runs on the saved host: no rented provider joins it.
    if let Some(profile) = profile.filter(|profile| profile.provider != provider::DOCKER_HOST.id) {
        for provider in provider::ALL
            .into_iter()
            .filter(|provider| provider.needs_settings && provider.supports(profile))
        {
            if entries.iter().all(|entry| entry.provider != provider) {
                entries.push(Entry {
                    provider,
                    unavailable: unavailable(prices, provider),
                    selected: current == Some(provider),
                });
            }
        }
    }
    if entries.is_empty() {
        entries.push(Entry {
            provider: current.unwrap_or(&provider::RUNPOD),
            unavailable: None,
            selected: true,
        });
    }
    entries
}

/// A provider the heading lists, and why this machine cannot start a cloud there yet.
pub(in crate::app::cloud_panel) struct Entry {
    pub provider: &'static Description,
    /// What to add before a cloud on it can start, when it cannot start here yet.
    pub unavailable: Option<String>,
    /// Whether this dialog's cloud runs there.
    pub selected: bool,
}

/// What a machine without Azure settings must add; no key form in the dialog can make them.
pub(in crate::app::cloud_panel) const AZURE_MISSING: &str = "No Azure settings on this computer yet. Save Cloud settings once, then add an azure section to its settings.json with token_file, subscription_id, vm_sizes and regions.";

/// Whether `chosen` stays this cloud's provider although no choice lists it: a saved Docker
/// host, which runs the profile with no reading to offer it, or a provider whose VM sizes
/// and regions are a settings section this machine has not read. Start refuses the cloud
/// and says what to add, so the choice is not silently moved to another provider.
pub(in crate::app::cloud_panel) fn keeps_its_choice(chosen: &Description, profile: &Profile) -> bool {
    chosen.kind == provider::Kind::DockerHost || chosen.needs_settings && chosen.supports(profile)
}

/// Why a cloud cannot start on `provider` here yet, in the words the checks use: a
/// provider whose placement is a settings section needs one this machine has read, and
/// says which part is missing or broken rather than always asking for settings.
fn unavailable(prices: &State, provider: &Description) -> Option<String> {
    match provider.kind {
        provider::Kind::Azure => {
            let azure = &prices.azure;
            if azure.bound() {
                // With a section, only its token or its prices can refuse the cloud.
                azure.problem().map(str::to_owned)
            } else if azure.absent() {
                Some(AZURE_MISSING.to_owned())
            } else {
                // Settings that could not be read, or that no read has answered for yet.
                azure.read_error().map(str::to_owned)
            }
        }
        provider::Kind::RunPod | provider::Kind::Hetzner | provider::Kind::DockerHost => None,
    }
}

/// Keeps a cloud that runs on a provider whose placement is a settings section on the
/// cheapest worker that section allows, while the person has chosen no row of their own.
/// Its prices can arrive after the provider was chosen, and the section decides the size
/// and region either way, so the dialog never waits for a choice it already has. A profile
/// written for that provider counts as the choice: it reaches the same placement, and its
/// row list can be missing while another provider's prices are unavailable.
pub(in crate::app::cloud_panel) fn settle(form: &mut Production) {
    let Some(profile) = selector::profile(form) else {
        return;
    };
    let provider = current(form.provider, profile);
    let waiting = provider.needs_settings
        && provider.supports(profile)
        && form.placement.cpu_types.is_empty()
        && form.placement.data_centers.is_empty();
    if waiting {
        choose(form, provider);
    }
}

/// Moves this dialog's cloud to `provider`: at the profile's own size and place, then onto
/// the cheapest worker that provider's catalog offers for it, as a profile written for that
/// provider starts. A provider with no worker to show leaves the size and place to it.
pub(in crate::app::cloud_panel) fn choose(form: &mut Production, provider: &'static Description) {
    form.provider = Some(provider);
    form.size = None;
    form.placement = horizon_core::cloud_panel::Placement::default();
    let Some(catalog) = selector::catalog(form) else {
        return;
    };
    let cheapest = catalog
        .offers
        .iter()
        .take(catalog.matching)
        .find(|offer| offer.provider == provider.label);
    if let Some(offer) = cheapest {
        selector::choose(form, catalog.gpu, offer);
    }
}

/// One provider in the heading: its mark and its name, drawn as a button. A provider this
/// machine cannot start a cloud on yet stays as a choice, dimmed, so that choosing it shows
/// what it needs. Returns whether the person clicked it.
pub(in crate::app::cloud_panel::production) fn choice(ui: &mut Ui, entry: &Entry) -> bool {
    let id = ui.scope_id().with(("cloud-provider-choice", entry.provider.id));
    let hovered = ui.ctx().read_response(id).is_some_and(|response| response.hovered());
    // The stroke never changes width, so that the heading keeps its height as the choice does.
    let (fill, stroke) = if entry.selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.14),
            Stroke::new(1.0, theme::ACCENT()),
        )
    } else if hovered {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::FG_DIM()))
    } else {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::BORDER_SUBTLE()))
    };
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let color = if entry.unavailable.is_some() {
                    theme::FG_DIM()
                } else {
                    theme::FG_SOFT()
                };
                mark(ui, entry.provider, 15.0);
                ui.label(RichText::new(entry.provider.label).size(13.0).color(color));
            });
        });
    let response = ui
        .interact(frame.response.rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let label = format!(
        "{}, {}",
        entry.provider.label,
        match (&entry.unavailable, entry.selected) {
            (Some(_), _) => "cannot start a cloud here yet",
            (None, true) => "this cloud's provider",
            (None, false) => "run this cloud there",
        }
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), entry.selected, &label));
    response.clicked()
}

/// A provider's mark, `size` points square: its logo where Horizon has one, else a
/// drawn mark, so listing a new provider needs one line here.
pub(in crate::app::cloud_panel::production) fn mark(ui: &mut egui::Ui, provider: &Description, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    match provider.kind {
        provider::Kind::RunPod => {
            if let Some(texture) = runpod_mark(ui.ctx()) {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(texture.id(), rect, uv, crate::theme::FG());
            }
        }
        provider::Kind::Hetzner => lettered(ui, rect, egui::Color32::from_rgb(0xD5, 0x0C, 0x2D), "H"),
        provider::Kind::DockerHost => {
            if let Some(texture) = docker_mark(ui.ctx()) {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(texture.id(), rect, uv, egui::Color32::WHITE);
            }
        }
        provider::Kind::Azure => lettered(ui, rect, egui::Color32::from_rgb(0x00, 0x78, 0xD4), "A"),
    }
}

/// A drawn mark: `letter` in white on a rounded square of `fill`.
fn lettered(ui: &egui::Ui, rect: egui::Rect, fill: egui::Color32, letter: &str) {
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(rect.height() * 0.72),
        egui::Color32::WHITE,
    );
}

/// The Docker whale in Docker blue; `assets/docker-mark.svg` names its source and license.
/// The PNG is decoded once on a worker thread, never on the UI thread; the mark shows
/// from the frame after.
pub(in crate::app::cloud_panel) fn docker_mark(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    // A process-wide cache: the pixels never change, and every window may draw the mark.
    static PIXELS: std::sync::OnceLock<Option<egui::ColorImage>> = std::sync::OnceLock::new();
    static DECODE: std::sync::Once = std::sync::Once::new();
    let id = egui::Id::unique("cloud-provider-mark-docker");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    DECODE.call_once(|| {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            PIXELS.get_or_init(|| {
                let png = include_bytes!("../../../../../assets/docker-mark.png");
                let icon = eframe::icon_data::from_png_bytes(png).ok()?;
                let size = [icon.width as usize, icon.height as usize];
                Some(egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba))
            });
            ctx.request_repaint();
        });
    });
    let image = PIXELS.get()?.clone()?;
    let texture = ctx.load_texture("cloud-provider-mark-docker", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    Some(texture)
}

/// The `RunPod` hexagon, cut from the wordmark Horizon already ships.
fn runpod_mark(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("cloud-provider-mark-runpod");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../../../assets/runpod-logo.png")).ok()?;
    let (width, height) = (icon.width as usize, icon.height as usize);
    // The hexagon fills a square at the start of the wordmark; the name follows it.
    let side = height.min(width);
    let pixels: Vec<u8> = (0..height)
        .flat_map(|row| icon.rgba[row * width * 4..(row * width + side) * 4].iter().copied())
        .collect();
    let image = egui::ColorImage::from_rgba_unmultiplied([side, height], &pixels);
    let texture = ctx.load_texture("cloud-provider-mark-runpod", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    Some(texture)
}
