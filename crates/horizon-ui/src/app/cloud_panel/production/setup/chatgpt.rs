//! The Codex sign-in card: sign in with `ChatGPT` and let eligible work use the
//! user's `ChatGPT` plan. The flow runs in the system browser against a loopback
//! callback; only the token-less account summary comes back to the form.
use super::dashboard::{caption, label};
use crate::{app::util::primary_button, theme};
use egui::{RichText, vec2};
use horizon_core::cloud_runtime::chatgpt;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Whether a card message reports an error or a success.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageTone {
    Error,
    Success,
}

/// What the card waits for and what it last learned.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    signing_in: Option<Receiver<Result<chatgpt::Connection, String>>>,
    signing_out: Option<Receiver<Result<Option<bool>, String>>>,
    confirming: Option<Receiver<Result<(), String>>>,
    /// Whether the saved connection was loaded from the draft.
    loaded: bool,
    /// The saved connection, loaded once and refreshed when a flow finishes.
    connection: Option<chatgpt::Connection>,
    message: Option<(MessageTone, String)>,
    /// Ends the sign-in under way when the card goes, such as on Cancel.
    abort: Option<Abort>,
}

/// Cancels its flow when dropped.
struct Abort(horizon_core::cloud_runtime::Cancellation);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Card {
    /// Whether a sign-in, sign-out or confirmation runs, which must hold the form save.
    pub(super) fn busy(&self) -> bool {
        self.signing_in.is_some() || self.signing_out.is_some() || self.confirming.is_some()
    }

    /// Keeps `connection_slot`, the draft's `chatgpt` field, in step with finished flows.
    /// It runs on every frame, also while another mode is selected, so a finished
    /// flow always lands and the busy state always ends.
    pub(super) fn tick(&mut self, ui: &egui::Ui, connection_slot: &mut Option<chatgpt::Connection>) {
        if !self.loaded {
            self.connection.clone_from(connection_slot);
            self.loaded = true;
        }
        if let Some(rx) = &self.signing_in {
            match rx.try_recv() {
                Ok(Ok(connection)) => {
                    *connection_slot = Some(connection.clone());
                    self.connection = Some(connection);
                    self.signing_in = None;
                }
                Ok(Err(message)) => {
                    self.message = Some((MessageTone::Error, message));
                    self.signing_in = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.signing_in = None,
            }
        }
        if let Some(rx) = &self.signing_out {
            match rx.try_recv() {
                Ok(Ok(revoked)) => {
                    *connection_slot = None;
                    self.connection = None;
                    self.message = Some((
                        MessageTone::Success,
                        if revoked == Some(true) {
                            "Signed out. Codex no longer uses your ChatGPT plan.".into()
                        } else {
                            "Signed out locally. Remote revocation was not confirmed; disconnect this app in ChatGPT Settings.".into()
                        },
                    ));
                    self.signing_out = None;
                }
                Ok(Err(message)) => {
                    *connection_slot = None;
                    self.connection = None;
                    self.message = Some((MessageTone::Error, message));
                    self.signing_out = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => {
                    *connection_slot = None;
                    self.connection = None;
                    self.message = Some((
                        MessageTone::Error,
                        "Sign-out could not be verified. Reopen settings to check the saved account.".into(),
                    ));
                    self.signing_out = None;
                }
            }
        }
        if let Some(rx) = &self.confirming {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.confirming = None;
                    if let Some(connection) = self
                        .connection
                        .as_mut()
                        .filter(|connection| !connection.usage_confirmed)
                    {
                        connection.usage_confirmed = true;
                    }
                    if let Some(connection) = connection_slot
                        .as_mut()
                        .filter(|connection| !connection.usage_confirmed)
                    {
                        connection.usage_confirmed = true;
                    }
                }
                Ok(Err(message)) => {
                    self.message = Some((MessageTone::Error, message));
                    self.confirming = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.confirming = None,
            }
        }
        // A finished attempt, also a failed one, stops its loopback server at once.
        if self.signing_in.is_none() {
            self.abort = None;
        }
    }

    /// Opens the `ChatGPT` sign-in in the system browser against the loopback callback.
    fn sign_in(&mut self, ctx: &egui::Context, root: &std::path::Path) {
        let (tx, rx) = channel();
        self.signing_in = Some(rx);
        self.message = None;
        let cancel = horizon_core::cloud_runtime::Cancellation::default();
        self.abort = Some(Abort(cancel.clone()));
        let ctx = ctx.clone();
        let root: std::path::PathBuf = root.to_owned();
        std::thread::spawn(move || {
            let result: Result<chatgpt::Connection, String> =
                match chatgpt::start(&root, horizon_core::open_url, cancel) {
                    Err(error) => Err(error.to_string()),
                    Ok(receiver) => match receiver.recv() {
                        Ok(Ok(connection)) => Ok(connection),
                        Ok(Err(error)) => Err(error.to_string()),
                        Err(_) => Err("The sign-in flow ended unexpectedly".to_owned()),
                    },
                };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Revokes the renewable session on the provider, then clears the tokens locally.
    fn sign_out(&mut self, ctx: &egui::Context, root: &std::path::Path, client_id: String) {
        let (tx, rx) = channel();
        self.signing_out = Some(rx);
        self.message = None;
        let ctx = ctx.clone();
        let root: std::path::PathBuf = root.to_owned();
        std::thread::spawn(move || {
            let result = chatgpt::sign_out(&root, &client_id).map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Shows the first-time plan-usage confirmation and records it.
    fn confirm_usage(&mut self, ctx: &egui::Context, root: &std::path::Path, client_id: String) {
        let (tx, rx) = channel();
        self.confirming = Some(rx);
        self.message = None;
        let ctx = ctx.clone();
        let root: std::path::PathBuf = root.to_owned();
        std::thread::spawn(move || {
            let result = chatgpt::confirm_usage(&root, &client_id).map_err(|error| error.to_string());
            if let Err(error) = &result {
                tracing::warn!(%error, "could not record the ChatGPT plan-usage confirmation");
            }
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// The account row of the connected card, with the plan-usage notice.
    fn connected(&mut self, ui: &mut egui::Ui, root: &std::path::Path, connection: &chatgpt::Connection) {
        let name = connection
            .email
            .as_deref()
            .map_or_else(|| format!("account {}", connection.subject), str::to_owned);
        ui.horizontal(|ui| {
            label(ui, &format!("Signed in as {name}"));
            if connection.plan_usage {
                ui.label(RichText::new("ChatGPT plan").size(12.0).color(theme::PALETTE_GREEN()));
            }
        });
        if connection.plan_usage && !connection.usage_confirmed {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("You're using your ChatGPT plan for eligible work.")
                        .size(12.0)
                        .color(theme::PALETTE_YELLOW()),
                );
                if self.confirming.is_none()
                    && ui
                        .add_enabled(!self.busy(), primary_button("Got it").min_size(vec2(72.0, 24.0)))
                        .clicked()
                {
                    self.confirm_usage(ui.ctx(), root, connection.client_id.clone());
                }
            });
            caption(ui, "You can review your plan's usage any time on ChatGPT.");
        }
        ui.horizontal_wrapped(|ui| {
            if connection.plan_usage {
                // Link-styled button: an explicit hyperlink would treat its label as a URL.
                if ui
                    .add_enabled(
                        !self.busy(),
                        egui::Button::new(RichText::new("Manage usage").color(theme::ACCENT()).underline())
                            .fill(egui::Color32::TRANSPARENT),
                    )
                    .clicked()
                    && let Err(error) = horizon_core::open_url("https://chatgpt.com/settings/usage")
                {
                    tracing::warn!(%error, "could not open the `ChatGPT` usage page");
                }
            }
            if self.confirming.is_none()
                && ui
                    .add_enabled(!self.busy(), egui::Button::new("Sign out").min_size(vec2(72.0, 24.0)))
                    .clicked()
            {
                self.sign_out(ui.ctx(), root, connection.client_id.clone());
            }
        });
        if !connection.plan_usage {
            caption(
                ui,
                "Sign this account in with a ChatGPT plan to use the plan for eligible work.",
            );
        }
    }
}

/// The Codex "`ChatGPT` plan" row of the Coding agents card. `connection_slot` is the
/// draft's `chatgpt` field, kept in step when a flow finishes.
pub(super) fn row(
    ui: &mut egui::Ui,
    connection_slot: &mut Option<chatgpt::Connection>,
    root: &std::path::Path,
    card: &mut Card,
) {
    card.tick(ui, connection_slot);
    let signing_in = card.signing_in.is_some();
    // A signed-out registration is retained on disk; only a live connection shows
    // the connected row.
    let connection = card.connection.clone().filter(|connection| connection.signed_in);
    if let Some(connection) = connection {
        card.connected(ui, root, &connection);
    } else {
        caption(
            ui,
            "Use your ChatGPT plan for eligible Codex work. Sign-in happens once, in your browser.",
        );
        if ui
            .add_enabled(
                !card.busy(),
                primary_button("Continue with ChatGPT").min_size(vec2(190.0, 30.0)),
            )
            .clicked()
        {
            card.sign_in(ui.ctx(), root);
        }
    }
    if signing_in {
        caption(ui, "Waiting for sign-in: finish in your browser, then come back here.");
        ui.horizontal_wrapped(|ui| {
            if card.abort.is_some() && ui.button("Cancel").clicked() {
                card.abort = None;
                card.signing_in = None;
            }
        });
    }
    if let Some((tone, message)) = &card.message {
        let color = if *tone == MessageTone::Success {
            theme::PALETTE_GREEN()
        } else {
            theme::PALETTE_RED()
        };
        ui.label(RichText::new(message).size(12.0).color(color));
    }
}

#[cfg(test)]
mod tests {
    use super::{Abort, Card};
    use crate::test_egui::DiscardTextures as _;

    #[test]
    fn uncertain_sign_out_invalidates_both_cached_connections() {
        for disconnected in [false, true] {
            let connection = horizon_core::cloud_runtime::chatgpt::Connection {
                client_id: "client-a".into(),
                email: None,
                subject: "account-a".into(),
                scopes: vec!["chatgpt.tokens.use.direct".into()],
                plan_usage: true,
                usage_confirmed: true,
                signed_in: true,
                saved_at_unix: 0,
            };
            let (sender, receiver) = std::sync::mpsc::channel();
            let mut card = Card {
                loaded: true,
                connection: Some(connection.clone()),
                signing_out: Some(receiver),
                ..Card::default()
            };
            if disconnected {
                drop(sender);
            } else {
                sender
                    .send(Err("The local clear could not be verified.".into()))
                    .unwrap();
            }
            let mut slot = Some(connection);
            let _ = egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| card.tick(ui, &mut slot))
                .discard_textures();
            assert!(slot.is_none() && card.connection.is_none());
            assert!(!card.busy());
            assert!(card.message.is_some());
        }
    }

    #[test]
    fn a_failed_sign_in_stops_its_server_at_once() {
        let root = tempfile::tempdir().unwrap();
        let mut draft = horizon_core::cloud_runtime::setup::Draft::load(root.path()).unwrap();
        let cancel = horizon_core::cloud_runtime::Cancellation::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut card = Card {
            signing_in: Some(receiver),
            abort: Some(Abort(cancel.clone())),
            ..Card::default()
        };
        sender.send(Err("The browser could not be opened.".into())).unwrap();
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| card.tick(ui, &mut draft.chatgpt))
            .discard_textures();
        assert!(cancel.is_cancelled(), "the loopback server ends with the attempt");
        assert!(!card.busy());
    }

    #[test]
    fn a_sign_in_under_way_holds_the_form_save() {
        let mut card = Card::default();
        assert!(!card.busy());
        let (_sender, receiver) = std::sync::mpsc::channel();
        card.signing_in = Some(receiver);
        assert!(card.busy(), "Save waits while the flow runs");
    }

    #[test]
    fn a_finished_confirmation_releases_busy_and_confirms_usage() {
        let connection = horizon_core::cloud_runtime::chatgpt::Connection {
            client_id: "oaiapp_one".into(),
            email: Some("peters@example.com".into()),
            subject: "user-1".into(),
            scopes: vec!["chatgpt.tokens.use.direct".into()],
            plan_usage: true,
            usage_confirmed: false,
            signed_in: true,
            saved_at_unix: 0,
        };
        let mut card = Card {
            connection: Some(connection.clone()),
            loaded: true,
            ..Card::default()
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        card.confirming = Some(receiver);
        assert!(card.busy(), "Save waits while the confirmation is written");
        sender.send(Ok(())).unwrap();
        let mut slot = Some(connection);
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| card.tick(ui, &mut slot))
            .discard_textures();
        assert!(!card.busy(), "the confirmation ending releases Save");
        assert!(card.connection.as_ref().unwrap().usage_confirmed);
        assert!(slot.as_ref().unwrap().usage_confirmed);
    }
}
