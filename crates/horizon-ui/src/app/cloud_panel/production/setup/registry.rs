//! The container registry card: one block per image repository, each bound to a read-only pull
//! credential. "Verified" means a saved validation exists for the repository's current pull grant.
use super::dashboard::{Tone, Verified, caption, chip, field, header, label, secret, surface};
use super::needed::{self, Needed};
use crate::{
    app::util::{chrome_button, danger_button, primary_button},
    theme,
};
use egui::{Align, Layout, RichText, Ui, vec2};
use horizon_core::cloud_runtime::{
    registry::{Action, Binding, draft::Draft},
    setup,
};

pub(super) fn card(
    ui: &mut Ui,
    accounts: &mut setup::Draft,
    verified: &Verified,
    needed: &mut Needed,
) -> Option<Action> {
    let mut action = None;
    let compute_saved = accounts.runpod_key.is_empty();
    let pending = needed.pending(&accounts.registries).is_some();
    let status = if pending {
        (Tone::Attention, "Not set up")
    } else {
        aggregate(&accounts.registries, verified)
    };
    surface(ui, |ui| {
        header(
            ui,
            "Container registry",
            "Private images workers pull with a read-only credential",
            Some(status),
        );
        if accounts.registries.is_empty() && !pending {
            caption(
                ui,
                "Bind an image repository to give workers read-only pull access. Horizon never publishes \
                 an image and allocates no worker to validate one.",
            );
        }
        if !compute_saved {
            caption(
                ui,
                "Clear the unsaved compute key to manage existing provider access, or save it to switch accounts.",
            );
        }
        needed::block(ui, needed, &mut accounts.registries);
        let credentials = &accounts.saved_credentials;
        for (index, draft) in accounts.registries.iter_mut().enumerate() {
            let entry = ui.push_id(index, |ui| {
                ui.separator();
                let saved_publish = draft
                    .original
                    .as_ref()
                    .and_then(|binding| binding.publish.as_ref())
                    .is_some_and(|auth| credentials.contains(&auth.secret_file));
                let saved_pull = draft
                    .original
                    .as_ref()
                    .is_some_and(|binding| credentials.contains(&binding.pull.secret_file));
                let publishing_open = needed.added(draft);
                binding(
                    ui,
                    draft,
                    verified,
                    compute_saved,
                    (saved_pull, saved_publish, publishing_open),
                    &mut action,
                );
            });
            if needed.reveal(draft) {
                entry.response.scroll_to_me(Some(Align::Center));
            }
        }
        // A shown repository keeps the empty form of another one behind this button.
        let add = if accounts.registries.is_empty() && needed.pending(&accounts.registries).is_none() {
            "Add image repository"
        } else {
            "Add another"
        };
        if ui.add(chrome_button(add).min_size(vec2(160.0, 32.0))).clicked() {
            accounts.registries.push(Draft::default());
        }
    });
    action
}

/// One word for the whole card: what the least ready binding needs.
fn aggregate(registries: &[Draft], verified: &Verified) -> (Tone, &'static str) {
    registries
        .iter()
        .map(|draft| state(draft, verified))
        .min_by_key(|(tone, _)| match tone {
            Tone::Attention => 0,
            Tone::Idle => 1,
            Tone::Ready => 2,
        })
        .unwrap_or((Tone::Idle, "Optional"))
}

fn state(draft: &Draft, verified: &Verified) -> (Tone, &'static str) {
    match &draft.original {
        None => (Tone::Attention, "Not saved"),
        Some(_) if !draft.is_saved() => (Tone::Attention, "Unsaved changes"),
        Some(binding) if verified.contains_key(&binding.repository) => (Tone::Ready, "Pull access verified"),
        Some(_) => (Tone::Attention, "Needs validation"),
    }
}

fn binding(
    ui: &mut Ui,
    draft: &mut Draft,
    verified: &Verified,
    compute_saved: bool,
    (saved_pull, saved_publish, publishing_open): (bool, bool, bool),
    action: &mut Option<Action>,
) {
    let (tone, word) = state(draft, verified);
    ui.horizontal(|ui| {
        let name = if draft.repository.is_empty() {
            "New image repository"
        } else {
            draft.repository.as_str()
        };
        ui.label(RichText::new(name).size(13.0).strong().color(theme::FG()));
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| chip(ui, tone, word));
    });
    label(ui, "Image repository (registry.example.com/team/worker)");
    ui.add_enabled(
        draft.original.is_none(),
        egui::TextEdit::singleline(&mut draft.repository)
            .id_salt("repository")
            .desired_width(f32::INFINITY)
            .margin(vec2(12.0, 9.0)),
    );
    ui.columns(2, |columns| {
        field(
            &mut columns[0],
            "Worker pull username",
            &mut draft.pull_username,
            "Username",
        );
        label(&mut columns[1], "Read-only pull credential");
        secret(
            &mut columns[1],
            "pull-secret",
            &mut draft.pull_secret,
            saved_pull,
            "Paste dedicated credential",
        );
    });
    ui.checkbox(
        &mut draft.read_only_confirmed,
        RichText::new("This dedicated pull grant is read-only and limited to the intended repository")
            .size(12.0)
            .color(theme::FG_SOFT()),
    );
    caption(
        ui,
        "For ghcr.io, only read:packages is accepted. Other registries require you to confirm the issuer's \
         grant. Unknown expiry is shown as unknown.",
    );
    expiry_and_publishing(ui, draft, saved_publish, publishing_open);
    if let Some(saved) = draft.original.clone() {
        management(ui, draft, &saved, verified, compute_saved, action);
    }
}

/// `open`: the entry was added for a cloud's repository, whose push needs these fields.
fn expiry_and_publishing(ui: &mut Ui, draft: &mut Draft, saved_publish: bool, open: bool) {
    egui::CollapsingHeader::new(
        RichText::new("Expiry and publishing")
            .size(12.0)
            .color(theme::FG_SOFT()),
    )
    .default_open(open)
    .show(ui, |ui| {
        field(
            ui,
            "Pull expiry",
            &mut draft.pull_expiry,
            "Unknown, or 2027-01-01T00:00:00Z",
        );
        field(
            ui,
            "Publishing username (optional for existing images)",
            &mut draft.publish_username,
            "Username",
        );
        label(ui, "Publishing credential");
        secret(
            ui,
            "publish-secret",
            &mut draft.publish_secret,
            saved_publish,
            "Paste dedicated credential",
        );
        field(
            ui,
            "Publishing expiry",
            &mut draft.publish_expiry,
            "Unknown, or 2027-01-01T00:00:00Z",
        );
    });
}

fn management(
    ui: &mut Ui,
    draft: &mut Draft,
    saved: &Binding,
    verified: &Verified,
    compute_saved: bool,
    action: &mut Option<Action>,
) {
    caption(ui, &format!("Pull generation: {}", saved.generation));
    if let Some(validation) = verified.get(&saved.repository) {
        caption(
            ui,
            &format!(
                "Verified image: {}. Scope: {}. Expiry: {}.",
                validation.image,
                validation.scope,
                validation.expires_at.as_deref().unwrap_or("Unknown")
            ),
        );
    }
    field(
        ui,
        "Immutable image to validate (repository@sha256:…)",
        &mut draft.validation_image,
        "ghcr.io/team/worker@sha256:…",
    );
    let manageable = compute_saved && draft.is_saved();
    ui.add_enabled_ui(manageable, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !draft.validation_image.is_empty(),
                    primary_button("Validate pull access").min_size(vec2(148.0, 32.0)),
                )
                .clicked()
            {
                *action = Some(Action::Verify {
                    image: draft.validation_image.clone(),
                });
            }
            if ui.add(chrome_button("Status").min_size(vec2(84.0, 32.0))).clicked() {
                *action = Some(Action::Status {
                    repository: saved.repository.clone(),
                    generation: saved.generation.clone(),
                });
            }
            if ui.add(chrome_button("Reconcile").min_size(vec2(84.0, 32.0))).clicked() {
                *action = Some(Action::Reconcile {
                    repository: saved.repository.clone(),
                    generation: saved.generation.clone(),
                });
            }
            if ui
                .add(danger_button("Revoke pull binding").min_size(vec2(140.0, 32.0)))
                .clicked()
            {
                *action = Some(Action::Revoke {
                    repository: saved.repository.clone(),
                    generation: saved.generation.clone(),
                });
            }
        });
        for generation in &saved.retired {
            ui.horizontal_wrapped(|ui| {
                caption(ui, &format!("Previous: {generation}"));
                if ui
                    .add(chrome_button("Reconcile previous").min_size(vec2(120.0, 28.0)))
                    .clicked()
                {
                    *action = Some(Action::Reconcile {
                        repository: saved.repository.clone(),
                        generation: generation.clone(),
                    });
                }
                if ui
                    .add(danger_button("Revoke previous").min_size(vec2(112.0, 28.0)))
                    .clicked()
                {
                    *action = Some(Action::Revoke {
                        repository: saved.repository.clone(),
                        generation: generation.clone(),
                    });
                }
            });
        }
    });
    if !draft.is_saved() {
        caption(ui, "Save changes before managing provider access.");
    }
    caption(
        ui,
        "Enter a replacement pull credential and save to rotate. Previous bindings remain available for explicit \
         revocation. Revocation removes provider pull access; revoke the token at its issuer separately. \
         Running workers are unchanged.",
    );
}
