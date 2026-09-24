use super::*;

fn target(id: &str) -> Target {
    Target {
        scope: Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        },
        cloud_id: id.into(),
        declaration: Declaration {
            repository: "example/app".into(),
            profile: "cpu".into(),
        },
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
