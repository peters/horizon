//! The GitHub card of Cloud settings: connect this machine's own GitHub App, choose
//! its repositories, and choose how a new cloud gets its GitHub access.
use super::dashboard::{Tone, caption, header, label, surface};
use crate::{
    app::util::{chrome_button, primary_button},
    theme,
};
use egui::{RichText, vec2};
use horizon_core::cloud_runtime::{
    github::{self, Mode, connect},
    setup::{self, Draft},
};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// What the card waits for and what it last learned.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    connecting: Option<Receiver<Result<Connected, String>>>,
    checking: Option<Receiver<Option<bool>>>,
    /// Whether the app permits the device sign-in; `None` until checked.
    device_flow: Option<bool>,
    message: Option<String>,
}

struct Connected {
    settings: github::Settings,
    device_flow: Option<bool>,
}

fn open(url: &str) {
    if let Err(error) = horizon_core::open_url(url) {
        tracing::warn!(%error, "could not open a GitHub page");
    }
}

impl Card {
    /// Creates the app with the manifest flow, saves it, and checks its device sign-in.
    fn connect(&mut self, root: std::path::PathBuf) {
        let (tx, rx) = channel();
        self.connecting = Some(rx);
        self.message = None;
        std::thread::spawn(move || {
            let name = connect::app_name();
            let result = connect::start(&root, &name, horizon_core::open_url)
                .and_then(|created| created.recv().map_err(|_| horizon_core::cloud_runtime::Error::Busy)?)
                .and_then(|settings| {
                    setup::save_github(&root, Some(settings.clone()))?;
                    Ok(settings)
                })
                .map(|settings| {
                    open(&settings.installation_url());
                    let device_flow = connect::device_flow_enabled(&settings).ok();
                    Connected { settings, device_flow }
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
    }

    fn check(&mut self, settings: github::Settings) {
        let (tx, rx) = channel();
        self.checking = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(connect::device_flow_enabled(&settings).ok());
        });
    }

    fn poll(&mut self, ui: &egui::Ui, draft: &mut Draft) {
        if let Some(rx) = &self.connecting {
            match rx.try_recv() {
                Ok(Ok(connected)) => {
                    draft.settings.github = Some(connected.settings);
                    self.device_flow = connected.device_flow;
                    self.connecting = None;
                }
                Ok(Err(message)) => {
                    self.message = Some(message);
                    self.connecting = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.connecting = None,
            }
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(found) => {
                    self.device_flow = found;
                    self.checking = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.checking = None,
            }
        }
        if self.device_flow.is_none()
            && self.checking.is_none()
            && let Some(settings) = &draft.settings.github
        {
            self.check(settings.clone());
        }
    }
}

pub(super) fn card(ui: &mut egui::Ui, draft: &mut Draft, card: &mut Card) {
    card.poll(ui, draft);
    let status = match (&draft.settings.github, card.connecting.is_some()) {
        (_, true) => (Tone::Attention, "Waiting for GitHub"),
        (Some(_), false) => (Tone::Ready, "Connected"),
        (None, false) => (Tone::Idle, "Not connected"),
    };
    surface(ui, |ui| {
        header(ui, "GitHub", "Clone, push and open pull requests as you", Some(status));
        if card.connecting.is_some() {
            caption(
                ui,
                "Finish in your browser: click Create GitHub App, then choose the repositories.",
            );
        } else if let Some(settings) = draft.settings.github.clone() {
            connected(ui, draft, card, &settings);
        } else {
            caption(
                ui,
                "Horizon creates a private GitHub App that belongs to you. You choose which repositories \
                 it can use. Each cloud then signs in once and renews its own access.",
            );
            if ui
                .add(primary_button("Connect GitHub").min_size(vec2(140.0, 30.0)))
                .clicked()
            {
                card.connect(draft.root().to_owned());
            }
        }
        if let Some(message) = &card.message {
            ui.label(RichText::new(message).size(12.0).color(theme::PALETTE_RED()));
        }
    });
}

fn connected(ui: &mut egui::Ui, draft: &mut Draft, card: &mut Card, settings: &github::Settings) {
    ui.horizontal(|ui| {
        label(ui, &format!("App: {}", settings.slug));
        if ui.add(chrome_button("Choose repositories")).clicked() {
            open(&settings.installation_url());
        }
    });
    match card.device_flow {
        Some(false) => {
            ui.label(
                RichText::new("Turn on Enable Device Flow in the app's settings, once.")
                    .size(12.0)
                    .color(theme::PALETTE_YELLOW()),
            );
            ui.horizontal(|ui| {
                if ui.add(chrome_button("Open app settings")).clicked() {
                    open(&connect::settings_url(settings));
                }
                if ui.add(chrome_button("Check again")).clicked() {
                    card.device_flow = None;
                }
            });
        }
        Some(true) => caption(ui, "Device sign-in is on."),
        None => caption(ui, "Checking the app's settings…"),
    }
    ui.add_space(4.0);
    label(ui, "New clouds get GitHub access");
    let mode = draft.settings.github.as_ref().map_or(Mode::Ask, |github| github.mode);
    let mut chosen = mode;
    ui.radio_value(
        &mut chosen,
        Mode::Ask,
        "Ask me for each new cloud (one Authorize click)",
    );
    caption(ui, "The app secret never leaves this computer.");
    ui.radio_value(&mut chosen, Mode::Automatic, "Automatic (no clicks)");
    caption(
        ui,
        "Each cloud keeps a copy of the app secret, readable only by its system service.",
    );
    if chosen != mode
        && let Some(github) = &mut draft.settings.github
    {
        github.mode = chosen;
    }
    caption(
        ui,
        "Each cloud renews its own access for about 6 months, also while this computer is off.",
    );
    if ui.add(chrome_button("Disconnect")).clicked() {
        draft.settings.github = None;
        card.device_flow = None;
        card.message = Some(
            "Save settings to disconnect. New clouds then get no GitHub access. To end the access of running \
             clouds, delete the app on GitHub."
                .into(),
        );
    }
}
