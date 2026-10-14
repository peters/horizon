use super::*;

#[test]
fn two_accounts_build_one_repository_into_separate_namespaces() {
    let alice = Destination::automatic_github("Alice", "github.com/upstream/app", "cpu").unwrap();
    let bob = Destination::automatic_github("bob", "github.com/upstream/app", "cpu").unwrap();
    assert!(alice.repository.starts_with("ghcr.io/alice/app-cpu-worker-"));
    assert!(bob.repository.starts_with("ghcr.io/bob/app-cpu-worker-"));
    assert_eq!(alice.repository.split('/').nth(2), bob.repository.split('/').nth(2));
    assert_eq!(alice.publisher, Publisher::GitHub { login: "alice".into() });
}

#[test]
fn source_hosts_owners_and_profiles_do_not_collide() {
    let target = |source, profile| {
        Destination::automatic_github("alice", source, profile)
            .unwrap()
            .repository
    };
    let original = target("github.com/team/app", "cpu");
    for (source, profile) in [
        ("gitlab.example/team/app", "cpu"),
        ("github.com/other/app", "cpu"),
        ("github.com/team/app", "gpu"),
        ("github.com/team/app", "CPU"),
    ] {
        assert_ne!(target(source, profile), original);
    }
    assert_eq!(target("github.com/team/app", "cpu"), original);
}

#[test]
fn hostile_source_names_cannot_select_another_registry_or_account() {
    for source in [
        "registry.example/owner/..",
        "ghcr.io/someone/app",
        "a name with spaces",
        "日本語",
        "repo:tag",
        "repo@sha256:abc",
    ] {
        let target = Destination::automatic_github("alice", source, "cpu").unwrap();
        assert!(target.repository.starts_with("ghcr.io/alice/"));
        assert!(valid_repository(&target.repository));
    }
    for source in ["", "a\nrepository", "\0"] {
        assert_eq!(
            Destination::automatic_github("alice", source, "cpu"),
            Err(DestinationError::Source)
        );
    }
}

#[test]
fn missing_or_malformed_account_and_profile_names_are_refused() {
    for login in [
        "",
        "-alice",
        "alice-",
        "alice--bob",
        "alice/bob",
        "alice@host",
        "alice:token",
    ] {
        assert_eq!(
            Destination::automatic_github(login, "project", "cpu"),
            Err(DestinationError::Account)
        );
    }
    for profile in ["", "../cpu", "cpu:tag", "cpu\n"] {
        assert_eq!(
            Destination::automatic_github("alice", "project", profile),
            Err(DestinationError::Source)
        );
    }
}

#[test]
fn a_local_binding_supports_registry_ports_and_nested_projects() {
    for repository in [
        "registry.gitlab.com/group/subgroup/project/worker",
        "registry.example:5050/group/project/worker",
        "localhost:5000/worker",
        "registry:5000/worker",
        "registry.example/worker__name/worker---name/worker.name",
        "ghcr.io/organization/shared-worker",
    ] {
        let target = Destination::bound(repository.into(), "grant-1".into()).unwrap();
        assert_eq!(target.repository, repository);
        assert_eq!(
            target.publisher,
            Publisher::Registry {
                binding: "grant-1".into()
            }
        );
    }
}

#[test]
fn destinations_never_contain_tags_digests_userinfo_or_implicit_registries() {
    for repository in [
        "owner/worker",
        "worker",
        "ghcr.io/owner/worker:latest",
        "ghcr.io/owner/worker@sha256:abc",
        "https://ghcr.io/owner/worker",
        "user:token@registry.example/worker",
        "registry.example/../worker",
        "registry.example/owner//worker",
        "registry.example/owner/Worker",
        "registry.example:0/worker",
        "registry.example:65536/worker",
        "registry.example:abc/worker",
        "registry.example/owner/worker/",
        "registry..example/worker",
        "registry.example/worker..name",
        "registry.example/worker___name",
        "registry.example/worker.-name",
    ] {
        assert_eq!(
            Destination::bound(repository.into(), "grant-1".into()),
            Err(DestinationError::Repository),
            "{repository}"
        );
    }
    assert_eq!(
        Destination::bound("registry.example/worker".into(), "../grant".into()),
        Err(DestinationError::Binding)
    );
}

#[test]
fn stored_account_identity_cannot_be_replaced_by_an_unrelated_account() {
    let original = Destination::automatic_github("alice", "github.com/team/app", "cpu").unwrap();
    assert!(original.check_publisher(&original.publisher).is_ok());
    assert!(
        original
            .check_publisher(&Publisher::GitHub { login: "Alice".into() })
            .is_ok()
    );
    assert_eq!(
        original.check_publisher(&Publisher::GitHub { login: "bob".into() }),
        Err(DestinationError::Account)
    );
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(serde_json::from_str::<Destination>(&json).unwrap(), original);
    assert!(!json.contains("token"));
}

#[test]
fn a_personal_destination_cannot_name_another_namespace_or_noncanonical_stored_account() {
    let mut target = Destination::automatic_github("alice", "project", "cpu").unwrap();
    target.repository = "ghcr.io/organization/app".into();
    assert_eq!(target.validate(), Err(DestinationError::Account));
    assert_eq!(
        target.check_publisher(&Publisher::GitHub { login: "alice".into() }),
        Err(DestinationError::Account)
    );
    target.repository = "ghcr.io/alice/app".into();
    target.publisher = Publisher::GitHub { login: "Alice".into() };
    assert_eq!(target.validate(), Err(DestinationError::Account));
}

#[test]
fn long_and_unicode_names_remain_valid_and_distinct() {
    let prefix = "a".repeat(200);
    let first = Destination::automatic_github("alice", &format!("team/{prefix}a"), "cpu").unwrap();
    let second = Destination::automatic_github("alice", &format!("team/{prefix}b"), "cpu").unwrap();
    assert_ne!(first.repository, second.repository);
    assert!(first.repository.len() < 150);
    assert!(
        Destination::automatic_github("alice", "team/日本語", "cpu")
            .unwrap()
            .validate()
            .is_ok()
    );
}

#[test]
fn an_existing_destination_requires_the_same_publication_credential_reference() {
    let target = Destination::bound("registry.example/team/app".into(), "grant-1".into()).unwrap();
    assert!(
        target
            .check_publisher(&Publisher::Registry {
                binding: "grant-1".into()
            })
            .is_ok()
    );
    assert_eq!(
        target.check_publisher(&Publisher::Registry {
            binding: "grant-2".into()
        }),
        Err(DestinationError::Binding)
    );
    assert_eq!(
        target.check_publisher(&Publisher::GitHub { login: "alice".into() }),
        Err(DestinationError::Binding)
    );
}
