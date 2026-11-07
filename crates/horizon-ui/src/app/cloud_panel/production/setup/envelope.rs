//! The spending envelope card of Cloud settings: the person's one approval for the clouds an
//! agent rents, and the way to take it away. A machine without it rents nothing for an agent,
//! so the card says so plainly instead of leaving the section to a file.
use super::dashboard::{Tone, caption, header, label, surface};
use crate::{app::util::chrome_button, theme};
use egui::RichText;
use horizon_core::cloud_runtime::{
    envelope::{self, Envelope, LONGEST_HOURS, Spent},
    provider,
    setup::{self, Draft},
};
use std::{
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

/// How often an open card reads the rent record again. The file is small, and the
/// figure moves only when an agent's create is admitted.
const REREAD: Duration = Duration::from_secs(5);

/// What the card holds while a person fills it in, and what it waits for.
#[derive(Default)]
pub(in crate::app::cloud_panel) struct Card {
    /// The provider the approval names, as the person chose it.
    provider: Option<&'static str>,
    /// The hours and the total as typed, so a half-typed number is never an approval.
    hours: String,
    total: String,
    /// A save under way, and the message it left.
    saving: Option<Receiver<Result<Saved, String>>>,
    message: Option<String>,
    /// What the approved envelope already paid for, and the approval it belongs to. The
    /// record is read off the UI thread, so a slow disk never stalls a frame.
    spent: Option<Spent>,
    spent_for: Option<String>,
    reading: Option<Receiver<Option<Spent>>>,
    /// When the record is read again, so a rent that arrives while the card is open
    /// shows without reopening the dialog.
    next: Option<Instant>,
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
            .map(|committed| Saved {
                committed,
                envelope: next,
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Reads what the approved envelope already paid for: once when the approval
    /// changes, then again while the card stays open.
    fn read_spent(&mut self, ctx: &egui::Context, draft: &Draft) {
        let Some(approved) = draft.settings.spending_envelope.clone() else {
            self.spent = None;
            self.spent_for = None;
            self.next = None;
            return;
        };
        if self.reading.is_some() {
            return;
        }
        let now = Instant::now();
        let changed = self.spent_for.as_deref() != Some(approved.approved_at.as_str());
        if !changed && let Some(next) = self.next.filter(|next| now < *next) {
            // An idle card draws no frame by itself; wake it for the next read.
            ctx.request_repaint_after(next - now);
            return;
        }
        let (tx, rx) = channel();
        self.reading = Some(rx);
        let root = draft.root().to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let spent = envelope::Ledger::at(&root).spent(&approved).ok();
            let _ = tx.send(spent);
            ctx.request_repaint();
        });
    }

    /// Takes the reading of the record when it arrives.
    fn poll_spent(&mut self, ctx: &egui::Context, draft: &Draft) {
        let Some(rx) = &self.reading else {
            return;
        };
        match rx.try_recv() {
            Ok(spent) => {
                self.spent = spent;
                self.spent_for = draft
                    .settings
                    .spending_envelope
                    .as_ref()
                    .map(|approved| approved.approved_at.clone());
                self.next = Some(Instant::now() + REREAD);
                self.reading = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(250)),
            Err(TryRecvError::Disconnected) => self.reading = None,
        }
    }

    fn poll(&mut self, ui: &egui::Ui, draft: &mut Draft) {
        let Some(rx) = &self.saving else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(saved)) => {
                // The card shows what the file now holds, and the form counts that file as its
                // starting point, so its own Save is not refused as a change made elsewhere.
                draft.adopt_spending_envelope(&saved.committed, saved.envelope);
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

/// What a save committed, and the envelope the settings now hold.
struct Saved {
    committed: setup::Committed,
    envelope: Option<Envelope>,
}

/// The approved envelope: its limits, who approved it, what it already paid for, and the
/// way to take it away.
fn approved_state(ui: &mut egui::Ui, draft: &Draft, card: &mut Card, envelope: &Envelope) {
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
    // What the record says the envelope already paid for, and what that leaves. A
    // person cannot judge a bound without the figure that moves toward it.
    // Nothing rented needs no line: the limits above already say what is left.
    if let Some(spent) = card.spent.filter(|spent| spent.hours > 0.0 || spent.amount > 0.0) {
        let left = envelope.remaining(spent);
        caption(
            ui,
            &format!(
                "Rented so far: {} of {} h · {:.2} of {:.2} {}. Left: {} h · {:.2} {}.",
                count(spent.hours),
                count(envelope.max_hours),
                spent.amount,
                envelope.max_total,
                envelope.currency,
                count(left.hours),
                left.amount,
                left.currency
            ),
        );
    }
    if ui.add_enabled(card.saving.is_none(), chrome_button("Revoke")).clicked() {
        let root = draft.root().to_owned();
        card.save(ui.ctx(), root, Some(envelope.clone()), None);
    }
}

/// Hours without a needless decimal, as the envelope itself writes them.
fn count(hours: f64) -> String {
    if hours.fract().abs() < 1e-9 {
        format!("{hours:.0}")
    } else {
        format!("{hours:.1}")
    }
}

/// The providers an envelope may name: the ones that rent compute, never a saved host.
fn providers() -> Vec<&'static provider::Description> {
    vec![&provider::RUNPOD, &provider::HETZNER, &provider::AZURE]
}

pub(in crate::app::cloud_panel) fn card(ui: &mut egui::Ui, draft: &mut Draft, card: &mut Card) {
    card.poll(ui, draft);
    card.read_spent(ui.ctx(), draft);
    card.poll_spent(ui.ctx(), draft);
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
            approved_state(ui, draft, card, envelope);
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
