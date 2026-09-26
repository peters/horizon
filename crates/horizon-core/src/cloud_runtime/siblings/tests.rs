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
                local_repository: fixture.path("native-lib").canonicalize().unwrap(),
                profile: "dev".into(),
            }],
        }
    );
    let primary = primary_config(NATIVE).profiles["dev"].build.clone().unwrap();
    assert_eq!(set.members[0].recipe(&primary, &fixture.runner()).unwrap(), primary);
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
    let dash = primary_config("  dash:\n    repository: example/-lib\n    profile: dev\n    placement: same_worker\n");
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
    // Profiles accept only linux/amd64 today; the check keeps a later platform from mixing.
    let revision = git(&lib, &["rev-parse", "HEAD~2"]);
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
