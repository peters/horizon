//! The spending envelope card of Cloud settings: the person's one approval for the clouds an
//! agent rents, and the way to take it away. A machine without it rents nothing for an agent,
//! so the card says so plainly instead of leaving the section to a file.
use super::dashboard::{Tone, caption, header, label, surface};
use crate::{app::util::chrome_button, theme};
use egui::RichText;
use horizon_core::cloud_runtime::{
    envelope::{self, Envelope, LONGEST_HOURS},
    provider,
    setup::{self, Draft},
};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// What the card holds while a person fills it in, and what it waits for.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    /// The provider the approval names, as the person chose it.
    provider: Option<&'static str>,
    /// The hours and the total as typed, so a half-typed number is never an approval.
    hours: String,
    total: String,
    /// A save under way, and the message it left.
    saving: Option<Receiver<Result<Option<Envelope>, String>>>,
    message: Option<String>,
}

impl Card {
    /// Whether a save is under way, so the form's own Save waits for it.
    pub(super) fn busy(&self) -> bool {
        self.saving.is_some()
    }

    fn save(&mut self, ctx: &egui::Context, root: std::path::PathBuf, shown: Option<Envelope>, next: Option<Envelope>) {
        let (tx, rx) = channel();
        self.saving = Some(rx);
        self.message = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = setup::save_spending_envelope(
                &root,
                shown.as_ref(),
                next.clone(),
                &horizon_core::cloud_runtime::Cancellation::default(),
            )
            .map(|_| next)
            .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ui: &egui::Ui, draft: &mut Draft) {
        let Some(rx) = &self.saving else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(next)) => {
                // The card shows what the file now holds, not what it held when it opened.
                draft.settings.spending_envelope = next;
                self.hours.clear();
                self.total.clear();
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
}

/// The providers an envelope may name: the ones that rent compute, never a saved host.
fn providers() -> Vec<&'static provider::Description> {
    vec![&provider::RUNPOD, &provider::HETZNER, &provider::AZURE]
}

pub(in crate::app::cloud_panel) fn card(ui: &mut egui::Ui, draft: &mut Draft, card: &mut Card) {
    card.poll(ui, draft);
    let approved = draft.settings.spending_envelope.clone();
    surface(ui, |ui| {
        header(
            ui,
            "Spending envelope",
            "What an agent's clouds may cost",
            Some(if approved.is_some() {
                (Tone::Ready, "Approved")
            } else {
                (Tone::Attention, "None")
            }),
        );
        caption(
            ui,
            "An agent rents clouds only inside this approval. Without one, no agent rents anything here.",
        );
        if let Some(envelope) = &approved {
            label(
                ui,
                &format!(
                    "{} · up to {} h · up to {:.2} {}",
                    envelope.provider, envelope.max_hours, envelope.max_total, envelope.currency
                ),
            );
            caption(
                ui,
                &format!("Approved by {} at {}.", envelope.approved_by, envelope.approved_at),
            );
            if ui.add_enabled(card.saving.is_none(), chrome_button("Revoke")).clicked() {
                let root = draft.root().to_owned();
                card.save(ui.ctx(), root, approved.clone(), None);
            }
        } else {
            ui.horizontal(|ui| {
                for description in providers() {
                    let chosen = card.provider == Some(description.id);
                    if ui.selectable_label(chosen, description.label).clicked() {
                        card.provider = Some(description.id);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Hours");
                ui.add(egui::TextEdit::singleline(&mut card.hours).desired_width(60.0));
                ui.label("Total");
                ui.add(egui::TextEdit::singleline(&mut card.total).desired_width(80.0));
                let currency = card
                    .provider
                    .and_then(provider::by_id)
                    .map_or("", |description| description.currency);
                ui.label(currency);
            });
            caption(
                ui,
                &format!("At most {LONGEST_HOURS:.0} hours, and one provider: another provider never fits."),
            );
            let ready = card.provider.is_some()
                && card.hours.trim().parse::<f64>().is_ok_and(|hours| hours > 0.0)
                && card.total.trim().parse::<f64>().is_ok_and(|total| total > 0.0);
            if ui
                .add_enabled(ready && card.saving.is_none(), chrome_button("Approve"))
                .clicked()
                && let (Some(id), Ok(hours), Ok(total)) = (
                    card.provider,
                    card.hours.trim().parse::<f64>(),
                    card.total.trim().parse::<f64>(),
                )
                && let Some(description) = provider::by_id(id)
            {
                let root = draft.root().to_owned();
                let approval = Envelope {
                    provider: id.to_owned(),
                    currency: description.currency.to_owned(),
                    max_hours: hours,
                    max_total: total,
                    approved_by: who(),
                    approved_at: envelope::approved_now(),
                };
                card.save(ui.ctx(), root, None, Some(approval));
            }
        }
        if let Some(message) = &card.message {
            ui.label(RichText::new(message).size(12.0).color(theme::PALETTE_RED()));
        }
    });
}

/// The person who approves: this machine's user, as this computer knows it.
fn who() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "this computer's user".to_owned())
}
