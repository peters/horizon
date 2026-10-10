//! The Codex sign-in card: sign in with `ChatGPT` and let eligible work use the
//! user's `ChatGPT` plan. The flow runs in the system browser against a loopback
//! callback; only the token-less account summary comes back to the form.
use super::dashboard::{caption, label};
use crate::{app::util::primary_button, theme};
use egui::{RichText, vec2};
use horizon_core::cloud_runtime::chatgpt;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// What the card waits for and what it last learned.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    signing_in: Option<Receiver<Result<chatgpt::Connection, String>>>,
    signing_out: Option<Receiver<Result<Option<bool>, String>>>,
    confirming: bool,
    /// Whether the saved connection was loaded from the draft.
    loaded: bool,
    /// The saved connection, loaded once and refreshed when a flow finishes.
    connection: Option<chatgpt::Connection>,
    message: Option<String>,
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
    /// Whether a sign-in or sign-out runs, which must hold the form save.
    pub(super) fn busy(&self) -> bool {
        self.signing_in.is_some() || self.signing_out.is_some() || self.confirming
    }

    /// Keeps `connection_slot`, the draft's `chatgpt` field, in step with finished flows.
    fn poll(&mut self, ui: &egui::Ui, connection_slot: &mut Option<chatgpt::Connection>) {
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
                    self.message = Some(message);
                    self.signing_in = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.signing_in = None,
            }
        }
        if let Some(rx) = &self.signing_out {
            match rx.try_recv() {
                Ok(Ok(_revoked)) => {
                    *connection_slot = None;
                    self.connection = None;
                    self.message = Some("Signed out. Codex no longer uses your ChatGPT plan.".into());
                    self.signing_out = None;
                }
                Ok(Err(message)) => {
                    self.message = Some(message);
                    self.signing_out = None;
                }
                Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
                Err(TryRecvError::Disconnected) => self.signing_out = None,
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
        self.confirming = true;
        let ctx = ctx.clone();
        let root: std::path::PathBuf = root.to_owned();
        std::thread::spawn(move || {
            if let Err(error) = chatgpt::confirm_usage(&root, &client_id) {
                tracing::warn!(%error, "could not record the ChatGPT plan-usage confirmation");
            }
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
                if !self.confirming
                    && ui
                        .add_enabled(!self.busy(), primary_button("Got it").min_size(vec2(72.0, 24.0)))
                        .clicked()
                {
                    self.confirm_usage(ui.ctx(), root, connection.client_id.clone());
                }
            });
            caption(ui, "You can review your plan's usage any time on ChatGPT.");
        }
        if connection.plan_usage {
            ui.horizontal_wrapped(|ui| {
                if ui.hyperlink("Manage usage").clicked()
                    && let Err(error) = horizon_core::open_url("https://chatgpt.com/settings/usage")
                {
                    tracing::warn!(%error, "could not open the `ChatGPT` usage page");
                }
                if !self.confirming
                    && ui
                        .add_enabled(!self.busy(), egui::Button::new("Sign out").min_size(vec2(72.0, 24.0)))
                        .clicked()
                {
                    self.sign_out(ui.ctx(), root, connection.client_id.clone());
                }
            });
        } else {
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
    card.poll(ui, connection_slot);
    let signing_in = card.signing_in.is_some();
    if let Some(connection) = card.connection.clone() {
        card.connected(ui, root, &connection);
    } else {
        caption(
            ui,
            "Use your `ChatGPT` plan for eligible Codex work. Sign-in happens once, in your browser.",
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
    if let Some(message) = &card.message {
        ui.label(RichText::new(message).size(12.0).color(theme::PALETTE_RED()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Abort, Card};
    use crate::test_egui::DiscardTextures as _;

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
            .run_ui(egui::RawInput::default(), |ui| card.poll(ui, &mut draft.chatgpt))
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
}
