//! The source step at the top of the New cloud dialog, and the footer while it is the only step.
use super::{super::selector::widgets, Failure, Remote, State};
use crate::{
    app::cloud_panel::production::{Production, creation::Actions},
    theme,
};
use egui::{Button, Frame, Id, Key, RichText, Stroke, TextEdit, Ui, Vec2};
use std::path::PathBuf;

/// What the step asks the app to do once drawing is over.
#[derive(Default)]
pub(in super::super) struct Step {
    pub browse: bool,
    pub choose_folder: bool,
    pub adopt: Option<PathBuf>,
}

/// The link or folder field with its status: progress, access, where a clone lands, and the
/// token card for a private repository.
pub(in super::super) fn step(ui: &mut Ui, form: &mut Production, refocus: bool) -> Step {
    let mut step = Step::default();
    form.source.mirror(&form.repository);
    ui.label(
        RichText::new("Where is your code?")
            .size(14.0)
            .strong()
            .color(theme::FG()),
    );
    let id = Id::new("cloud-source");
    let empty = form.repository.trim().is_empty();
    if form.focus_title_on_open
        && empty
        && !ui.is_sizing_pass()
        && ui.is_enabled()
        && !ui.input(|input| input.pointer.any_down() || input.pointer.any_released())
    {
        ui.memory_mut(|memory| memory.request_focus(id));
        form.focus_title_on_open = false;
    }
    let browse = 96.0;
    // While a clone runs the field is not a request for something else.
    let response = ui
        .add_enabled_ui(form.source.job.is_none(), |ui| {
            ui.horizontal(|ui| {
                let edit = ui.add_sized(
                    [ui.available_width() - browse - ui.spacing().item_spacing.x, 38.0],
                    TextEdit::singleline(&mut form.source.input)
                        .id(id)
                        .font(egui::FontId::proportional(15.0))
                        .margin(Vec2::new(12.0, 10.0))
                        .hint_text("github.com/owner/repo  ·  gitlab.com/group/repo  ·  ~/code/project"),
                );
                step.browse = ui
                    .add_sized(
                        [browse, 38.0],
                        Button::new(RichText::new("Browse…").size(13.0)).corner_radius(8),
                    )
                    .clicked();
                edit
            })
            .inner
        })
        .inner;
    // A link may carry a token; what was typed is not kept for undoing.
    super::super::forget_undo(ui.ctx(), response.id);
    if refocus {
        response.request_focus();
    }
    // Enter does what Continue would, and nothing while Continue waits for something.
    if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) && form.source.next_step().is_ok() {
        form.source.start(ui.ctx());
    }
    if let Some(folder) = form.source.folder_settled(ui.ctx()) {
        if std::path::Path::new(&form.repository) == folder {
            form.source.accept_text();
        } else {
            step.adopt = Some(folder);
        }
    }
    let enter = ui.input(|input| input.key_pressed(Key::Enter));
    step.choose_folder = form.source.status(ui, enter);
    form.source.github_section(ui);
    step
}

impl State {
    /// The clone's status in one place. True when the folder for clones is to be chosen.
    fn status(&mut self, ui: &mut Ui, enter: bool) -> bool {
        if let Some(job) = &self.job {
            let snapshot = job
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            super::progress::show(ui, &snapshot, &job.cancel);
            return false;
        }
        // What was said about the last clone is over once another repository is being asked for.
        if self.editing() {
            self.note = None;
        }
        if let Some(note) = self.note {
            ui.label(RichText::new(note).size(13.0).color(theme::FG_DIM()));
        }
        let Some(remote) = self.remote().cloned() else {
            if self.unrecognised() {
                ui.label(
                    RichText::new("That is neither a repository link nor a folder that exists.")
                        .size(13.0)
                        .color(theme::FG_DIM()),
                );
            }
            return false;
        };
        ui.horizontal(|ui| {
            provider_badge(ui, &remote.host);
            ui.label(RichText::new(&remote.name).size(15.0).strong().color(theme::FG()));
        });
        let choose_folder = self.destination_row(ui, &remote);
        self.choosing_parent |= choose_folder;
        self.probe_access(ui.ctx(), &remote);
        if self.probe.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Checking access…").size(13.5).color(theme::FG_SOFT()));
            });
            return choose_folder;
        }
        let unfinished = self.plan(&remote).resumable.clone();
        match self.failure.clone() {
            Some(Failure::SignIn(host)) => {
                if self.offers_account(&remote) {
                    self.connected_card(ui);
                } else {
                    let origin = super::origin(&remote.url).to_owned();
                    self.sign_in(ui, &host, &origin, enter);
                }
            }
            Some(Failure::Interrupted(text)) => {
                ui.label(RichText::new(text).size(13.0).color(theme::PALETTE_YELLOW()));
            }
            Some(failure) => {
                ui.label(
                    RichText::new(failure.to_string())
                        .size(13.0)
                        .color(theme::PALETTE_RED()),
                );
            }
            None if unfinished.is_some() => {
                ui.label(
                    RichText::new(
                        "An earlier clone of this repository stopped here. Continue resumes from what it received.",
                    )
                    .size(13.0)
                    .color(theme::PALETTE_YELLOW()),
                );
            }
            None if self.public => {
                ui.label(
                    RichText::new("Git can read this repository. No token needed.")
                        .size(13.0)
                        .color(theme::PALETTE_GREEN()),
                );
            }
            None => {}
        }
        if let Some(folder) = unfinished
            && ui.small_button("Start over").clicked()
        {
            self.start_over(&folder);
        }
        choose_folder
    }

    /// Where the clone will land, with a button that is hard to miss. True when it was pressed.
    fn destination_row(&mut self, ui: &mut Ui, remote: &Remote) -> bool {
        let plan = self.plan(remote);
        // A checkout of this link that is already there is used as it is, wherever it sits.
        let (caption, target) = match (&plan.existing, &plan.resumable) {
            (Some(existing), _) => ("ALREADY CLONED, CONTINUE USES IT", existing.clone()),
            (None, Some(stopped)) => ("UNFINISHED CLONE, CONTINUE RESUMES IT", stopped.clone()),
            (None, None) => ("CLONE INTO", plan.destination.clone()),
        };
        widgets::caption(ui, caption);
        ui.horizontal(|ui| {
            let button = 150.0;
            Frame::new()
                .fill(theme::PANEL_BG_ALT())
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width((ui.available_width() - button - 32.0).max(120.0));
                    ui.label(
                        RichText::new(horizon_core::dir_search::abbreviate_home(&target))
                            .size(14.0)
                            .color(theme::FG()),
                    );
                });
            ui.add(
                Button::new(RichText::new("Choose folder…").size(14.0).color(theme::ACCENT()))
                    .stroke(Stroke::new(1.0, theme::ACCENT()))
                    .min_size(Vec2::new(button, 36.0))
                    .corner_radius(8),
            )
            .clicked()
        })
        .inner
    }

    /// Git had no credential for this host: ask for a token instead of a terminal.
    fn sign_in(&mut self, ui: &mut Ui, host: &str, origin: &str, enter: bool) {
        Frame::new()
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(10)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(
                    RichText::new(format!("Private repository on {host}"))
                        .size(14.5)
                        .strong()
                        .color(theme::FG()),
                );
                let (body, color) = if self.token_tried {
                    (
                        format!("{host} did not accept that token. It needs read access to this repository."),
                        theme::PALETTE_RED(),
                    )
                } else {
                    (
                        format!(
                            "This repository is private, or the address is wrong. Paste a personal access token with read access to {host} once; it goes to Git only."
                        ),
                        theme::FG_SOFT(),
                    )
                };
                ui.label(RichText::new(body).size(13.0).color(color));
                let field = ui.add_sized(
                    [ui.available_width(), 36.0],
                    TextEdit::singleline(&mut self.token)
                        .id(Id::new("cloud-clone-token"))
                        .password(true)
                        .margin(Vec2::new(12.0, 9.0))
                        .hint_text("Access token"),
                );
                super::super::forget_undo(ui.ctx(), field.id);
                if !std::mem::replace(&mut self.token_focused, true) {
                    field.request_focus();
                }
                ui.horizontal(|ui| {
                    widgets::checkbox(ui, &mut self.remember, "Save it in Git’s credential helper");
                    if let Some(page) = token_page(host, origin) {
                        ui.hyperlink_to(RichText::new("Create a token").size(13.0), page);
                    }
                });
                if field.lost_focus() && enter && !self.token.trim().is_empty() {
                    self.start(ui.ctx());
                }
            });
    }
}

/// Where a personal access token is made, for the hosts whose page is known.
fn token_page(host: &str, origin: &str) -> Option<String> {
    match host {
        "github.com" => Some("https://github.com/settings/personal-access-tokens/new".into()),
        "gitlab.com" => Some(format!("{origin}/-/user_settings/personal_access_tokens")),
        _ => None,
    }
}

/// A host's short name in a small pill, so the link's target reads at a glance.
fn provider_badge(ui: &mut Ui, host: &str) {
    let name = match host {
        "github.com" => "GitHub",
        "gitlab.com" => "GitLab",
        "bitbucket.org" => "Bitbucket",
        host => host,
    };
    Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 30))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(name).size(12.0).strong().color(theme::ACCENT()));
        });
}

/// The footer while the repository is being chosen: Continue in place of Start cloud.
pub(in super::super) fn continue_footer(ui: &mut Ui, form: &mut Production, actions: &mut Actions) {
    let next = if form.launch.loading() {
        Err("Reading the repository’s cloud settings…")
    } else {
        form.source.next_step()
    };
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 330.0).max(160.0));
            let (Ok(text) | Err(text)) = next;
            ui.label(RichText::new(text).size(13.0).color(theme::FG_SOFT()));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let go = ui
                .add_enabled(
                    next.is_ok(),
                    Button::new(RichText::new("Continue").size(14.0).strong().color(theme::BG()))
                        .fill(theme::ACCENT())
                        .min_size(Vec2::new(120.0, 40.0))
                        .corner_radius(10),
                )
                .clicked();
            if go {
                form.source.start(ui.ctx());
            }
            actions.cancel |= ui
                .add(
                    Button::new(RichText::new("Cancel").size(14.0))
                        .min_size(Vec2::new(120.0, 40.0))
                        .corner_radius(10),
                )
                .clicked();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::token_page;

    #[test]
    fn a_token_page_is_offered_only_where_it_is_known() {
        assert_eq!(
            token_page("gitlab.com", "https://gitlab.com").as_deref(),
            Some("https://gitlab.com/-/user_settings/personal_access_tokens")
        );
        assert_eq!(
            token_page("github.com", "https://github.com").as_deref(),
            Some("https://github.com/settings/personal-access-tokens/new")
        );
        assert_eq!(token_page("bitbucket.org", "https://bitbucket.org"), None);
        assert_eq!(token_page("git.example.org", "https://git.example.org:8443"), None);
    }
}
