//! Paste a `RunPod` key or a Hetzner token in the checks list, so a first cloud needs no visit
//! to the settings form. It saves what that form would, and makes the SSH key.
use crate::theme;
use egui::{Button, Id, Key, RichText, TextEdit, Ui, Vec2};
use horizon_core::cloud_runtime::setup::{Provider, save_provider_key};
use std::{
    path::Path,
    sync::mpsc::{Receiver, channel},
};
use zeroize::Zeroize;

#[derive(Default)]
pub(super) struct State {
    hetzner: bool,
    secret: String,
    saving: Option<Receiver<Result<(), String>>>,
    error: Option<String>,
}

impl Drop for State {
    /// A key typed here and never saved does not outlive the dialog.
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl State {
    /// The result of a save that has finished.
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let saved = self.saving.as_ref()?.try_recv().ok()?;
        self.saving = None;
        Some(saved)
    }

    /// Offers the tab for the provider the chosen profile runs on, unless a key is being typed.
    pub fn prefer(&mut self, hetzner: bool) {
        if self.secret.is_empty() {
            self.hetzner = hetzner;
        }
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
    }
}

pub(super) fn form(ui: &mut Ui, state: &mut State, root: &Path) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.hetzner, false, "RunPod");
        ui.selectable_value(&mut state.hetzner, true, "Hetzner");
    });
    let field = ui.add_sized(
        [ui.available_width(), 34.0],
        TextEdit::singleline(&mut state.secret)
            .id(Id::new("cloud-account-key"))
            .password(true)
            .margin(Vec2::new(12.0, 8.0))
            .hint_text(if state.hetzner {
                "Paste Hetzner API token"
            } else {
                "Paste RunPod API key"
            }),
    );
    let saving = state.saving.is_some();
    let ready = !saving && !state.secret.trim().is_empty();
    let go = ui
        .add_enabled(
            ready,
            Button::new(
                RichText::new(if saving { "Saving…" } else { "Save key" })
                    .size(13.5)
                    .strong()
                    .color(theme::BG()),
            )
            .fill(theme::ACCENT())
            .min_size(Vec2::new(0.0, 32.0))
            .corner_radius(8),
        )
        .clicked();
    if let Some(error) = &state.error {
        ui.label(RichText::new(error).size(12.5).color(theme::PALETTE_RED()));
    }
    ui.label(
        RichText::new("Stored privately on this computer. Change it later in Cloud settings.")
            .size(12.0)
            .color(theme::FG_DIM()),
    );
    if ready && (go || (field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))) {
        save(state, root, ui.ctx());
    }
}

fn save(state: &mut State, root: &Path, ctx: &egui::Context) {
    let provider = if state.hetzner {
        Provider::Hetzner
    } else {
        Provider::RunPod
    };
    let (root, ctx, mut key) = (root.to_owned(), ctx.clone(), std::mem::take(&mut state.secret));
    let (sender, receiver) = channel();
    state.error = None;
    state.saving = Some(receiver);
    std::thread::spawn(move || {
        let saved = save_provider_key(&root, provider, &key)
            .map(drop)
            .map_err(|error| error.to_string());
        key.zeroize();
        let _ = sender.send(saved);
        ctx.request_repaint();
    });
}
