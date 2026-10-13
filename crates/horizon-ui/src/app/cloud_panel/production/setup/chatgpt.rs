//! Sign in with `ChatGPT` for Codex in Cloud settings: one login on this computer that
//! each cloud gets, so Codex there starts signed in.
use super::dashboard::caption;
use crate::{
    app::util::{chrome_button, primary_button},
    theme,
};
use egui::{RichText, vec2};
use horizon_core::cloud_runtime::{
    Cancellation,
    codex_login::{self, Account},
};
use std::{
    path::Path,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

/// What the sign-in section waits for and what it last learned.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    /// A sign-in or sign-out under way; it ends with the account now on this computer.
    pending: Option<Receiver<Result<Option<Account>, String>>>,
    signing_in: bool,
    message: Option<String>,
    /// Ends a sign-in under way when the card goes, such as on Cancel.
    abort: Option<Abort>,
}

/// Cancels its sign-in when dropped, which stops `codex login` and its local server.
struct Abort(Cancellation);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Card {
    /// Whether a sign-in or sign-out is under way.
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn sign_in(&mut self, ctx: &egui::Context, home: std::path::PathBuf) {
        let cancel = Cancellation::default();
        self.abort = Some(Abort(cancel.clone()));
        self.signing_in = true;
        self.start(ctx, move || match codex_login::sign_in(&home, &cancel) {
            Ok(account) => Ok(Some(account)),
            // Cancel sign-in is no failure; the card shows whatever login is left.
            Err(_) if cancel.is_cancelled() => codex_login::account(&home).map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        });
    }

    fn sign_out(&mut self, ctx: &egui::Context, home: std::path::PathBuf) {
        self.signing_in = false;
        self.start(ctx, move || {
            codex_login::sign_out(&home)
                .map(|()| None)
                .map_err(|error| error.to_string())
        });
    }

    fn start(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Result<Option<Account>, String> + Send + 'static) {
        let (tx, rx) = channel();
        self.pending = Some(rx);
        self.message = None;
        // The job wakes the UI when it finishes, so its outcome shows without input.
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    /// Takes the outcome of a sign-in or sign-out when it is there. Called every frame, also
    /// while Codex shows another choice, so a finished sign-in always releases Save.
    pub(super) fn poll(&mut self, ui: &egui::Ui, signed_in: &mut Option<Account>) {
        let Some(rx) = &self.pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(account)) => {
                *signed_in = account;
                self.pending = None;
            }
            Ok(Err(message)) => {
                self.message = Some(message);
                self.pending = None;
            }
            Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(250)),
            Err(TryRecvError::Disconnected) => self.pending = None,
        }
        if self.pending.is_none() {
            self.abort = None;
            self.signing_in = false;
        }
    }
}

/// The sign-in shown under Codex when it signs in with `ChatGPT`: `signed_in` is the
/// account of the login of the cloud root `root`.
pub(super) fn section(ui: &mut egui::Ui, root: &Path, signed_in: &mut Option<Account>, card: &mut Card) {
    let home = codex_login::home(root);
    if card.signing_in {
        caption(
            ui,
            "Finish in your browser. If no ChatGPT page opened, check that the Codex CLI works \
             on this computer.",
        );
        if ui.add(chrome_button("Cancel sign-in")).clicked() {
            card.abort = None;
        }
    } else if let Some(account) = signed_in.clone() {
        let name = account.email.as_deref().unwrap_or("your ChatGPT account");
        let plan = account.plan.map(|plan| format!(" ({plan})")).unwrap_or_default();
        ui.label(
            RichText::new(format!("Signed in as {name}{plan}"))
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
        caption(
            ui,
            "Each cloud starts Codex signed in to this account when it is created or reconnected. \
             This is not your own Codex login on this computer.",
        );
        if ui.add_enabled(!card.busy(), chrome_button("Sign out")).clicked() {
            card.sign_out(ui.ctx(), home);
        }
    } else {
        caption(
            ui,
            "Sign in once with your ChatGPT plan. Each cloud then starts Codex signed in.",
        );
        if ui
            .add_enabled(
                !card.busy(),
                primary_button("Sign in with ChatGPT").min_size(vec2(170.0, 30.0)),
            )
            .clicked()
        {
            card.sign_in(ui.ctx(), home);
        }
    }
    if let Some(message) = &card.message {
        ui.label(RichText::new(message).size(12.0).color(theme::PALETTE_RED()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Abort, Card};
    use crate::test_egui::DiscardTextures as _;
    use horizon_core::cloud_runtime::codex_login::Account;

    fn poll(card: &mut Card, signed_in: &mut Option<Account>) {
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| card.poll(ui, signed_in))
            .discard_textures();
    }

    #[test]
    fn a_finished_sign_in_shows_the_account_and_stops_its_server() {
        let mut signed_in = None;
        let cancel = horizon_core::cloud_runtime::Cancellation::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut card = Card {
            pending: Some(receiver),
            signing_in: true,
            abort: Some(Abort(cancel.clone())),
            ..Card::default()
        };
        let account = Account {
            email: Some("person@example.test".into()),
            plan: Some("plus".into()),
        };
        sender.send(Ok(Some(account.clone()))).unwrap();
        poll(&mut card, &mut signed_in);
        assert_eq!(signed_in, Some(account));
        assert!(cancel.is_cancelled(), "codex login ends with the attempt");
        assert!(!card.busy() && !card.signing_in);
    }

    #[test]
    fn a_failed_sign_in_keeps_the_account_and_shows_why() {
        let mut signed_in = Some(Account::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut card = Card {
            pending: Some(receiver),
            ..Card::default()
        };
        sender.send(Err("Codex: Error logging in: synthetic".into())).unwrap();
        poll(&mut card, &mut signed_in);
        assert_eq!(signed_in, Some(Account::default()));
        assert_eq!(card.message.as_deref(), Some("Codex: Error logging in: synthetic"));
        assert!(!card.busy());
    }

    #[test]
    fn a_sign_out_clears_the_account() {
        let mut signed_in = Some(Account::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut card = Card {
            pending: Some(receiver),
            ..Card::default()
        };
        sender.send(Ok(None)).unwrap();
        poll(&mut card, &mut signed_in);
        assert_eq!(signed_in, None);
    }
}
