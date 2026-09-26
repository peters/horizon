use super::*;
use std::io::Write;

struct Fixture {
    primary: tempfile::TempDir,
    library: tempfile::TempDir,
    tools: tempfile::TempDir,
    tokens: std::cell::RefCell<Vec<tempfile::NamedTempFile>>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            primary: tempfile::tempdir().unwrap(),
            library: tempfile::tempdir().unwrap(),
            tools: tempfile::tempdir().unwrap(),
            tokens: std::cell::RefCell::default(),
        }
    }

    fn binding(&self, local: &Path, repository: &str, token: &[u8]) -> Binding {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(token).unwrap();
        let token_file = file.path().into();
        self.tokens.borrow_mut().push(file);
        Binding {
            local_repository: local.into(),
            repository: repository.into(),
            token_file,
            author_name: format!("{repository} author"),
            author_email: "test@example.invalid".into(),
        }
    }

    fn siblings(&self) -> [Sibling<'_>; 2] {
        [
            Sibling {
                alias: "library",
                local_repository: self.library.path(),
            },
            Sibling {
                alias: "tools",
                local_repository: self.tools.path(),
            },
        ]
    }
}

fn payload(prepared: &Prepared) -> serde_json::Value {
    serde_json::from_reader(prepared.0.reopen().unwrap()).unwrap()
}

#[test]
fn each_repository_gets_only_its_own_binding_and_unbound_repositories_get_none() {
    let fixture = Fixture::new();
    let missing = fixture.primary.path().join("missing-checkout");
    let bindings = vec![
        fixture.binding(&missing, "example/unrelated", b"unrelated-token"),
        fixture.binding(fixture.library.path(), "example/library", b"library-token\n"),
        fixture.binding(fixture.primary.path(), "example/consumer", b"consumer-token"),
    ];
    let selected = select(&bindings, fixture.primary.path(), &fixture.siblings()).unwrap();
    let chosen: Vec<_> = selected
        .iter()
        .map(|entry| (entry.target.clone(), entry.binding.repository.as_str()))
        .collect();
    assert_eq!(
        chosen,
        [
            (Target::Primary, "example/consumer"),
            (Target::Sibling("library".into()), "example/library"),
        ]
    );

    let prepared = Prepared::for_repositories(&bindings, fixture.primary.path(), &fixture.siblings())
        .unwrap()
        .unwrap();
    assert_eq!(
        payload(&prepared),
        serde_json::json!({
            "version": 2,
            "grants": [
                {
                    "repository": "example/consumer",
                    "token": "consumer-token",
                    "author_name": "example/consumer author",
                    "author_email": "test@example.invalid",
                    "target": "primary",
                },
                {
                    "repository": "example/library",
                    "token": "library-token",
                    "author_name": "example/library author",
                    "author_email": "test@example.invalid",
                    "target": "sibling:library",
                },
            ],
        })
    );
}

#[test]
fn clouds_without_siblings_keep_the_single_repository_payload() {
    let fixture = Fixture::new();
    let bindings = vec![fixture.binding(fixture.primary.path(), "example/consumer", b"consumer-token")];
    let prepared = Prepared::for_repositories(&bindings, fixture.primary.path(), &[])
        .unwrap()
        .unwrap();
    let value = payload(&prepared);
    assert_eq!(value["repository"], "example/consumer");
    assert_eq!(value["token"], "consumer-token");
    assert!(value.get("version").is_none());
    assert!(
        Prepared::for_repositories(&[], fixture.primary.path(), &[])
            .unwrap()
            .is_none()
    );
}

#[test]
fn siblings_switch_even_a_primary_only_grant_to_version_two() {
    let fixture = Fixture::new();
    let bindings = vec![fixture.binding(fixture.primary.path(), "example/consumer", b"consumer-token")];
    let prepared = Prepared::for_repositories(&bindings, fixture.primary.path(), &fixture.siblings())
        .unwrap()
        .unwrap();
    let value = payload(&prepared);
    assert_eq!(value["version"], 2);
    assert_eq!(value["grants"].as_array().unwrap().len(), 1);
    assert_eq!(value["grants"][0]["target"], "primary");
    assert!(
        Prepared::for_repositories(&[], fixture.primary.path(), &fixture.siblings())
            .unwrap()
            .is_none()
    );
}

#[test]
fn ambiguous_or_duplicate_grants_fail_before_any_token_is_read() {
    let fixture = Fixture::new();
    let library = fixture.library.path();
    let twice = vec![
        fixture.binding(library, "example/library", b"first-token"),
        fixture.binding(library, "example/library-fork", b"second-token"),
    ];
    let error = select(&twice, fixture.primary.path(), &fixture.siblings()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Multiple Git credential bindings match this repository"
    );

    let shared = vec![
        fixture.binding(fixture.primary.path(), "example/library", b"first-token"),
        fixture.binding(library, "Example/Library", b"second-token"),
    ];
    let error = select(&shared, fixture.primary.path(), &fixture.siblings()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "One Git repository is bound to two worker repositories"
    );

    let same = [
        Sibling {
            alias: "library",
            local_repository: fixture.library.path(),
        },
        Sibling {
            alias: "library",
            local_repository: fixture.tools.path(),
        },
    ];
    assert!(select(&[], fixture.primary.path(), &same).is_err());
    for alias in [
        "",
        "Library",
        "../escape",
        "a/b",
        "1library",
        "library:x",
        &"a".repeat(65),
    ] {
        let sibling = [Sibling {
            alias,
            local_repository: fixture.library.path(),
        }];
        assert!(select(&[], fixture.primary.path(), &sibling).is_err(), "{alias}");
    }
    let missing = [Sibling {
        alias: "library",
        local_repository: &fixture.primary.path().join("missing-checkout"),
    }];
    assert!(select(&[], fixture.primary.path(), &missing).is_err());
}

#[test]
fn more_than_sixteen_grants_are_refused() {
    let fixture = Fixture::new();
    let checkouts: Vec<_> = (0..16).map(|_| tempfile::tempdir().unwrap()).collect();
    let aliases: Vec<_> = (0..16).map(|index| format!("sibling{index}")).collect();
    let mut bindings = vec![fixture.binding(fixture.primary.path(), "example/primary", b"token")];
    for (index, checkout) in checkouts.iter().enumerate() {
        bindings.push(fixture.binding(checkout.path(), &format!("example/sibling{index}"), b"token"));
    }
    let siblings: Vec<_> = aliases
        .iter()
        .zip(&checkouts)
        .map(|(alias, checkout)| Sibling {
            alias,
            local_repository: checkout.path(),
        })
        .collect();
    let error = select(&bindings, fixture.primary.path(), &siblings).unwrap_err();
    assert_eq!(error.to_string(), "Too many Git credential grants for one worker");
    assert_eq!(
        select(&bindings, fixture.primary.path(), &siblings[..15])
            .unwrap()
            .len(),
        16
    );
}

#[test]
fn every_grant_token_and_binding_is_checked_before_transfer() {
    let fixture = Fixture::new();
    let bindings = vec![
        fixture.binding(fixture.primary.path(), "example/consumer", b"consumer-token"),
        fixture.binding(fixture.library.path(), "example/library", b"private\ninjected-value"),
    ];
    let error = Prepared::for_repositories(&bindings, fixture.primary.path(), &fixture.siblings())
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "Invalid Git credential value");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut bindings = bindings;
        let mut replacement = fixture.binding(fixture.library.path(), "example/library", b"library-token");
        std::fs::set_permissions(&replacement.token_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::mem::swap(&mut bindings[1], &mut replacement);
        assert!(Prepared::for_repositories(&bindings, fixture.primary.path(), &fixture.siblings()).is_err());
    }

    let mut unrelated = fixture.binding(
        &fixture.primary.path().join("missing-checkout"),
        "example/other",
        b"token",
    );
    unrelated.author_email = "test@example.invalid\nhelper=bad".into();
    assert!(select(&[unrelated], fixture.primary.path(), &fixture.siblings()).is_err());
}

#[test]
fn the_payload_schema_matches_the_worker_and_never_prints_tokens() {
    let grant = serde_json::json!({
        "repository": "example/library",
        "token": "synthetic-token",
        "author_name": "Test User",
        "author_email": "test@example.invalid",
        "target": "sibling:library",
    });
    let parsed: GrantSet = serde_json::from_value(serde_json::json!({"version": 2, "grants": [grant]})).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.grants[0].target, Target::Sibling("library".into()));
    assert!(!format!("{parsed:?}").contains("synthetic-token"));
    assert_eq!(
        serde_json::to_value(&parsed).unwrap(),
        serde_json::json!({"version": 2, "grants": [grant]})
    );

    let mut extra = grant.clone();
    extra["helper"] = "bad".into();
    for rejected in [
        serde_json::json!({"version": 3, "grants": [grant]}),
        serde_json::json!({"version": 2, "grants": [grant], "extra": true}),
        serde_json::json!({"version": 2, "grants": [extra]}),
        serde_json::json!({"grants": [grant]}),
    ] {
        assert!(serde_json::from_value::<GrantSet>(rejected).is_err());
    }
    for target in ["sibling:", "sibling:Library", "sibling:../x", "secondary", "Primary"] {
        let mut value = grant.clone();
        value["target"] = target.into();
        assert!(serde_json::from_value::<Grant>(value).is_err(), "{target}");
    }

    let mut duplicate = grant.clone();
    duplicate["repository"] = "Example/Library".into();
    duplicate["target"] = "primary".into();
    let set: GrantSet =
        serde_json::from_value(serde_json::json!({"version": 2, "grants": [grant, duplicate]})).unwrap();
    assert!(set.validate().is_err());
    let empty: GrantSet = serde_json::from_value(serde_json::json!({"version": 2, "grants": []})).unwrap();
    assert!(empty.validate().is_err());
}
