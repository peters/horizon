use super::*;
use crate::cloud_runtime::Cancellation;

const PROFILES: &str = "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .\n      dockerfile: Dockerfile\n  prebuilt:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n";

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

/// A committed checkout with `origin` and `.horizon/cloud.yml`; returns its HEAD.
fn checkout(path: &Path, origin: &str, config: &str) -> String {
    std::fs::create_dir_all(path.join(".horizon")).unwrap();
    git(path, &["init", "--quiet"]);
    git(path, &["remote", "add", "origin", origin]);
    std::fs::write(path.join(".horizon/cloud.yml"), config).unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "Add fixture"]);
    git(path, &["rev-parse", "HEAD"])
}

fn primary_config(companions: &str) -> CloudConfig {
    CloudConfig::parse(&format!("{PROFILES}companions:\n{companions}")).unwrap()
}

const NATIVE: &str = "  native:\n    repository: example/native-lib\n    profile: dev\n    placement: same_worker\n  service:\n    repository: example/service\n    profile: dev\n";

struct Fixture {
    root: tempfile::TempDir,
    cancel: Cancellation,
}

impl Fixture {
    /// `app` from `example/app` beside `native-lib` from `Example/Native-Lib`.
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        checkout(&root.path().join("app"), "https://github.com/example/app.git", PROFILES);
        checkout(
            &root.path().join("native-lib"),
            "git@github.com:Example/Native-Lib.git",
            PROFILES,
        );
        Self {
            root,
            cancel: Cancellation::default(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn runner(&self) -> Runner<'_> {
        Runner {
            cancel: &self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        }
    }

    fn resolve(&self, config: &CloudConfig, profile: &str, bindings: &[(&str, &str)]) -> Result<Set> {
        let bindings: Vec<_> = bindings
            .iter()
            .map(|(alias, path)| Binding {
                alias: (*alias).into(),
                local_repository: self.path(path),
                revision: None,
            })
            .collect();
        resolve(
            &self.path("app"),
            config,
            &config.profiles[profile],
            &bindings,
            &self.runner(),
        )
        .map(|set| set.expect("bindings resolve to a set"))
    }

    fn refusal(&self, config: &CloudConfig, profile: &str, bindings: &[(&str, &str)]) -> SiblingError {
        match self.resolve(config, profile, bindings) {
            Err(Error::Sibling(refusal)) => refusal,
            other => panic!("expected a sibling refusal, got {other:?}"),
        }
    }
}

#[test]
fn resolution_pins_each_chosen_sibling_to_its_committed_head_and_recipe() {
    let fixture = Fixture::new();
    let committed = git(&fixture.path("native-lib"), &["rev-parse", "HEAD"]);
    std::fs::write(fixture.path("native-lib/.horizon/cloud.yml"), "dirty: [").unwrap();
    let set = fixture
        .resolve(&primary_config(NATIVE), "dev", &[("native", "native-lib/.horizon")])
        .unwrap();
    assert_eq!(
        set,
        Set {
            primary_directory: "app".into(),
            members: vec![Sibling {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                revision: committed,
                image_revision: None,
                local_repository: fixture.path("native-lib").canonicalize().unwrap(),
                profile: "dev".into(),
            }],
        }
    );
    let primary = primary_config(NATIVE).profiles["dev"].build.clone().unwrap();
    assert_eq!(
        set.members[0]
            .recipe(&set.members[0].revision, &primary, &fixture.runner())
            .unwrap(),
        primary
    );
}

#[test]
fn resolution_refuses_choices_that_are_not_declared_same_worker_siblings() {
    let fixture = Fixture::new();
    let config = primary_config(NATIVE);
    assert_eq!(
        fixture.refusal(&config, "dev", &[("missing", "native-lib")]),
        SiblingError::Undeclared("missing".into())
    );
    assert_eq!(
        fixture.refusal(&config, "dev", &[("service", "native-lib")]),
        SiblingError::SeparateCloud("service".into())
    );
    assert_eq!(
        fixture.refusal(&config, "dev", &[("native", "native-lib"), ("native", "native-lib")]),
        SiblingError::Duplicate("native".into())
    );
    assert_eq!(
        fixture.refusal(&config, "prebuilt", &[("native", "native-lib")]),
        SiblingError::PrimaryImageOnly
    );
    let many: Vec<_> = (0..=MAX_SIBLINGS).map(|_| ("native", "native-lib")).collect();
    assert_eq!(fixture.refusal(&config, "dev", &many), SiblingError::TooMany);
}

#[test]
fn resolution_refuses_checkouts_from_another_repository_or_the_primary() {
    let fixture = Fixture::new();
    let config = primary_config(NATIVE);
    assert_eq!(
        fixture.refusal(&config, "dev", &[("native", "app")]),
        SiblingError::Primary("native".into())
    );
    checkout(&fixture.path("fork"), "https://github.com/someone/native-lib", PROFILES);
    assert_eq!(
        fixture.refusal(&config, "dev", &[("native", "fork")]),
        SiblingError::OriginMismatch {
            alias: "native".into(),
            declared: "example/native-lib".into(),
            found: "someone/native-lib".into(),
        }
    );
    checkout(&fixture.path("local"), "/srv/native-lib.git", PROFILES);
    assert_eq!(
        fixture.refusal(&config, "dev", &[("native", "local")]),
        SiblingError::Origin("native".into())
    );
    std::fs::create_dir(fixture.path("plain")).unwrap();
    assert_eq!(
        fixture.refusal(&config, "dev", &[("native", "plain")]),
        SiblingError::Origin("native".into())
    );
    let itself =
        primary_config("  itself:\n    repository: Example/App\n    profile: dev\n    placement: same_worker\n");
    assert_eq!(
        fixture.refusal(&itself, "dev", &[("itself", "native-lib")]),
        SiblingError::Primary("itself".into())
    );
    checkout(&fixture.path("other-app"), "https://github.com/other/APP", PROFILES);
    let clash = primary_config("  clash:\n    repository: other/APP\n    profile: dev\n    placement: same_worker\n");
    assert_eq!(
        fixture.refusal(&clash, "dev", &[("clash", "other-app")]),
        SiblingError::Directory("clash".into())
    );
    checkout(&fixture.path("dash"), "https://github.com/example/-lib", PROFILES);
    // Parsing already refuses this; resolution does not rely on it.
    let mut dash =
        primary_config("  dash:\n    repository: example/lib\n    profile: dev\n    placement: same_worker\n");
    dash.companions.get_mut("dash").unwrap().repository = "example/-lib".into();
    assert_eq!(
        fixture.refusal(&dash, "dev", &[("dash", "dash")]),
        SiblingError::DirectoryName("-lib".into())
    );
}

#[test]
fn resolution_requires_a_github_primary_and_a_layerable_sibling_recipe() {
    let fixture = Fixture::new();
    git(
        &fixture.path("app"),
        &["remote", "set-url", "origin", "https://example.com/app.git"],
    );
    assert_eq!(
        fixture.refusal(&primary_config(NATIVE), "dev", &[("native", "native-lib")]),
        SiblingError::PrimaryOrigin
    );
    git(
        &fixture.path("app"),
        &["remote", "set-url", "origin", "https://github.com/example/app"],
    );
    let declared = |profile: &str| {
        primary_config(&format!(
            "  native:\n    repository: example/native-lib\n    profile: {profile}\n    placement: same_worker\n"
        ))
    };
    assert_eq!(
        fixture.refusal(&declared("absent"), "dev", &[("native", "native-lib")]),
        SiblingError::MissingProfile {
            alias: "native".into(),
            profile: "absent".into(),
        }
    );
    assert_eq!(
        fixture.refusal(&declared("prebuilt"), "dev", &[("native", "native-lib")]),
        SiblingError::ImageOnly {
            alias: "native".into(),
            profile: "prebuilt".into(),
        }
    );
    let lib = fixture.path("native-lib");
    git(&lib, &["rm", "--quiet", ".horizon/cloud.yml"]);
    git(&lib, &["commit", "--quiet", "-m", "Remove configuration"]);
    assert_eq!(
        fixture.refusal(&declared("dev"), "dev", &[("native", "native-lib")]),
        SiblingError::MissingConfig("native".into())
    );
    std::fs::create_dir_all(lib.join(".horizon")).unwrap();
    std::fs::write(lib.join(".horizon/cloud.yml"), "version: [").unwrap();
    git(&lib, &["add", "."]);
    git(&lib, &["commit", "--quiet", "-m", "Break configuration"]);
    assert_eq!(
        fixture.refusal(&declared("dev"), "dev", &[("native", "native-lib")]),
        SiblingError::InvalidConfig("native".into())
    );
    std::fs::write(lib.join(".horizon/cloud.yml"), "#".repeat(5 * 1024 * 1024)).unwrap();
    git(&lib, &["add", "."]);
    git(&lib, &["commit", "--quiet", "-m", "Grow configuration"]);
    match fixture.resolve(&declared("dev"), "dev", &[("native", "native-lib")]) {
        Err(Error::Invalid(message)) => assert!(message.contains("exceeded its bound"), "{message}"),
        other => panic!("an operational failure is not a sibling refusal: {other:?}"),
    }
    // Profiles accept only linux/amd64 today; the check keeps a later platform from mixing.
    let revision = git(&lib, &["rev-parse", "HEAD~3"]);
    let arm = Build {
        platform: "linux/arm64".into(),
        ..declared("dev").profiles["dev"].build.clone().unwrap()
    };
    match recipe("native", &lib, &revision, "dev", &arm, &fixture.runner()) {
        Err(Error::Sibling(refusal)) => assert_eq!(
            refusal,
            SiblingError::Platform {
                alias: "native".into(),
                sibling: "linux/amd64".into(),
                primary: "linux/arm64".into(),
            }
        ),
        other => panic!("expected a platform refusal, got {other:?}"),
    }
}

#[test]
fn resolution_refuses_a_sibling_checkout_without_a_commit() {
    let fixture = Fixture::new();
    let empty = fixture.path("empty");
    std::fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "--quiet"]);
    git(
        &empty,
        &["remote", "add", "origin", "https://github.com/example/native-lib"],
    );
    assert_eq!(
        fixture.refusal(&primary_config(NATIVE), "dev", &[("native", "empty")]),
        SiblingError::NoCommit("native".into())
    );
}

#[test]
fn candidates_suggest_only_a_neighbouring_checkout_of_the_declared_repository() {
    let fixture = Fixture::new();
    let config = primary_config(&format!(
        "{NATIVE}  tool:\n    repository: example/tool\n    profile: dev\n    placement: same_worker\n"
    ));
    checkout(&fixture.path("tool"), "https://github.com/someone/tool", PROFILES);
    let listed = candidates(&fixture.path("app/.horizon"), &config, &fixture.runner()).unwrap();
    assert_eq!(
        listed,
        [
            Candidate {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                suggested: Some(fixture.path("native-lib").canonicalize().unwrap()),
            },
            Candidate {
                alias: "tool".into(),
                repository: "example/tool".into(),
                directory: "tool".into(),
                suggested: None,
            },
        ]
    );
    fixture.cancel.cancel();
    assert!(candidates(&fixture.path("app"), &config, &fixture.runner()).is_err());
}

#[test]
fn no_bindings_mean_no_siblings_without_inspecting_the_primary() {
    let fixture = Fixture::new();
    git(&fixture.path("app"), &["remote", "remove", "origin"]);
    let config = primary_config(NATIVE);
    for profile in ["dev", "prebuilt"] {
        let resolved = resolve(
            &fixture.path("app"),
            &config,
            &config.profiles[profile],
            &[],
            &fixture.runner(),
        );
        assert!(matches!(resolved, Ok(None)), "{resolved:?}");
    }
}

#[test]
fn cancellation_is_never_reported_as_a_refusal() {
    let fixture = Fixture::new();
    fixture.cancel.cancel();
    let cancelled = |result: Result<()>| matches!(result, Err(Error::Provider(horizon_cloud::CloudError::Cancelled)));
    assert!(cancelled(
        fixture
            .resolve(&primary_config(NATIVE), "dev", &[("native", "native-lib")])
            .map(|_| ())
    ));
    let lib = fixture.path("native-lib");
    let revision = git(&lib, &["rev-parse", "HEAD"]);
    let primary = primary_config(NATIVE).profiles["dev"].build.clone().unwrap();
    assert!(cancelled(
        recipe("native", &lib, &revision, "dev", &primary, &fixture.runner()).map(|_| ())
    ));
}

#[test]
#[cfg(unix)] // Creating symlinks on Windows needs a privilege CI runners may lack.
fn a_symlinked_binding_names_the_checkout_it_points_to() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(fixture.path("native-lib"), fixture.path("link")).unwrap();
    let set = fixture
        .resolve(&primary_config(NATIVE), "dev", &[("native", "link")])
        .unwrap();
    assert_eq!(
        set.members[0].local_repository,
        fixture.path("native-lib").canonicalize().unwrap()
    );
    std::os::unix::fs::symlink(fixture.path("app"), fixture.path("primary-link")).unwrap();
    assert_eq!(
        fixture.refusal(&primary_config(NATIVE), "dev", &[("native", "primary-link")]),
        SiblingError::Primary("native".into())
    );
}

#[test]
fn two_siblings_cannot_share_a_directory_ignoring_case() {
    let fixture = Fixture::new();
    checkout(&fixture.path("lib-a"), "https://github.com/a/lib", PROFILES);
    checkout(&fixture.path("lib-b"), "https://github.com/b/LIB", PROFILES);
    // Parsing already refuses this; resolution does not rely on it.
    let mut config = primary_config("  first:\n    repository: a/lib\n    profile: dev\n    placement: same_worker\n");
    let mut second = config.companions["first"].clone();
    second.repository = "b/LIB".into();
    config.companions.insert("second".into(), second);
    assert_eq!(
        fixture.refusal(&config, "dev", &[("first", "lib-a"), ("second", "lib-b")]),
        SiblingError::Directory("second".into())
    );
}

#[test]
fn a_pinned_commit_the_checkout_lost_is_named_as_such() {
    let fixture = Fixture::new();
    let primary = primary_config(NATIVE).profiles["dev"].build.clone().unwrap();
    match recipe(
        "native",
        &fixture.path("native-lib"),
        &"0".repeat(40),
        "dev",
        &primary,
        &fixture.runner(),
    ) {
        Err(Error::Sibling(refusal)) => assert_eq!(refusal, SiblingError::RevisionUnavailable("native".into())),
        other => panic!("expected a lost-commit refusal, got {other:?}"),
    }
}

#[test]
fn an_origin_with_a_credential_is_refused_without_being_emitted() {
    let fixture = Fixture::new();
    git(
        &fixture.path("native-lib"),
        &[
            "remote",
            "set-url",
            "origin",
            "https://x-access-token:synthetic-secret@github.com/example/native-lib",
        ],
    );
    let output = std::cell::RefCell::new(String::new());
    let emit = |event| {
        if let super::super::Event::Output(line) = event {
            output.borrow_mut().push_str(&line);
        }
    };
    let runner = Runner {
        cancel: &fixture.cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let config = primary_config(NATIVE);
    let refused = resolve(
        &fixture.path("app"),
        &config,
        &config.profiles["dev"],
        &[Binding {
            alias: "native".into(),
            local_repository: fixture.path("native-lib"),
            revision: None,
        }],
        &runner,
    );
    assert!(
        matches!(refused, Err(Error::Sibling(SiblingError::Origin(_)))),
        "{refused:?}"
    );
    let listed = candidates(&fixture.path("app"), &config, &runner).unwrap();
    assert_eq!(listed[0].suggested, None);
    assert!(!output.borrow().contains("synthetic-secret"), "{}", output.borrow());
}

#[test]
fn a_deployment_keeps_the_siblings_chosen_before_its_image_was_built() {
    let fixture = Fixture::new();
    let app = fixture.path("app");
    std::fs::write(
        app.join(".horizon/cloud.yml"),
        format!("{PROFILES}companions:\n{NATIVE}"),
    )
    .unwrap();
    git(&app, &["commit", "--quiet", "-am", "Declare siblings"]);
    let profile = &primary_config(NATIVE).profiles["dev"];
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"siblings","repository":app,"revision":git(&app, &["rev-parse", "HEAD"]),
        "profile":profile,"stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let runner = fixture.runner();
    let chosen = [Binding {
        alias: "native".into(),
        local_repository: fixture.path("native-lib"),
        revision: None,
    }];
    assert!(!bind(&[], &mut state, &runner).unwrap());
    assert!(state.siblings.is_none());
    assert!(bind(&chosen, &mut state, &runner).unwrap());
    assert_eq!(state.version, RECORD_VERSION);
    let pinned = state.siblings.clone().unwrap();
    git(
        &fixture.path("native-lib"),
        &["commit", "--quiet", "--allow-empty", "-m", "Move on"],
    );
    assert!(
        !bind(&chosen, &mut state, &runner).unwrap(),
        "a retry keeps the pinned revision"
    );
    let inside = [Binding {
        alias: "native".into(),
        local_repository: fixture.path("native-lib/.horizon"),
        revision: None,
    }];
    assert!(
        !bind(&inside, &mut state, &runner).unwrap(),
        "a retry from inside the checkout names the same checkout"
    );
    assert!(!bind(&[], &mut state, &runner).unwrap(), "a reconnect keeps the set");
    assert_eq!(state.siblings.as_ref(), Some(&pinned));
    let moved = [Binding {
        alias: "native".into(),
        local_repository: app.clone(),
        revision: None,
    }];
    assert!(matches!(
        bind(&moved, &mut state, &runner),
        Err(Error::Sibling(SiblingError::Rebound))
    ));
    state.siblings = None;
    state.spec = Some(serde_json::from_value(serde_json::json!({
        "operation_id":"siblings","image_digest":format!("example.invalid/worker@sha256:{}", "a".repeat(64)),
        "profile":profile,"public_key":"unused","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]
    }))
    .unwrap());
    assert!(matches!(
        bind(&chosen, &mut state, &runner),
        Err(Error::Sibling(SiblingError::Late))
    ));
    assert!(state.siblings.is_none());
}

#[test]
fn project_migration_refuses_a_record_it_would_strip_of_siblings() {
    // Records with siblings are always the sibling record version, which migration refuses.
    use crate::cloud_runtime::allocation::{AllocationId, ControllerId, ProjectId, ProjectIdentity, legacy::Records};
    let mut record = serde_json::json!({
        "version":1,"cloud_id":"legacy-cloud","repository":"/synthetic/app","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    });
    let convert = |record: &serde_json::Value| {
        let identity = ProjectIdentity::new(
            ProjectId::generate(),
            "session".into(),
            "workspace".into(),
            "legacy-cloud".into(),
        )
        .unwrap();
        Records::from_legacy(
            &serde_json::to_vec(record).unwrap(),
            identity,
            AllocationId::generate(),
            ControllerId::generate(),
        )
    };
    assert!(convert(&record).is_ok());
    record["siblings"] = serde_json::json!({"primary_directory":"app","members":[]});
    assert!(convert(&record).is_err(), "migration never drops a field it was given");
    record["version"] = RECORD_VERSION.into();
    record["siblings"]["members"] = serde_json::json!([{
        "alias":"native","repository":"example/native-lib","directory":"native-lib","revision":"b".repeat(40),
        "local_repository":"/synthetic/native-lib","profile":"dev"
    }]);
    assert!(convert(&record).is_err());
}

#[test]
fn a_moved_checkout_is_named_before_it_is_read() {
    let fixture = Fixture::new();
    let mut set = fixture
        .resolve(&primary_config(NATIVE), "dev", &[("native", "native-lib")])
        .unwrap();
    assert!(set.members[0].checkout().is_ok());
    set.members[0].local_repository = fixture.path("moved");
    match set.members[0].checkout() {
        Err(Error::Sibling(refusal)) => assert_eq!(refusal, SiblingError::Moved("native".into())),
        other => panic!("expected a moved checkout, got {other:?}"),
    }
}

#[test]
#[cfg(unix)] // Project migration needs durable directory updates, which only Unix hosts have.
fn project_migration_checks_a_sibling_record_neighbor_for_the_same_worker() {
    use crate::cloud_runtime::{allocation::ControllerId, state::migration::MigratedStore};
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().canonicalize().unwrap();
    let record = |cloud: &str, worker: &str| {
        serde_json::json!({
            "version":1,"cloud_id":cloud,"repository":"/synthetic/app","revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":worker},"spec":null,"worker":null,"sessions":[]
        })
    };
    let write = |cloud: &str, value: &serde_json::Value| {
        let root = parent.join(cloud);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("deployment.json"), serde_json::to_vec(value).unwrap()).unwrap();
        root
    };
    let mut neighbor = record("neighbor", "shared-worker");
    neighbor["version"] = RECORD_VERSION.into();
    neighbor["siblings"] = serde_json::json!({"primary_directory":"app","members":[{
        "alias":"native","repository":"example/native-lib","directory":"native-lib","revision":"b".repeat(40),
        "local_repository":"/synthetic/native-lib","profile":"dev"
    }]});
    write("neighbor", &neighbor);
    let legacy = write("legacy", &record("legacy", "shared-worker"));
    match MigratedStore::migrate(&legacy, "session", "workspace", ControllerId::generate()) {
        Err(Error::Invalid(message)) => {
            assert_eq!(message, "Another legacy project references the same provider worker");
        }
        Err(other) => panic!("expected the duplicate worker refusal, got {other:?}"),
        Ok(_) => panic!("a shared worker must not migrate"),
    }
}

#[test]
fn a_rebuild_moves_only_the_revisions_of_the_recorded_siblings() {
    let fixture = Fixture::new();
    let config = primary_config(NATIVE);
    let set = fixture.resolve(&config, "dev", &[("native", "native-lib")]).unwrap();
    let profile = &config.profiles["dev"];
    let latest_of =
        |config: &CloudConfig, set: &Set| latest(set, &fixture.path("app"), config, profile, &fixture.runner());
    assert_eq!(latest_of(&config, &set).unwrap(), [set.members[0].revision.clone()]);
    let lib = fixture.path("native-lib");
    git(&lib, &["commit", "--quiet", "--allow-empty", "-m", "Move on"]);
    assert_eq!(latest_of(&config, &set).unwrap(), [git(&lib, &["rev-parse", "HEAD"])]);
    let moved = primary_config(
        "  native:\n    repository: example/native-lib\n    profile: prebuilt\n    placement: same_worker\n",
    );
    assert!(matches!(
        latest_of(&moved, &set),
        Err(Error::Sibling(SiblingError::ImageOnly { .. }))
    ));
    let mut renamed = set.clone();
    renamed.primary_directory = "other".into();
    assert!(matches!(
        latest_of(&config, &renamed),
        Err(Error::Sibling(SiblingError::Changed))
    ));
    let mut elsewhere = set.clone();
    elsewhere.members[0].profile = "gpu".into();
    assert!(matches!(
        latest_of(&config, &elsewhere),
        Err(Error::Sibling(SiblingError::Changed))
    ));
    let mut gone = set;
    gone.members[0].local_repository = fixture.path("gone");
    assert!(matches!(
        latest_of(&config, &gone),
        Err(Error::Sibling(SiblingError::Moved(_)))
    ));
}

#[test]
fn a_rebuild_reports_a_changed_declaration_as_a_changed_cloud() {
    let fixture = Fixture::new();
    let config = primary_config(NATIVE);
    let set = fixture.resolve(&config, "dev", &[("native", "native-lib")]).unwrap();
    let profile = &config.profiles["dev"];
    for companions in [
        "  other:\n    repository: example/native-lib\n    profile: dev\n    placement: same_worker\n",
        "  native:\n    repository: example/native-lib\n    profile: dev\n",
        "  native:\n    repository: Example/Native-Lib\n    profile: dev\n    placement: same_worker\n",
    ] {
        let changed = primary_config(companions);
        assert!(
            matches!(
                latest(&set, &fixture.path("app"), &changed, profile, &fixture.runner()),
                Err(Error::Sibling(SiblingError::Changed))
            ),
            "{companions}"
        );
    }
}

#[test]
fn a_recorded_set_holds_commit_ids_and_names_only_moved_recipes() {
    let fixture = Fixture::new();
    let mut set = fixture
        .resolve(&primary_config(NATIVE), "dev", &[("native", "native-lib")])
        .unwrap();
    let revision = set.members[0].revision.clone();
    assert!(set.fits());
    assert!(set.moved(std::slice::from_ref(&revision)).is_empty());
    assert_eq!(set.moved(&["f".repeat(40)]).len(), 1);
    set.members[0].image_revision = Some("f".repeat(40));
    assert!(set.fits());
    assert!(set.moved(&["f".repeat(40)]).is_empty());
    assert_eq!(set.moved(std::slice::from_ref(&revision)).len(), 1);
    for image_revision in [revision, "not-a-commit".into()] {
        set.members[0].image_revision = Some(image_revision);
        assert!(!set.fits());
    }
}

#[test]
fn an_expected_revision_pins_exactly_the_reviewed_commit() {
    let fixture = Fixture::new();
    let app = fixture.path("app");
    std::fs::write(
        app.join(".horizon/cloud.yml"),
        format!("{PROFILES}companions:\n{NATIVE}"),
    )
    .unwrap();
    git(&app, &["commit", "--quiet", "-am", "Declare siblings"]);
    let config = primary_config(NATIVE);
    let profile = &config.profiles["dev"];
    let checkout = fixture.path("native-lib");
    let reviewed = git(&checkout, &["rev-parse", "HEAD"]);
    let expecting = |revision: &str| {
        [Binding {
            alias: "native".into(),
            local_repository: checkout.clone(),
            revision: Some(revision.to_owned()),
        }]
    };
    let runner = fixture.runner();
    let set = resolve(&app, &config, profile, &expecting(&reviewed), &runner)
        .unwrap()
        .unwrap();
    assert_eq!(set.members[0].revision, reviewed);
    let refusal = resolve(&app, &config, profile, &expecting(&"f".repeat(40)), &runner);
    assert!(
        matches!(refusal, Err(Error::Sibling(SiblingError::Advanced(ref alias))) if alias == "native"),
        "{refusal:?}"
    );

    // A retry after a failure before the set was recorded checks the checkout again.
    git(&checkout, &["commit", "--quiet", "--allow-empty", "-m", "Move on"]);
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"siblings","repository":app,"revision":git(&app, &["rev-parse", "HEAD"]),
        "profile":profile,"stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    assert!(matches!(
        bind(&expecting(&reviewed), &mut state, &runner),
        Err(Error::Sibling(SiblingError::Advanced(_)))
    ));
    assert!(state.siblings.is_none(), "a refused choice records nothing");

    // Once recorded, a retry expecting the pinned commit keeps it; another commit is refused.
    let moved_to = git(&checkout, &["rev-parse", "HEAD"]);
    assert!(bind(&expecting(&moved_to), &mut state, &runner).unwrap());
    assert!(!bind(&expecting(&moved_to), &mut state, &runner).unwrap());
    assert!(matches!(
        bind(&expecting(&reviewed), &mut state, &runner),
        Err(Error::Sibling(SiblingError::Rebound))
    ));
    assert_eq!(state.siblings.as_ref().unwrap().members[0].revision, moved_to);
}

#[test]
fn a_binding_without_an_expected_revision_keeps_its_encoding() {
    let binding = Binding {
        alias: "native".into(),
        local_repository: "/synthetic/native-lib".into(),
        revision: None,
    };
    let encoded = serde_json::to_value(&binding).unwrap();
    assert!(encoded.get("revision").is_none());
    let legacy: Binding =
        serde_json::from_value(serde_json::json!({"alias":"native","local_repository":"/synthetic/native-lib"}))
            .unwrap();
    assert_eq!(legacy, binding);
    let expected = Binding {
        revision: Some("a".repeat(40)),
        ..binding
    };
    let round_trip: Binding = serde_json::from_value(serde_json::to_value(&expected).unwrap()).unwrap();
    assert_eq!(round_trip, expected);
}

#[test]
fn review_refuses_each_chosen_sibling_on_its_own_row() {
    let fixture = Fixture::new();
    let config = primary_config(&format!(
        "{NATIVE}  tool:\n    repository: example/tool\n    profile: dev\n    placement: same_worker\n"
    ));
    checkout(&fixture.path("tool"), "https://github.com/someone/tool", PROFILES);
    let bindings = [
        Binding {
            alias: "native".into(),
            local_repository: fixture.path("native-lib"),
        },
        Binding {
            alias: "tool".into(),
            local_repository: fixture.path("tool"),
        },
    ];
    let reviewed = review(
        &fixture.path("app"),
        &config,
        &config.profiles["dev"],
        &bindings,
        &fixture.runner(),
    )
    .unwrap();
    assert!(!reviewed.passed());
    assert_eq!(reviewed.choice, None);
    let native = reviewed.siblings["native"].as_ref().unwrap();
    assert_eq!(
        native.revision,
        git(&fixture.path("native-lib"), &["rev-parse", "HEAD"])
    );
    assert_eq!(
        reviewed.siblings["tool"],
        Err(SiblingError::OriginMismatch {
            alias: "tool".into(),
            declared: "example/tool".into(),
            found: "someone/tool".into(),
        })
    );
    let relative = [Binding {
        alias: "native".into(),
        local_repository: "native-lib".into(),
    }];
    let reviewed = review(
        &fixture.path("app"),
        &config,
        &config.profiles["dev"],
        &relative,
        &fixture.runner(),
    )
    .unwrap();
    assert_eq!(
        reviewed.siblings["native"],
        Err(SiblingError::Relative("native".into()))
    );
}

#[test]
fn review_reports_a_primary_refusal_once_for_the_whole_choice() {
    let fixture = Fixture::new();
    let config = primary_config(NATIVE);
    let bindings = [Binding {
        alias: "native".into(),
        local_repository: fixture.path("native-lib"),
    }];
    let reviewed = review(
        &fixture.path("app"),
        &config,
        &config.profiles["prebuilt"],
        &bindings,
        &fixture.runner(),
    )
    .unwrap();
    assert_eq!(reviewed.choice, Some(SiblingError::PrimaryImageOnly));
    assert!(reviewed.siblings.is_empty() && !reviewed.passed());
    let passed = review(
        &fixture.path("app"),
        &config,
        &config.profiles["dev"],
        &bindings,
        &fixture.runner(),
    )
    .unwrap();
    assert!(passed.passed());
    let many: Vec<_> = (0..=MAX_SIBLINGS).map(|_| bindings[0].clone()).collect();
    let crowded = review(
        &fixture.path("app"),
        &config,
        &config.profiles["dev"],
        &many,
        &fixture.runner(),
    )
    .unwrap();
    assert_eq!(crowded.choice, Some(SiblingError::TooMany));
    fixture.cancel.cancel();
    assert!(matches!(
        review(
            &fixture.path("app"),
            &config,
            &config.profiles["dev"],
            &bindings,
            &fixture.runner(),
        ),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
}

#[test]
fn every_refusal_that_names_a_sibling_reports_its_alias() {
    let named = SiblingError::OriginMismatch {
        alias: "native".into(),
        declared: "example/native-lib".into(),
        found: "someone/native-lib".into(),
    };
    assert_eq!(named.alias(), Some("native"));
    assert_eq!(SiblingError::Relative("tool".into()).alias(), Some("tool"));
    assert_eq!(SiblingError::DirectoryName("-lib".into()).alias(), None);
    assert_eq!(SiblingError::PrimaryImageOnly.alias(), None);
}

#[test]
fn a_reviewed_sibling_is_unmoved_only_while_its_head_is_the_reviewed_commit() {
    let fixture = Fixture::new();
    let set = fixture
        .resolve(&primary_config(NATIVE), "dev", &[("native", "native-lib")])
        .unwrap();
    let sibling = &set.members[0];
    sibling.unmoved(&fixture.runner()).unwrap();
    let checkout = fixture.path("native-lib");
    std::fs::write(checkout.join("later.txt"), "later").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "--quiet", "-m", "Later"]);
    assert!(matches!(
        sibling.unmoved(&fixture.runner()),
        Err(Error::Sibling(SiblingError::Advanced(alias))) if alias == "native"
    ));
    assert_eq!(SiblingError::Advanced("native".into()).alias(), Some("native"));
}
