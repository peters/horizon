//! The provider a new cloud runs on, read from each provider's description, and the
//! offers of providers that place workers in named locations. A provider choice is shown
//! only when more than one configured provider supports the profile; nothing moves a
//! cloud between providers on its own.
use super::super::{Production, machine_size::Size, prices::State};
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

/// The providers the dialog compares, for the heading: every configured provider the
/// selected profile can use, or every configured provider before a profile is read.
/// With none configured, the provider the profile names, as the heading named it before.
pub(super) fn compared(form: &Production) -> Vec<&'static Description> {
    let profile = form
        .profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile));
    let providers = profile.map_or_else(|| configured(&form.prices), |profile| choices(&form.prices, profile));
    if providers.is_empty() {
        vec![profile.map_or(&provider::RUNPOD, |profile| current(form.provider, profile))]
    } else {
        providers
    }
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

/// Euros with cents, or four decimals below a cent.
pub(super) fn euros(value: f64) -> String {
    if value < 0.1 {
        format!("€{value:.4}")
    } else {
        format!("€{value:.2}")
    }
}
