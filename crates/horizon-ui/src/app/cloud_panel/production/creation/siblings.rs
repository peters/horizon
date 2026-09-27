//! Same-worker siblings chosen in New cloud: one row per sibling the loaded configuration
//! declares, reviewed off the UI thread whenever the choice changes. A declaration only
//! offers a sibling; checking its row authorizes it, so every row starts unchecked.
use crate::theme;
use egui::{Button, Id, RichText, Stroke, TextEdit, Ui, Vec2};
use horizon_core::{
    cloud_panel::CloudConfig,
    cloud_runtime::{
        self, Cancellation,
        command::Runner,
        siblings::{self, Binding, Candidate, Review, Sibling},
    },
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::Duration,
};

const NOT_REVIEWED: &str = "Wait until the chosen siblings are checked, and fix any they report";

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    /// The primary checkout and committed revision whose configuration declares `rows`.
    source: Option<(String, String)>,
    rows: Vec<Row>,
    suggestions: Option<Job<Vec<Candidate>>>,
    reviewing: Option<(Key, Job<Review>)>,
    reviewed: Option<(Key, Result<Review, String>)>,
    /// The choice as of the last sync, while any row is checked.
    wanted: Option<Key>,
    /// The sibling whose checkout the open directory picker chooses.
    browsing: Option<String>,
}

struct Row {
    alias: String,
    repository: String,
    profile: String,
    chosen: bool,
    path: String,
    /// Typed or browsed, so a suggestion arriving later leaves it alone.
    edited: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    repository: String,
    revision: String,
    profile: String,
    bindings: Vec<Binding>,
}

struct Job<T> {
    receiver: Receiver<cloud_runtime::Result<T>>,
    cancel: Cancellation,
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl<T: Send + 'static> Job<T> {
    fn spawn(ctx: &egui::Context, work: impl FnOnce(&Runner<'_>) -> cloud_runtime::Result<T> + Send + 'static) -> Self {
        let cancel = Cancellation::default();
        let worker = cancel.clone();
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let runner = Runner {
                cancel: &worker,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let _ = sender.send(work(&runner));
            ctx.request_repaint();
        });
        Self { receiver, cancel }
    }

    /// `None` while the job runs.
    fn poll(&self) -> Option<cloud_runtime::Result<T>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(cloud_runtime::Error::Invalid("Sibling check interrupted"))),
        }
    }
}

enum Status {
    Unchecked,
    Checking,
    Ready(String),
    Refused(String),
}

impl State {
    /// Keeps the rows on the loaded configuration and the review on the current choice.
    /// `revision` is the committed revision `config` was read at.
    pub(super) fn sync(
        &mut self,
        ctx: &egui::Context,
        repository: &str,
        revision: Option<&str>,
        config: Option<&CloudConfig>,
        profile: &str,
    ) {
        let (Some(config), Some(revision)) = (config, revision) else {
            // While the primary is being read again, a checked choice keeps blocking launch
            // until it is reviewed against what that read returns.
            self.forget_review();
            return;
        };
        if self
            .source
            .as_ref()
            .is_none_or(|(primary, read)| primary != repository || read != revision)
        {
            self.load(ctx, repository, revision, config);
        }
        if let Some(result) = self.suggestions.as_ref().and_then(Job::poll) {
            self.suggestions = None;
            // Only cancellation fails; an unreadable neighbour is simply not suggested.
            if let Ok(candidates) = result {
                self.suggest(candidates);
            }
        }
        let wanted = self.key(repository, revision, profile);
        if wanted != self.wanted {
            // A review describes the checkouts when it ran; any change of choice, including
            // unchecking and checking again, reads them again.
            self.forget_review();
            self.wanted = wanted;
        }
        if let Some(result) = self.reviewing.as_ref().and_then(|(_, job)| job.poll())
            && let Some((key, _)) = self.reviewing.take()
        {
            self.reviewed = Some((key, result.map_err(|error| error.to_string())));
        }
        if let Some(wanted) = &self.wanted
            && self.reviewing.is_none()
            && self.reviewed.as_ref().is_none_or(|(key, _)| key != wanted)
            && let Some(selected) = config.profiles.get(profile)
        {
            let primary = PathBuf::from(repository);
            let config = config.clone();
            let selected = selected.clone();
            let bindings = wanted.bindings.clone();
            let job = Job::spawn(ctx, move |runner| {
                siblings::review(&primary, &config, &selected, &bindings, runner)
            });
            self.reviewing = Some((wanted.clone(), job));
        }
        if self.reviewing.is_some() || self.suggestions.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Whether a checked sibling is still being reviewed or was refused.
    pub(super) fn blocks_launch(&self) -> bool {
        self.wanted.as_ref().is_some_and(|wanted| !self.passed(wanted))
    }

    /// The checked siblings as reviewed, in declaration order, once their review for exactly
    /// this primary, revision and profile passed. Each carries the commit it was reviewed at.
    /// # Errors
    /// A checked sibling is unreviewed or refused.
    pub(in crate::app::cloud_panel::production) fn launch_siblings(
        &self,
        repository: &str,
        revision: Option<&str>,
        profile: &str,
    ) -> Result<Vec<Sibling>, &'static str> {
        let Some(key) = self.key(repository, revision.unwrap_or_default(), profile) else {
            return Ok(Vec::new());
        };
        let Some((reviewed, Ok(review))) = &self.reviewed else {
            return Err(NOT_REVIEWED);
        };
        if *reviewed != key || !review.passed() {
            return Err(NOT_REVIEWED);
        }
        key.bindings
            .iter()
            .map(|binding| match review.siblings.get(&binding.alias) {
                Some(Ok(sibling)) => Ok(sibling.clone()),
                _ => Err(NOT_REVIEWED),
            })
            .collect()
    }

    /// Discards the review, so the current choice is read again before it can launch.
    pub(in crate::app::cloud_panel::production) fn forget_review(&mut self) {
        self.reviewing = None;
        self.reviewed = None;
    }

    pub(super) fn browse(&mut self, alias: String) {
        self.browsing = Some(alias);
    }

    /// Applies a directory chosen while browsing for a sibling. Returns whether one was.
    pub(super) fn choose_checkout(&mut self, path: &Path) -> bool {
        let Some(alias) = self.browsing.take() else {
            return false;
        };
        if let Some(row) = self.rows.iter_mut().find(|row| row.alias == alias) {
            row.path = path.to_string_lossy().into_owned();
            row.edited = true;
        }
        true
    }

    /// The directory picker closed without choosing.
    pub(super) fn stop_browsing(&mut self) {
        self.browsing = None;
    }

    /// Where to start browsing for `alias`: its current path, or the primary checkout.
    pub(super) fn seed(&self, alias: &str) -> Option<PathBuf> {
        let row = self.rows.iter().find(|row| row.alias == alias)?;
        if row.path.trim().is_empty() {
            self.source.as_ref().map(|(primary, _)| PathBuf::from(primary))
        } else {
            Some(horizon_core::Config::expand_tilde(row.path.trim()))
        }
    }

    fn load(&mut self, ctx: &egui::Context, repository: &str, revision: &str, config: &CloudConfig) {
        // A reread of the same checkout keeps only choices whose declaration, repository and
        // profile alike, did not change; anything else must be authorized again.
        let same_primary = self.source.as_ref().is_some_and(|(primary, _)| primary == repository);
        let previous = std::mem::take(&mut self.rows);
        self.rows = config
            .same_worker_siblings()
            .map(|(alias, declaration)| {
                let kept = previous.iter().find(|row| {
                    same_primary
                        && row.alias == alias
                        && row.repository == declaration.repository
                        && row.profile == declaration.profile
                });
                Row {
                    alias: alias.to_owned(),
                    repository: declaration.repository.clone(),
                    profile: declaration.profile.clone(),
                    chosen: kept.is_some_and(|row| row.chosen),
                    path: kept.map(|row| row.path.clone()).unwrap_or_default(),
                    edited: kept.is_some_and(|row| row.edited),
                }
            })
            .collect();
        self.source = Some((repository.to_owned(), revision.to_owned()));
        self.reviewing = None;
        self.reviewed = None;
        self.suggestions = (!self.rows.is_empty()).then(|| {
            let primary = PathBuf::from(repository);
            let config = config.clone();
            Job::spawn(ctx, move |runner| siblings::candidates(&primary, &config, runner))
        });
    }

    fn suggest(&mut self, candidates: Vec<Candidate>) {
        for candidate in candidates {
            if let Some(suggested) = candidate.suggested
                && let Some(row) = self.rows.iter_mut().find(|row| row.alias == candidate.alias)
                && !row.edited
                && row.path.is_empty()
            {
                row.path = suggested.to_string_lossy().into_owned();
            }
        }
    }

    fn key(&self, repository: &str, revision: &str, profile: &str) -> Option<Key> {
        let bindings: Vec<_> = self
            .rows
            .iter()
            .filter(|row| row.chosen)
            .map(|row| Binding {
                alias: row.alias.clone(),
                local_repository: horizon_core::Config::expand_tilde(row.path.trim()),
                revision: None,
            })
            .collect();
        (!bindings.is_empty()).then(|| Key {
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            profile: profile.to_owned(),
            bindings,
        })
    }

    fn passed(&self, wanted: &Key) -> bool {
        matches!(&self.reviewed, Some((key, Ok(review))) if key == wanted && review.passed())
    }

    fn current(&self) -> Option<&Result<Review, String>> {
        let wanted = self.wanted.as_ref()?;
        self.reviewed
            .as_ref()
            .filter(|(key, _)| key == wanted)
            .map(|(_, review)| review)
    }

    /// A refusal of the whole choice, or a check that could not run.
    fn refusal(&self) -> Option<String> {
        match self.current()? {
            Ok(review) => review.choice.as_ref().map(ToString::to_string),
            Err(error) => Some(error.clone()),
        }
    }

    fn status(&self, row: &Row) -> Status {
        if !row.chosen {
            return Status::Unchecked;
        }
        match self.current() {
            None => Status::Checking,
            Some(Err(_)) => Status::Unchecked,
            Some(Ok(review)) => match review.siblings.get(&row.alias) {
                Some(Ok(sibling)) => Status::Ready(sibling.revision.chars().take(12).collect()),
                Some(Err(refusal)) => Status::Refused(refusal.to_string()),
                None => Status::Unchecked,
            },
        }
    }
}

/// The "Siblings on this worker" section, when the configuration declares any. Returns the
/// sibling whose checkout to browse for.
pub(super) fn section(ui: &mut Ui, state: &mut State) -> Option<String> {
    if state.rows.is_empty() {
        return None;
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new("Siblings on this worker")
            .size(14.0)
            .strong()
            .color(theme::FG()),
    );
    ui.label(
        RichText::new(
            "Checked repositories are checked out beside this one and built into its image from their committed HEAD.",
        )
        .size(12.0)
        .color(theme::FG_SOFT()),
    );
    if let Some(refusal) = state.refusal() {
        ui.colored_label(theme::PALETTE_RED(), refusal);
    }
    let statuses: Vec<Status> = state.rows.iter().map(|row| state.status(row)).collect();
    let mut browse = None;
    for (row, status) in state.rows.iter_mut().zip(statuses) {
        let id = Id::new(("cloud-sibling", row.alias.as_str()));
        ui.push_id(id, |ui| {
            if row_ui(ui, row, &status) {
                browse = Some(row.alias.clone());
            }
        });
    }
    browse
}

/// Returns whether to browse for the row's checkout.
fn row_ui(ui: &mut Ui, row: &mut Row, status: &Status) -> bool {
    ui.add_space(4.0);
    choice(ui, &mut row.chosen, &row.repository);
    ui.label(
        RichText::new(format!("{} · profile {}", row.alias, row.profile))
            .size(12.0)
            .color(theme::FG_SOFT()),
    );
    let browse = ui
        .horizontal(|ui| {
            let button = 88.0;
            let width = (ui.available_width() - button - ui.spacing().item_spacing.x).max(120.0);
            let path = ui.add_sized(
                [width, 32.0],
                TextEdit::singleline(&mut row.path)
                    .id(path_id(&row.alias))
                    .margin(Vec2::new(10.0, 8.0))
                    .hint_text("Local checkout path"),
            );
            if path.changed() {
                row.edited = true;
            }
            ui.add(
                Button::new(RichText::new("Browse…").size(13.0))
                    .min_size(Vec2::new(button, 32.0))
                    .corner_radius(8),
            )
            .clicked()
        })
        .inner;
    match status {
        Status::Unchecked => {}
        Status::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Checking…").size(12.0).color(theme::FG_SOFT()));
            });
        }
        Status::Ready(revision) => {
            ui.label(
                RichText::new(format!("Ready at commit {revision}"))
                    .size(12.0)
                    .color(theme::PALETTE_GREEN()),
            );
        }
        Status::Refused(refusal) => {
            ui.colored_label(theme::PALETTE_RED(), refusal);
        }
    }
    browse
}

/// A checkbox whose checked state reads at a glance on the dark dialog: an outlined box
/// while unchecked, an accent-filled box with a bright mark once checked.
fn choice(ui: &mut Ui, chosen: &mut bool, repository: &str) {
    ui.scope(|ui| {
        let checked = *chosen;
        let widgets = &mut ui.visuals_mut().widgets;
        for visuals in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            visuals.bg_stroke = Stroke::new(1.5, if checked { theme::ACCENT() } else { theme::FG_SOFT() });
            if checked {
                visuals.bg_fill = theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.6);
                visuals.fg_stroke = Stroke::new(2.0, theme::FG());
            }
        }
        ui.checkbox(chosen, RichText::new(repository).size(14.0).color(theme::FG()));
    });
}

fn path_id(alias: &str) -> Id {
    Id::new(("cloud-sibling-path", alias))
}

#[cfg(all(test, unix))]
mod tests;
