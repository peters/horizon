use super::*;

fn target(id: &str) -> Target {
    Target {
        scope: Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        },
        cloud_id: id.into(),
        declaration: Declaration::new("example/app", "cpu"),
    }
}

#[test]
fn legacy_yaml_stays_valid_and_declarations_are_passive_metadata() {
    let original = crate::CloudConfig::parse(crate::EXAMPLE).unwrap();
    assert!(original.companions.is_empty());
    let yaml = format!(
        "{}\ncompanions:\n  app:\n    repository: example/app\n    profile: cpu\n",
        crate::EXAMPLE
    );
    let config = crate::CloudConfig::parse(&yaml).unwrap();
    assert_eq!(config.profiles, original.profiles);
    assert_eq!(config.companions["app"], target("b").declaration);
    let serialized = serde_yaml::to_string(&config).unwrap();
    assert_eq!(
        crate::CloudConfig::parse(&serialized).unwrap().companions,
        config.companions
    );
}

#[test]
fn declarations_reject_paths_credentials_and_ssh_syntax() {
    for repository in [
        "../app",
        "/local/app",
        "https://token@example.com/app",
        "a/b/c",
        "a/$(id)",
    ] {
        let yaml = format!(
            "{}\ncompanions:\n  app:\n    repository: {repository}\n    profile: cpu\n",
            crate::EXAMPLE
        );
        assert!(crate::CloudConfig::parse(&yaml).is_err(), "accepted {repository}");
    }
    for alias in ["*", "-proxy", "a b", "../app", "App"] {
        assert!(validate_declarations(&BTreeMap::from([(alias.into(), target("b").declaration)])).is_err());
    }
    let yaml = format!(
        "{}\ncompanions:\n  app:\n    repository: example/app\n    profile: cpu\n    private_key: private-value\n",
        crate::EXAMPLE
    );
    let error = crate::CloudConfig::parse(&yaml).unwrap_err().to_string();
    assert!(!error.contains("private-value"));
}

#[test]
fn selection_pins_target_and_declaration_across_serialization() {
    let source = target("a");
    let inventory = vec![target("b"), target("c")];
    assert_eq!(candidates(&source, &inventory[0].declaration, &inventory).len(), 2);
    let selection = Selection::new(&source, "app", &inventory[0]).unwrap();
    let selection: Selection = serde_json::from_slice(&serde_json::to_vec(&selection).unwrap()).unwrap();
    assert_eq!(
        selection
            .resolve(&source, "app", &inventory[0].declaration, &inventory)
            .unwrap()
            .cloud_id,
        "b"
    );
    assert_eq!(
        selection.resolve(&source, "app", &inventory[0].declaration, &inventory[1..]),
        Err(SelectionError::Missing)
    );
    let mut declaration = inventory[0].declaration.clone();
    declaration.profile = "gpu".into();
    assert_eq!(
        selection.resolve(&source, "app", &declaration, &inventory),
        Err(SelectionError::DeclarationChanged)
    );
}

#[test]
fn selection_cannot_cross_scope_or_follow_a_reused_id() {
    let source = target("a");
    let selected = target("b");
    let selection = Selection::new(&source, "app", &selected).unwrap();
    for field in ["session", "workspace", "repository"] {
        let mut changed = selected.clone();
        match field {
            "session" => changed.scope.session_id = "other".into(),
            "workspace" => changed.scope.workspace_id = "other".into(),
            _ => changed.declaration.repository = "example/other".into(),
        }
        assert!(
            selection
                .resolve(&source, "app", &selected.declaration, &[changed])
                .is_err()
        );
    }
    assert!(Selection::new(&source, "self", &source).is_err());
    let mut other_source = source.clone();
    other_source.scope.session_id = "copied-session".into();
    assert_eq!(
        selection.resolve(&other_source, "app", &selected.declaration, &[]),
        Err(SelectionError::ScopeMismatch)
    );
    assert_eq!(
        selection.resolve(
            &source,
            "app",
            &selected.declaration,
            &[selected.clone(), selected.clone()]
        ),
        Err(SelectionError::Ambiguous)
    );
}

#[test]
fn discovery_excludes_self_and_other_workspaces() {
    let source = target("a");
    let mut other = target("other");
    other.scope.workspace_id = "different".into();
    let inventory = [source.clone(), target("b"), other];
    let found = candidates(&source, &source.declaration, &inventory);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].cloud_id, "b");
}

#[test]
fn github_identity_accepts_dot_repositories_and_matches_case_insensitively() {
    let source = target("a");
    let mut selected = target("b");
    selected.declaration.repository = "Example/.GitHub".into();
    selected.declaration.validate().unwrap();
    let selection = Selection::new(&source, "app", &selected).unwrap();
    let mut declaration = selected.declaration.clone();
    declaration.repository = "example/.github".into();
    let inventory = [selected];
    assert_eq!(candidates(&source, &declaration, &inventory).len(), 1);
    assert!(selection.resolve(&source, "app", &declaration, &inventory).is_ok());
    declaration.profile = "CPU".into();
    assert!(candidates(&source, &declaration, &inventory).is_empty());
    assert_eq!(
        selection.resolve(&source, "app", &declaration, &inventory),
        Err(SelectionError::DeclarationChanged)
    );
    for repository in ["example/.", "example/.."] {
        declaration.repository = repository.into();
        assert!(declaration.validate().is_err());
    }
}

#[test]
fn copied_inventory_ids_in_other_scopes_do_not_shadow_the_selected_target() {
    let source = target("a");
    let selected = target("b");
    let selection = Selection::new(&source, "app", &selected).unwrap();
    let mut other = selected.clone();
    other.scope.session_id = "another-session".into();
    let inventory = [other, selected.clone()];
    assert_eq!(
        selection.resolve(&source, "app", &selected.declaration, &inventory),
        Ok(&selected)
    );
    for invalid in ["../session", "", "workspace/name"] {
        let mut invalid_source = source.clone();
        let mut invalid_target = selected.clone();
        invalid_source.scope.session_id = invalid.into();
        invalid_target.scope = invalid_source.scope.clone();
        assert_eq!(
            Selection::new(&invalid_source, "app", &invalid_target),
            Err(SelectionError::Invalid)
        );
        invalid_source.scope.session_id = "valid-session".into();
        invalid_source.scope.workspace_id = invalid.into();
        invalid_target.scope = invalid_source.scope.clone();
        assert_eq!(
            Selection::new(&invalid_source, "app", &invalid_target),
            Err(SelectionError::Invalid)
        );
    }
}

#[test]
fn owner_names_follow_login_constraints_including_managed_user_suffixes() {
    let declaration = |owner: &str| Declaration::new(format!("{owner}/.github"), "cpu");
    for owner in [
        "a",
        "Example-Owner",
        "mona-cat_octo",
        "octo_admin",
        "foo_bar",
        &"a".repeat(39),
    ] {
        assert!(declaration(owner).validate().is_ok(), "rejected {owner}");
    }
    for owner in [
        "",
        "foo.bar",
        "foo--bar",
        "foo-",
        "-foo",
        "foo__bar",
        "foo_ab",
        "foo_abcdefghi",
        "foo_bar_baz",
        "foo-_bar",
        &"a".repeat(40),
    ] {
        assert!(declaration(owner).validate().is_err(), "accepted {owner}");
    }
    assert!(declaration(&format!("{}_abc", "a".repeat(35))).validate().is_ok());
    assert!(declaration(&format!("{}_abc", "a".repeat(36))).validate().is_err());
}

fn with_companions(companions: &str) -> String {
    format!("{}\ncompanions:\n{companions}", crate::EXAMPLE)
}

#[test]
fn cloud_placement_is_the_default_and_serializes_as_before() {
    let implicit = crate::CloudConfig::parse(&with_companions(
        "  app:\n    repository: example/app\n    profile: cpu\n",
    ))
    .unwrap();
    let explicit = crate::CloudConfig::parse(&with_companions(
        "  app:\n    repository: example/app\n    profile: cpu\n    placement: cloud\n",
    ))
    .unwrap();
    assert_eq!(implicit.companions, explicit.companions);
    assert_eq!(implicit.companions["app"].placement, Placement::Cloud);
    let selection = Selection::new(&target("a"), "app", &target("b")).unwrap();
    let json = serde_json::to_value(&selection).unwrap();
    assert_eq!(
        json["declaration"],
        serde_json::json!({"repository": "example/app", "profile": "cpu"})
    );
    assert!(!serde_yaml::to_string(&explicit).unwrap().contains("placement"));
    assert_eq!(implicit.cloud_companions().count(), 1);
    assert_eq!(implicit.same_worker_siblings().count(), 0);
}

#[test]
fn same_worker_siblings_round_trip_and_name_their_directory() {
    let config = crate::CloudConfig::parse(&with_companions(
        "  consumer:\n    repository: Example/Consumer-App\n    profile: gpu\n    placement: same_worker\n  \
         service:\n    repository: example/service\n    profile: cpu\n",
    ))
    .unwrap();
    let siblings: Vec<_> = config.same_worker_siblings().collect();
    assert_eq!(siblings.len(), 1);
    let (alias, sibling) = siblings[0];
    assert_eq!(alias, "consumer");
    assert_eq!(sibling.placement, Placement::SameWorker);
    assert_eq!(sibling.directory_name(), "Consumer-App");
    assert_eq!(
        config.cloud_companions().map(|(alias, _)| alias).collect::<Vec<_>>(),
        ["service"]
    );
    let serialized = serde_yaml::to_string(&config).unwrap();
    assert!(serialized.contains("placement: same_worker"));
    let (head, tail) = crate::EXAMPLE.split_once("# companions:").unwrap();
    let example = crate::CloudConfig::parse(&format!("{head}companions:{}", tail.replace("\n# ", "\n"))).unwrap();
    assert_eq!(example.same_worker_siblings().count(), 1);
    assert_eq!(example.cloud_companions().count(), 1);
    assert_eq!(
        crate::CloudConfig::parse(&serialized).unwrap().companions,
        config.companions
    );
}

#[test]
fn placement_rejects_unknown_values_and_colliding_sibling_directories() {
    for placement in ["same-worker", "SameWorker", "worker", "\"\""] {
        let yaml = with_companions(&format!(
            "  app:\n    repository: example/app\n    profile: cpu\n    placement: {placement}\n"
        ));
        assert!(crate::CloudConfig::parse(&yaml).is_err(), "accepted {placement}");
    }
    let sibling = |repository: &str| Declaration {
        placement: Placement::SameWorker,
        ..Declaration::new(repository, "cpu")
    };
    let colliding = BTreeMap::from([
        ("one".into(), sibling("first/app")),
        ("two".into(), sibling("second/APP")),
    ]);
    assert!(validate_declarations(&colliding).is_err());
    // Only same-worker checkouts share a directory namespace.
    let mixed = BTreeMap::from([
        ("one".into(), sibling("first/app")),
        ("two".into(), Declaration::new("second/app", "cpu")),
    ]);
    assert!(validate_declarations(&mixed).is_ok());
    for repository in ["example/.", "example/.."] {
        assert!(sibling(repository).validate().is_err());
    }
}

#[test]
fn same_worker_declarations_are_never_selected_or_discovered() {
    let source = target("a");
    let mut sibling = target("b");
    sibling.declaration.placement = Placement::SameWorker;
    let inventory = [target("b")];
    assert!(candidates(&source, &sibling.declaration, &inventory).is_empty());
    assert_eq!(Selection::new(&source, "app", &sibling), Err(SelectionError::Invalid));
    let selection = Selection::new(&source, "app", &inventory[0]).unwrap();
    assert_eq!(
        selection.resolve(&source, "app", &sibling.declaration, &inventory),
        Err(SelectionError::DeclarationChanged)
    );
    assert!(!sibling.declaration.matches(&inventory[0].declaration));
}
