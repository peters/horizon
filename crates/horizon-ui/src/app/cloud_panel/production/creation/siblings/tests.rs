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

    /// What a launch would pin, as bindings.
    fn bound(&self, primary: &str, revision: Option<&str>, profile: &str) -> Result<Vec<Binding>, &'static str> {
        self.state.launch_siblings(primary, revision, profile).map(|siblings| {
            siblings
                .into_iter()
                .map(|sibling| Binding {
                    alias: sibling.alias,
                    local_repository: sibling.local_repository,
                    revision: Some(sibling.revision),
                })
                .collect()
        })
    }

    fn advance(&self, name: &str) -> String {
        let checkout = self.root.path().join(name);
        std::fs::write(checkout.join("later.txt"), "later").unwrap();
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "--quiet", "-m", "Later"]);
        git(&checkout, &["rev-parse", "HEAD"])
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
    assert_eq!(fixture.bound(&primary, Some(&fixture.revision), "dev"), Ok(Vec::new()));
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
    assert_eq!(fixture.bound(&primary, Some(&revision), "dev"), Err(NOT_REVIEWED));
    fixture.settle();
    assert!(!fixture.state.blocks_launch());
    let head = git(fixture.root.path().join("consumer").as_path(), &["rev-parse", "HEAD"]);
    assert!(matches!(fixture.status("consumer"), Status::Ready(shown) if head.starts_with(&shown)));
    let expected = vec![Binding {
        alias: "consumer".into(),
        local_repository: fixture.path("consumer").into(),
        revision: Some(head.clone()),
    }];
    assert_eq!(fixture.bound(&primary, Some(&revision), "dev"), Ok(expected));
    assert_eq!(
        fixture.bound(&primary, Some(&"b".repeat(40)), "dev"),
        Err(NOT_REVIEWED),
        "a review covers only the revision it read"
    );
}

#[test]
fn rereading_the_primary_blocks_a_passed_choice_until_it_is_reviewed_again() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.row("consumer").chosen = true;
    fixture.settle();
    assert!(!fixture.state.blocks_launch());
    let primary = fixture.path("app");
    let ctx = fixture.ctx.clone();
    let config = fixture.config.clone();
    let revision = fixture.revision.clone();
    for (revision, config) in [(None, Some(&config)), (Some(revision.as_str()), None)] {
        fixture.state.sync(&ctx, &primary, revision, config, "dev");
        assert!(fixture.state.blocks_launch(), "a reread must not keep the old review");
    }
    std::fs::write(fixture.root.path().join("app/notes.txt"), "later").unwrap();
    let app = fixture.root.path().join("app");
    git(&app, &["add", "."]);
    git(&app, &["commit", "--quiet", "-m", "Later"]);
    fixture.revision = git(&app, &["rev-parse", "HEAD"]);
    fixture.sync_primary("app");
    assert!(fixture.state.blocks_launch(), "the new revision is not reviewed yet");
    fixture.settle();
    assert!(!fixture.state.blocks_launch());
    assert_eq!(
        fixture
            .bound(&primary, Some(&fixture.revision), "dev")
            .map(|bindings| bindings.len()),
        Ok(1)
    );
    assert_eq!(
        fixture.bound(&primary, Some(&revision), "dev"),
        Err(NOT_REVIEWED),
        "the earlier revision's review is gone"
    );
    fixture.state.sync(&ctx, &primary, None, None, "dev");
    assert!(
        fixture.state.blocks_launch(),
        "choosing another repository blocks while it loads"
    );
}

#[test]
fn checking_again_reviews_the_checkout_at_its_current_commit() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.row("consumer").chosen = true;
    fixture.settle();
    fixture.row("consumer").chosen = false;
    fixture.sync_primary("app");
    let head = fixture.advance("consumer");
    fixture.row("consumer").chosen = true;
    fixture.sync_primary("app");
    assert!(matches!(fixture.status("consumer"), Status::Checking));
    assert!(fixture.state.blocks_launch(), "the earlier review is not reused");
    fixture.settle();
    assert!(matches!(fixture.status("consumer"), Status::Ready(shown) if head.starts_with(&shown)));
    let primary = fixture.path("app");
    let pinned = fixture
        .state
        .launch_siblings(&primary, Some(&fixture.revision), "dev")
        .unwrap();
    assert_eq!(pinned[0].revision, head);
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
fn a_changed_sibling_profile_must_be_checked_again() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.row("consumer").chosen = true;
    fixture.row("tool").chosen = true;
    let app = fixture.root.path().join("app");
    let yaml =
        format!("{PROFILES}{COMPANIONS}").replacen("consumer\n    profile: dev", "consumer\n    profile: release", 1);
    assert_ne!(yaml, format!("{PROFILES}{COMPANIONS}"));
    std::fs::write(app.join(".horizon/cloud.yml"), &yaml).unwrap();
    git(&app, &["commit", "--quiet", "-am", "Use another consumer profile"]);
    fixture.revision = git(&app, &["rev-parse", "HEAD"]);
    fixture.config = CloudConfig::parse(&yaml).unwrap();
    fixture.sync_primary("app");
    assert!(!fixture.row("consumer").chosen, "a new profile is not authorized yet");
    assert_eq!(fixture.row("consumer").profile, "release");
    assert!(fixture.row("tool").chosen, "an unchanged declaration keeps its choice");
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
    let create = |app: &mut crate::app::HorizonApp| {
        app.create_production_cloud(&ctx).unwrap();
        for _ in 0..500 {
            app.poll_cloud_creation(&ctx);
            if app.cloud_prototype.production.pending_creation.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("creation did not finish");
    };
    settle(&mut app);
    assert!(super::super::can_submit(&app.cloud_prototype.production));
    let head = fixture.advance("consumer");
    create(&mut app);
    assert!(
        app.cloud_prototype
            .error
            .as_ref()
            .is_some_and(|error| error.contains("moved to another commit")),
        "{:?}",
        app.cloud_prototype.error
    );
    assert!(
        app.cloud_prototype.groups.0.is_empty(),
        "a moved checkout creates nothing"
    );
    assert!(!super::super::can_submit(&app.cloud_prototype.production));
    settle(&mut app);
    let siblings = &app.cloud_prototype.production.launch.siblings;
    let row = siblings.rows.iter().find(|row| row.alias == "consumer").unwrap();
    assert!(matches!(siblings.status(row), Status::Ready(shown) if head.starts_with(&shown)));
    create(&mut app);
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
            // The cloud records the commit that was reviewed, so deployment pins it.
            revision: Some(head),
        }]
    );
}
