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
    checking: Option<Receiver<Result<bool, String>>>,
    /// Whether the app permits the device sign-in, or why that could not be checked;
    /// `None` until checked. A failed check is retried only by Check again.
    device_flow: Option<Result<bool, String>>,
    message: Option<String>,
    /// Ends a Connect flow under way when the card goes, such as on Cancel.
    abort: Option<Abort>,
    /// A mode change or Disconnect being saved; the card saves them itself, so they need
    /// no provider set up and no Save settings.
    saving: Option<Receiver<Result<Saved, String>>>,
}

/// A GitHub setting the card saved: the file's bytes and what it now holds.
type Saved = (setup::Committed, Option<github::Settings>);

/// Cancels its flow when dropped.
struct Abort(horizon_core::cloud_runtime::Cancellation);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct Connected {
    settings: github::Settings,
    committed: setup::Committed,
    device_flow: Result<bool, String>,
}

fn open(url: &str) {
    if let Err(error) = horizon_core::open_url(url) {
        tracing::warn!(%error, "could not open a GitHub page");
    }
}

impl Card {
    /// Whether a Connect GitHub flow or a GitHub setting is being saved.
    pub(super) fn connecting(&self) -> bool {
        self.connecting.is_some() || self.saving.is_some()
    }

    /// Saves `shown`, the app this card shows, with a changed mode, or forgets it on
    /// Disconnect (`github` is `None`), off the UI thread. Either applies only while the
    /// settings still name `shown`; a Disconnect also ends this computer's sign-in for it.
    fn save(
        &mut self,
        ctx: &egui::Context,
        root: std::path::PathBuf,
        github: Option<github::Settings>,
        shown: github::Settings,
    ) {
        let (tx, rx) = channel();
        self.saving = Some(rx);
        self.message = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let save = || {
                setup::save_github(
                    &root,
                    Some(shown.app_id),
                    github.clone(),
                    &horizon_core::cloud_runtime::Cancellation::default(),
                )
            };
            // A Disconnect forgets this computer's sign-in and saves under one lock, so no
            // other Horizon window stores a chain for the app in between.
            let result = if github.is_none() {
                github::host::disconnect(&root, &shown, save)
            } else {
                save()
            }
            .map(|committed| (committed, github))
            .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Creates the app with the manifest flow, saves it, and checks its device sign-in.
    fn connect(&mut self, ctx: &egui::Context, root: std::path::PathBuf) {
        let (tx, rx) = channel();
        self.connecting = Some(rx);
        self.message = None;
        let cancel = horizon_core::cloud_runtime::Cancellation::default();
        self.abort = Some(Abort(cancel.clone()));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let name = connect::app_name();
            let result = connect::start(&root, &name, horizon_core::open_url, cancel.clone())
                .and_then(|created| created.recv().map_err(|_| horizon_core::cloud_runtime::Error::Busy)?)
                // Settings closed meanwhile, or another window connected an app: the save,
                // which checks under the settings lock, saves nothing, and the app's secret goes.
                .and_then(
                    |settings| match setup::save_github(&root, None, Some(settings.clone()), &cancel) {
                        Ok(committed) => Ok((committed, settings)),
                        Err(error) => {
                            // Unless the settings file already names this app, its secret goes too.
                            connect::discard(&root, &settings);
                            Err(error)
                        }
                    },
                )
                .map(|(committed, settings)| {
                    open(&settings.installation_url());
                    let device_flow = connect::device_flow_enabled(&settings).map_err(|error| error.to_string());
                    Connected {
                        settings,
                        committed,
                        device_flow,
                    }
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn check(&mut self, ctx: &egui::Context, settings: github::Settings) {
        let (tx, rx) = channel();
        self.checking = Some(rx);
        // Each job wakes the UI when it finishes, so its outcome shows without input.
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(connect::device_flow_enabled(&settings).map_err(|error| error.to_string()));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ui: &egui::Ui, draft: &mut Draft) {
        if let Some(rx) = &self.connecting {
            match rx.try_recv() {
                Ok(Ok(connected)) => {
                    draft.adopt_github(&connected.committed, Some(connected.settings));
                    self.device_flow = Some(connected.device_flow);
                    self.connecting = None;
                }
                Ok(Err(message)) => {
                    self.message = Some(message);
                    self.connecting = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.connecting = None,
            }
            // A finished attempt, also a failed one, stops its loopback server at once.
            if self.connecting.is_none() {
                self.abort = None;
            }
        }
        if let Some(rx) = &self.saving {
            match rx.try_recv() {
                Ok(Ok((committed, github))) => {
                    if github.is_none() {
                        self.device_flow = None;
                        self.message = Some(
                            "Disconnected. New clouds get no GitHub access. To end the access of running clouds, \
                             delete the app on GitHub."
                                .into(),
                        );
                    }
                    draft.adopt_github(&committed, github);
                    self.saving = None;
                }
                Ok(Err(message)) => {
                    self.message = Some(message);
                    self.saving = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.saving = None,
            }
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(found) => {
                    self.device_flow = Some(found);
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
            self.check(ui.ctx(), settings.clone());
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
                card.connect(ui.ctx(), draft.root().to_owned());
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
    match card.device_flow.clone() {
        Some(Ok(false)) => {
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
        Some(Ok(true)) => caption(ui, "Device sign-in is on."),
        Some(Err(error)) => {
            ui.label(
                RichText::new(format!("Horizon could not check the app's settings: {error}"))
                    .size(12.0)
                    .color(theme::PALETTE_RED()),
            );
            if ui.add(chrome_button("Check again")).clicked() {
                card.device_flow = None;
            }
        }
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
    ui.radio_value(
        &mut chosen,
        Mode::Automatic,
        "Automatic (no clicks after the first approval)",
    );
    caption(
        ui,
        "The first cloud asks once to authorize the app in your browser. Each cloud keeps a copy of the \
         app secret, readable only by its system service.",
    );
    if chosen != mode
        && card.saving.is_none()
        && let Some(github) = draft.settings.github.clone()
    {
        card.save(
            ui.ctx(),
            draft.root().to_owned(),
            Some(github::Settings {
                mode: chosen,
                ..github.clone()
            }),
            github,
        );
    }
    caption(
        ui,
        "Each cloud renews its own access for about 6 months, also while this computer is off.",
    );
    if ui
        .add_enabled(card.saving.is_none(), chrome_button("Disconnect"))
        .clicked()
    {
        card.save(ui.ctx(), draft.root().to_owned(), None, settings.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::{Abort, Card};
    use crate::test_egui::DiscardTextures as _;

    #[test]
    fn a_failed_connect_attempt_stops_its_server_at_once() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = horizon_core::cloud_runtime::setup::Draft::load(root.path()).unwrap();
        let cancel = horizon_core::cloud_runtime::Cancellation::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut card = Card {
            connecting: Some(receiver),
            abort: Some(Abort(cancel.clone())),
            ..Card::default()
        };
        sender.send(Err("The browser could not be opened.".into())).unwrap();
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| card.poll(ui, &mut draft))
            .discard_textures();
        assert!(cancel.is_cancelled(), "the loopback server ends with the attempt");
        assert!(!card.connecting());
    }

    #[test]
    fn a_connect_flow_under_way_holds_the_form_save() {
        let mut card = Card::default();
        assert!(!card.connecting());
        let (_sender, receiver) = std::sync::mpsc::channel();
        card.connecting = Some(receiver);
        assert!(card.connecting(), "Save waits while the flow runs");
    }
}
