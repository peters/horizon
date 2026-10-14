//! The Repositories editor of an Upkeep board: the repository picker of New Upkeep over the
//! Upkeep's saved list. Save changes only what the person changed, on the list saved at that
//! moment, so a change made meanwhile by another part of Horizon stays.
use std::collections::BTreeSet;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{Align, Context, Frame, Id, Layout, Order, RichText, Stroke, Vec2};
use horizon_core::WorkspaceId;
use horizon_core::upkeep::worker::{self, Config};

use super::form::{Bot, Form};
use crate::app::HorizonApp;
use crate::theme;

pub(in crate::app) struct Editor {
    pub(in crate::app) cloud: String,
    /// The list when the editor opened; Save sends the difference to it.
    original: BTreeSet<String>,
    pub(in crate::app) form: Form,
    /// The repositories an agent proposed through `cloud_upkeep`, shown as their own rows:
    /// the list below may not hold them.
    pub(in crate::app) proposed: Vec<String>,
    /// The workspace of the Upkeep the agent proposed them for, as the editor names it: it
    /// can open while the person looks at another workspace.
    pub(in crate::app) proposed_for: String,
    saving: Option<Receiver<Result<(), String>>>,
    error: Option<String>,
}

impl Editor {
    pub(in crate::app) fn new(cloud: &str, config: &Config) -> Self {
        let original: BTreeSet<String> = config.repositories.iter().map(|name| name.to_lowercase()).collect();
        let mut form = Form::default();
        form.keep_unlisted = true;
        form.selected.clone_from(&original);
        form.bots = config
            .authors
            .iter()
            .filter_map(|author| Bot::from_author(author))
            .collect();
        if form.bots.is_empty() {
            form.bots.insert(Bot::Dependabot);
        }
        Self {
            cloud: cloud.to_owned(),
            original,
            form,
            proposed: Vec::new(),
            proposed_for: String::new(),
            saving: None,
            error: None,
        }
    }

    /// The repositories to add and to remove.
    pub(in crate::app) fn changes(&self) -> (Vec<String>, Vec<String>) {
        let added = self.form.selected.difference(&self.original).cloned().collect();
        let removed = self.original.difference(&self.form.selected).cloned().collect();
        (added, removed)
    }

    /// The chosen repositories that the app does not list now, once a search answered.
    /// They stay chosen, also through Clear.
    fn unlisted(&self) -> Vec<&str> {
        // Without an answer, or after a failed search, nothing is known to be unlisted.
        if self.form.finding.is_some() || self.form.find_error.is_some() || self.form.codex_login.is_none() {
            return Vec::new();
        }
        let listed = &self.form.catalog.repositories;
        // Only saved ones: an agent's proposal shows in its own box and is not saved yet.
        self.form
            .selected
            .intersection(&self.original)
            .filter(|name| !listed.iter().any(|repository| repository.name == **name))
            .map(String::as_str)
            .collect()
    }

    /// Why Save cannot save now, or `None` when it can.
    fn blocked(&self) -> Option<&'static str> {
        let (added, removed) = self.changes();
        if self.saving.is_some() {
            Some("Saving...")
        } else if self.form.finding.is_some() {
            // The list grows when the search answers; Save waits, so a click lands on it.
            Some("Finding repositories...")
        } else if self.form.selected.is_empty() {
            Some("An Upkeep keeps at least one repository. Pause it instead.")
        } else if added.is_empty() && removed.is_empty() {
            Some("Nothing changed.")
        } else {
            None
        }
    }
}

enum Outcome {
    Open,
    Cancel,
    Save,
}

impl HorizonApp {
    /// Opens the Repositories editor of the Upkeep of `cloud` and searches for repositories.
    /// Returns false while the saved Upkeeps are being read, so the board asks again.
    pub(super) fn open_upkeep_repositories(&mut self, ctx: &Context, cloud: &str) -> bool {
        let Some(saved) = self.upkeep.saved.as_ref() else {
            return false;
        };
        let Some(config) = saved
            .iter()
            .find(|(held, _)| held == cloud)
            .map(|(_, config)| config.clone())
        else {
            return true;
        };
        let mut editor = Editor::new(cloud, &config);
        if let Some(root) = self.upkeep_root() {
            editor.form.find(root, ctx);
        }
        self.upkeep.editor = Some(editor);
        true
    }

    /// Opens the Repositories editor of the Upkeep of `cloud`, whose saved settings are
    /// `config`, with `add` ticked: an agent's proposal through `cloud_upkeep`, which
    /// changes nothing until the person saves it. Save sends only the person's changes, so
    /// a change saved meanwhile stays.
    /// # Errors
    /// An Upkeep dialog is already open.
    pub(super) fn propose_upkeep_repositories(
        &mut self,
        ctx: &Context,
        workspace: WorkspaceId,
        cloud: &str,
        config: &Config,
        add: &[String],
    ) -> Result<(), String> {
        if self.upkeep.editor.is_some() || self.upkeep.form.is_some() {
            return Err("upkeep_unavailable: an Upkeep dialog is already open; try again later".into());
        }
        // The cloud must still be in the caller's workspace, the one the editor names.
        let held = self
            .board
            .workspace(workspace)
            .ok_or("upkeep_unavailable: the Upkeep's workspace is gone")?;
        let in_workspace = self.cloud_prototype.groups.0.iter().any(|group| {
            group.workspace == held.local_id && group.remote.as_ref().is_some_and(|remote| remote.id == cloud)
        });
        if !in_workspace {
            return Err("upkeep_unavailable: the Upkeep is no longer in this workspace".into());
        }
        let workspace = held.name.clone();
        let mut editor = Editor::new(cloud, config);
        editor.proposed_for = workspace;
        editor.proposed = add.iter().map(|name| name.to_lowercase()).collect();
        editor.form.selected.extend(editor.proposed.iter().cloned());
        // Tests read the developer's real cloud settings: they never search GitHub.
        if !cfg!(test)
            && let Some(root) = self.upkeep_root()
        {
            editor.form.find(root, ctx);
        }
        self.upkeep.editor = Some(editor);
        Ok(())
    }

    pub(in crate::app) fn render_upkeep_editor(&mut self, ctx: &Context) {
        let Some(editor) = self.upkeep.editor.as_mut() else {
            return;
        };
        editor.form.receive();
        editor.form.forget_counts_without_bots();
        if let Some(rx) = &editor.saving {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    let cloud = editor.cloud.clone();
                    self.upkeep.editor = None;
                    self.upkeep_settings_saved(&cloud);
                    return;
                }
                Ok(Err(error)) => {
                    editor.saving = None;
                    editor.error = Some(error);
                }
                Err(TryRecvError::Disconnected) => {
                    editor.saving = None;
                    editor.error = Some("Saving stopped.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if editor.form.stale()
            && let Some(root) = self.upkeep_root()
            && let Some(editor) = self.upkeep.editor.as_mut()
        {
            editor.form.find(root, ctx);
        }
        let Some(editor) = self.upkeep.editor.as_mut() else {
            return;
        };
        match show(ctx, editor) {
            Outcome::Open => {}
            Outcome::Cancel => self.upkeep.editor = None,
            Outcome::Save => {
                let (added, removed) = editor.changes();
                let cloud = editor.cloud.clone();
                let (tx, rx) = channel();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let saved = worker::update(&cloud, |config| {
                        config.remove(&removed);
                        config.add(&added);
                    })
                    .map(|_| ())
                    .map_err(|error| format!("{error}."));
                    let _ = tx.send(saved);
                    repaint.request_repaint();
                });
                editor.error = None;
                editor.saving = Some(rx);
            }
        }
    }
}

fn show(ctx: &Context, editor: &mut Editor) -> Outcome {
    let viewport = ctx.content_rect();
    let width = (viewport.width() - 64.0).clamp(240.0, 760.0);
    let mut outcome = Outcome::Open;
    let id = Id::new("upkeep-repositories-editor");
    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).order(Order::Tooltip))
        .frame(
            Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                .corner_radius(16)
                .inner_margin(24),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
            ui.label(RichText::new("Repositories").size(22.0).strong().color(theme::FG()));
            ui.label(
                RichText::new("The next run uses the new list. Runs in progress keep theirs.")
                    .size(13.0)
                    .color(theme::FG_SOFT()),
            );
            ui.add_space(8.0);
            ui.add_enabled_ui(editor.saving.is_none(), |ui| {
                proposed(ui, editor);
                super::repositories::show(ui, &mut editor.form);
            });
            let unlisted = editor.unlisted();
            if !unlisted.is_empty() {
                let count = unlisted.len();
                ui.label(
                    RichText::new(format!(
                        "{count} saved {} that the app does not list now {} chosen.",
                        if count == 1 { "repository" } else { "repositories" },
                        if count == 1 { "stays" } else { "stay" },
                    ))
                    .size(12.5)
                    .color(theme::FG_SOFT()),
                )
                .on_hover_text(unlisted.join("\n"));
            }
            ui.add_space(10.0);
            let (added, removed) = editor.changes();
            let blocked = editor.blocked();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let save = egui::Button::new(RichText::new("Save").size(14.0).strong().color(theme::BG()))
                    .fill(theme::ACCENT())
                    .corner_radius(10)
                    .min_size(Vec2::new(120.0, 38.0));
                if ui
                    .add_enabled(blocked.is_none(), save)
                    .on_disabled_hover_text(blocked.unwrap_or_default())
                    .clicked()
                {
                    outcome = Outcome::Save;
                }
                let cancel = egui::Button::new(RichText::new("Cancel").size(14.0))
                    .corner_radius(10)
                    .min_size(Vec2::new(110.0, 38.0));
                // A save under way finishes first, so its result reaches the exchange.
                if ui.add_enabled(editor.saving.is_none(), cancel).clicked() {
                    outcome = Outcome::Cancel;
                }
                if let Some(error) = &editor.error {
                    ui.add(egui::Label::new(RichText::new(error).size(12.5).color(theme::PALETTE_RED())).truncate())
                        .on_hover_text(error);
                } else {
                    ui.label(
                        RichText::new(summary(added.len(), removed.len(), editor.form.selected.len()))
                            .size(12.5)
                            .color(theme::FG_SOFT()),
                    );
                }
            });
        });
    ctx.move_to_top(response.response.layer_id);
    if response.should_close() && matches!(outcome, Outcome::Open) && editor.saving.is_none() {
        outcome = Outcome::Cancel;
    }
    outcome
}

/// The repositories an agent proposed, each with its own tick, so the person sees every
/// name that Save would add even when the list below does not show it.
fn proposed(ui: &mut egui::Ui, editor: &mut Editor) {
    if editor.proposed.is_empty() {
        return;
    }
    Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 24))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "An agent proposes these repositories for the Upkeep of the workspace \u{201c}{}\u{201d}. Untick any you do not want.",
                    editor.proposed_for
                ))
                .size(13.0)
                .color(theme::FG()),
            );
            for name in &editor.proposed {
                let mut on = editor.form.selected.contains(name);
                if ui.checkbox(&mut on, RichText::new(name).size(13.0)).changed() {
                    if on {
                        editor.form.selected.insert(name.clone());
                    } else {
                        editor.form.selected.remove(name);
                    }
                }
            }
        });
    ui.add_space(6.0);
}

/// "2 added, 1 removed · 17 repositories"
fn summary(added: usize, removed: usize, total: usize) -> String {
    let total = format!("{total} {}", if total == 1 { "repository" } else { "repositories" });
    match (added, removed) {
        (0, 0) => total,
        (added, 0) => format!("{added} added  ·  {total}"),
        (0, removed) => format!("{removed} removed  ·  {total}"),
        (added, removed) => format!("{added} added, {removed} removed  ·  {total}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            repositories: vec!["Acme/Web".into(), "acme/api".into()],
            authors: vec!["app/renovate".into(), "someone".into()],
            ..Config::default()
        }
    }

    #[test]
    fn save_sends_only_what_the_person_changed() {
        let mut editor = Editor::new("c1", &config());
        assert_eq!(editor.blocked(), Some("Nothing changed."));
        assert_eq!(editor.form.bots.iter().copied().collect::<Vec<_>>(), [Bot::Renovate]);
        editor.form.selected.remove("acme/web");
        editor.form.selected.insert("acme/docs".into());
        assert_eq!(
            editor.changes(),
            (vec!["acme/docs".to_string()], vec!["acme/web".to_string()])
        );
        assert_eq!(editor.blocked(), None);
        assert_eq!(summary(1, 1, 2), "1 added, 1 removed  ·  2 repositories");
        editor.form.selected.clear();
        assert!(editor.blocked().is_some_and(|reason| reason.contains("Pause")));
    }

    #[test]
    fn a_saved_repository_the_app_no_longer_lists_is_not_removed() {
        use horizon_core::upkeep::{Catalog, CatalogRepository};
        let mut editor = Editor::new("c1", &config());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(super::super::form::Found {
            catalog: Catalog {
                repositories: vec![CatalogRepository {
                    name: "acme/api".into(),
                    open: 1,
                }],
                search_error: None,
            },
            codex_login: true,
        }))
        .unwrap();
        editor.form.finding = Some(rx);
        assert_eq!(editor.blocked(), Some("Finding repositories..."));
        editor.form.receive();
        assert_eq!(editor.changes(), (Vec::new(), Vec::new()), "acme/web stays");
        assert_eq!(editor.unlisted(), ["acme/web"]);
        // An agent's proposal that the list does not hold is not called saved.
        editor.proposed = vec!["acme/new".into()];
        editor.form.selected.insert("acme/new".into());
        assert_eq!(editor.unlisted(), ["acme/web"]);
    }

    #[test]
    fn a_change_made_meanwhile_stays_when_the_difference_is_applied() {
        let mut editor = Editor::new("c1", &config());
        editor.form.selected.insert("acme/docs".into());
        let (added, removed) = editor.changes();
        // An agent added acme/tools while the editor was open.
        let mut saved = config();
        saved.add(&["acme/tools".to_string()]);
        saved.remove(&removed);
        saved.add(&added);
        assert_eq!(saved.repositories, ["Acme/Web", "acme/api", "acme/tools", "acme/docs"]);
    }
}
