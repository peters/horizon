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
        siblings::{self, Binding, Candidate, Review},
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
        self.wanted = self.key(repository, revision, profile);
        if self
            .reviewing
            .as_ref()
            .is_some_and(|(key, _)| Some(key) != self.wanted.as_ref())
        {
            self.reviewing = None;
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

    /// The checked siblings, in declaration order, once their review for exactly this
    /// primary, revision and profile passed.
    /// # Errors
    /// A checked sibling is unreviewed or refused.
    pub(in crate::app::cloud_panel::production) fn launch_bindings(
        &self,
        repository: &str,
        revision: Option<&str>,
        profile: &str,
    ) -> Result<Vec<Binding>, &'static str> {
        let Some(key) = self.key(repository, revision.unwrap_or_default(), profile) else {
            return Ok(Vec::new());
        };
        if self.passed(&key) {
            Ok(key.bindings)
        } else {
            Err(NOT_REVIEWED)
        }
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
        // A reread of the same checkout keeps choices whose declaration did not change.
        let same_primary = self.source.as_ref().is_some_and(|(primary, _)| primary == repository);
        let previous = std::mem::take(&mut self.rows);
        self.rows = config
            .same_worker_siblings()
            .map(|(alias, declaration)| {
                let kept = previous
                    .iter()
                    .find(|row| same_primary && row.alias == alias && row.repository == declaration.repository);
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
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use std::process::Command;

    const PROFILES: &str = "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .\n      dockerfile: Dockerfile\n";
    const COMPANIONS: &str = "companions:\n  consumer:\n    repository: example/consumer\n    profile: dev\n    placement: same_worker\n  tool:\n    repository: example/tool\n    profile: dev\n    placement: same_worker\n  service:\n    repository: example/service\n    profile: dev\n";

    fn git(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn checkout(path: &Path, origin: &str, config: &str) -> String {
        std::fs::create_dir_all(path.join(".horizon")).unwrap();
        git(path, &["init", "--quiet"]);
        git(path, &["remote", "add", "origin", origin]);
        std::fs::write(path.join(".horizon/cloud.yml"), config).unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "--quiet", "-m", "Add fixture"]);
        git(path, &["rev-parse", "HEAD"])
    }

    /// `app` declaring `consumer` and `tool`, with only `consumer` checked out beside it.
    struct Fixture {
        root: tempfile::TempDir,
        config: CloudConfig,
        revision: String,
        ctx: egui::Context,
        state: State,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let yaml = format!("{PROFILES}{COMPANIONS}");
            let revision = checkout(&root.path().join("app"), "https://github.com/example/app.git", &yaml);
            checkout(
                &root.path().join("consumer"),
                "git@github.com:example/consumer.git",
                PROFILES,
            );
            Self {
                root,
                config: CloudConfig::parse(&yaml).unwrap(),
                revision,
                ctx: egui::Context::default(),
                state: State::default(),
            }
        }

        fn path(&self, name: &str) -> String {
            self.root
                .path()
                .join(name)
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        }

        fn sync_primary(&mut self, primary: &str) {
            let primary = self.path(primary);
            self.state
                .sync(&self.ctx, &primary, Some(&self.revision), Some(&self.config), "dev");
        }

        /// Syncs until suggestions and the review of the current choice have arrived.
        fn settle(&mut self) {
            for _ in 0..500 {
                self.sync_primary("app");
                if self.state.suggestions.is_none() && self.state.reviewing.is_none() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("sibling jobs did not finish");
        }

        fn row(&mut self, alias: &str) -> &mut Row {
            self.state.rows.iter_mut().find(|row| row.alias == alias).unwrap()
        }

        fn status(&mut self, alias: &str) -> Status {
            let row = self.state.rows.iter().find(|row| row.alias == alias).unwrap();
            self.state.status(row)
        }
    }

    #[test]
    fn declared_siblings_start_unchecked_with_a_neighbouring_checkout_suggested() {
        let mut fixture = Fixture::new();
        fixture.settle();
        let rows: Vec<_> = fixture
            .state
            .rows
            .iter()
            .map(|row| (row.alias.as_str(), row.repository.as_str(), row.chosen))
            .collect();
        assert_eq!(
            rows,
            [("consumer", "example/consumer", false), ("tool", "example/tool", false)],
            "a separate-cloud companion is not a sibling"
        );
        let consumer = fixture.path("consumer");
        assert_eq!(fixture.row("consumer").path, consumer);
        assert!(fixture.row("tool").path.is_empty());
        assert!(!fixture.state.blocks_launch());
        let primary = fixture.path("app");
        assert_eq!(
            fixture.state.launch_bindings(&primary, Some(&fixture.revision), "dev"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_checked_sibling_blocks_launch_until_its_review_passes() {
        let mut fixture = Fixture::new();
        fixture.settle();
        fixture.row("consumer").chosen = true;
        fixture.sync_primary("app");
        assert!(matches!(fixture.status("consumer"), Status::Checking));
        assert!(fixture.state.blocks_launch());
        let primary = fixture.path("app");
        let revision = fixture.revision.clone();
        assert_eq!(
            fixture.state.launch_bindings(&primary, Some(&revision), "dev"),
            Err(NOT_REVIEWED)
        );
        fixture.settle();
        assert!(!fixture.state.blocks_launch());
        let head = git(fixture.root.path().join("consumer").as_path(), &["rev-parse", "HEAD"]);
        assert!(matches!(fixture.status("consumer"), Status::Ready(shown) if head.starts_with(&shown)));
        let expected = vec![Binding {
            alias: "consumer".into(),
            local_repository: fixture.path("consumer").into(),
        }];
        assert_eq!(
            fixture.state.launch_bindings(&primary, Some(&revision), "dev"),
            Ok(expected)
        );
        assert_eq!(
            fixture.state.launch_bindings(&primary, Some(&"b".repeat(40)), "dev"),
            Err(NOT_REVIEWED),
            "a review covers only the revision it read"
        );
    }

    #[test]
    fn a_refused_sibling_names_its_own_problem_and_keeps_blocking() {
        let mut fixture = Fixture::new();
        fixture.settle();
        let consumer = fixture.path("consumer");
        let tool = fixture.row("tool");
        tool.chosen = true;
        tool.path = consumer;
        fixture.settle();
        assert!(
            matches!(fixture.status("tool"), Status::Refused(refusal) if refusal.contains("example/consumer") && refusal.contains("example/tool"))
        );
        assert!(matches!(fixture.status("consumer"), Status::Unchecked));
        assert!(fixture.state.blocks_launch());
        fixture.row("tool").path = "tool".into();
        fixture.settle();
        assert!(matches!(fixture.status("tool"), Status::Refused(refusal) if refusal.contains("absolute path")));
        fixture.row("tool").chosen = false;
        fixture.sync_primary("app");
        assert!(!fixture.state.blocks_launch(), "unchecking a refused sibling unblocks");
    }

    #[test]
    fn rereading_keeps_choices_only_for_the_same_primary() {
        let mut fixture = Fixture::new();
        fixture.settle();
        fixture.row("consumer").chosen = true;
        fixture.row("tool").path = "/synthetic/tool".into();
        fixture.row("tool").edited = true;
        std::fs::write(fixture.root.path().join("app/notes.txt"), "later").unwrap();
        let app = fixture.root.path().join("app");
        git(&app, &["add", "."]);
        git(&app, &["commit", "--quiet", "-m", "Later"]);
        fixture.revision = git(&app, &["rev-parse", "HEAD"]);
        fixture.sync_primary("app");
        assert!(fixture.row("consumer").chosen);
        assert_eq!(fixture.row("tool").path, "/synthetic/tool");
        checkout(
            &fixture.root.path().join("other"),
            "https://github.com/example/other.git",
            &format!("{PROFILES}{COMPANIONS}"),
        );
        fixture.sync_primary("other");
        assert!(!fixture.row("consumer").chosen, "another primary authorizes nothing");
        assert!(fixture.row("tool").path.is_empty());
    }

    #[test]
    fn browsing_fills_the_checkout_of_the_sibling_being_browsed() {
        let mut fixture = Fixture::new();
        fixture.settle();
        assert!(!fixture.state.choose_checkout(Path::new("/synthetic/elsewhere")));
        assert_eq!(fixture.state.seed("tool"), Some(PathBuf::from(fixture.path("app"))));
        fixture.state.browse("tool".into());
        assert!(fixture.state.choose_checkout(Path::new("/synthetic/tool")));
        assert_eq!(fixture.row("tool").path, "/synthetic/tool");
        assert_eq!(fixture.state.seed("tool"), Some(PathBuf::from("/synthetic/tool")));
        fixture.state.suggest(vec![Candidate {
            alias: "tool".into(),
            repository: "example/tool".into(),
            directory: "tool".into(),
            suggested: Some("/synthetic/suggested".into()),
        }]);
        assert_eq!(fixture.row("tool").path, "/synthetic/tool", "a chosen path stays");
        fixture.state.browse("tool".into());
        fixture.state.stop_browsing();
        assert!(!fixture.state.choose_checkout(Path::new("/synthetic/late")));
    }

    #[test]
    fn launch_waits_for_the_review_and_the_cloud_keeps_the_chosen_siblings() {
        use crate::test_egui::DiscardTextures;
        let fixture = Fixture::new();
        let (_temp, mut app) = test_app();
        let ctx = egui::Context::default();
        let form = &mut app.cloud_prototype.production;
        form.creating = true;
        form.title = "Siblings".into();
        form.repository = fixture.path("app");
        form.profiles = Some(fixture.config.clone());
        form.selected_profile = "dev".into();
        form.launch.revision = Some(fixture.revision.clone());
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 900.0))),
            ..Default::default()
        };
        let render = |app: &mut crate::app::HorizonApp| {
            let _ = ctx
                .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
                .discard_textures();
        };
        let settle = |app: &mut crate::app::HorizonApp| {
            for _ in 0..500 {
                render(app);
                let siblings = &app.cloud_prototype.production.launch.siblings;
                if siblings.suggestions.is_none() && siblings.reviewing.is_none() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("sibling jobs did not finish");
        };
        settle(&mut app);
        assert!(ctx.read_response(path_id("consumer")).is_some(), "the row is shown");
        let siblings = &mut app.cloud_prototype.production.launch.siblings;
        siblings
            .rows
            .iter_mut()
            .find(|row| row.alias == "consumer")
            .unwrap()
            .chosen = true;
        render(&mut app);
        assert!(!super::super::can_submit(&app.cloud_prototype.production));
        assert!(
            app.create_production_cloud(&ctx)
                .unwrap_err()
                .to_string()
                .contains("chosen siblings")
        );
        settle(&mut app);
        assert!(super::super::can_submit(&app.cloud_prototype.production));
        app.create_production_cloud(&ctx).unwrap();
        for _ in 0..500 {
            app.poll_cloud_creation(&ctx);
            if app.cloud_prototype.production.pending_creation.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
        assert!(
            app.cloud_prototype.production.launch.siblings.rows.is_empty(),
            "the next cloud starts unchecked"
        );
        assert_eq!(
            app.cloud_prototype.groups.0[0].siblings,
            [Binding {
                alias: "consumer".into(),
                local_repository: fixture.path("consumer").into(),
            }]
        );
    }
}
