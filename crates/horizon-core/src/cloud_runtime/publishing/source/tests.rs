use super::*;
use horizon_cloud::Cancellation;

fn target(repo: &Path, origin: Option<&str>) -> Destination {
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let config = origin.map_or_else(String::new, |origin| format!("[remote \"origin\"]\nurl = {origin}\n"));
    std::fs::write(repo.join(".git/config"), config).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(repo.join(".git/objects")).unwrap();
    std::fs::create_dir_all(repo.join(".git/refs")).unwrap();
    let cancel = Cancellation::default();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let result = automatic("builder", repo, "cpu", &runner).unwrap();
    assert!(events.borrow().is_empty(), "the remote must never enter output events");
    result
}

#[test]
fn credentials_do_not_change_source_identity_or_escape_into_metadata() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let plain = target(&repo, Some("https://git.example/team/project.git"));
    let authenticated = target(
        &repo,
        Some("https://synthetic-user:synthetic-password@git.example/team/project.git"),
    );
    assert_eq!(plain, authenticated);
    let encoded = serde_json::to_string(&authenticated).unwrap();
    assert!(!encoded.contains("synthetic"));
    assert!(encoded.contains("ghcr.io/builder/"));
    assert_ne!(plain, target(&repo, Some("https://git.example:8443/team/project.git")));
    assert_ne!(plain, target(&repo, Some("https://another.example/team/project.git")));
}

#[test]
fn local_sources_are_distinct_and_cancelled_lookups_do_not_run_git() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a/project");
    let b = root.path().join("b/project");
    assert_ne!(target(&a, None), target(&b, None));
    let cancel = Cancellation::default();
    cancel.cancel();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(automatic("builder", &root.path().join("absent"), "cpu", &runner).is_err());
}

#[test]
fn git_origins_preserve_paths_suffixes_transports_and_local_scopes() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a/project");
    let b = root.path().join("b/project");
    assert_ne!(
        target(&a, Some("team/project.git")),
        target(&b, Some("team/project.git"))
    );
    for (first, second) in [
        (
            "https://git.example/team/base/-/one/project.git",
            "https://git.example/team/base/-/two/project.git",
        ),
        (
            "https://git.example/team/project.git",
            "https://git.example/team/project.git.git",
        ),
        (
            "https://git.example/team/project.git",
            "ssh://git.example/team/project.git",
        ),
        ("alice@git.example:project.git", "bob@git.example:project.git"),
    ] {
        assert_ne!(
            target(&a, Some(first)),
            target(&a, Some(second)),
            "distinct Git origins retain separate identities"
        );
    }
    for local in ["repos/team:project", "repos\\team:project"] {
        assert!(remote(local).unwrap().is_none());
    }
    assert!(
        remote("git@[2001:db8::1]:team/project.git")
            .unwrap()
            .unwrap()
            .starts_with("ssh-home://[2001:db8::1]/")
    );
    let scoped = remote("synthetic-ssh-user@git.example:project.git").unwrap().unwrap();
    assert!(!scoped.contains("synthetic-ssh-user"));
    assert_eq!(
        remote("https://u:password@git.example/team/project.git").unwrap(),
        remote("https://git.example/team/project.git").unwrap()
    );
    for ambiguous in [
        "https://git.example/repo.git?token=synthetic",
        "https://git.example/repo.git#other",
        "https://git.example/a.git\nhttps://git.example/b.git",
    ] {
        assert!(remote(ambiguous).is_err());
    }
}

#[test]
// APFS refuses these byte sequences before a checkout can exist. Linux exercises the fallback.
#[cfg(target_os = "linux")]
fn non_utf8_local_paths_are_refused_without_lossy_package_identity() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    for byte in [0xff, 0xfe] {
        let repo = root
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'r', byte]))
            .join("project");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(matches!(
            automatic("builder", &repo.canonicalize().unwrap(), "cpu", &runner),
            Err(Error::Invalid(
                "Automatic images need a UTF-8 checkout path or an explicit image destination"
            ))
        ));
    }
}

#[test]
fn multiple_origin_values_are_refused_even_when_one_is_empty() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    target(&repo, None);
    std::fs::write(
        repo.join(".git/config"),
        "[remote \"origin\"]\nurl = https://git.example/team/project.git\nurl =\n",
    )
    .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(matches!(
        automatic("builder", &repo, "cpu", &runner),
        Err(Error::Invalid(ORIGIN_INVALID))
    ));
}

#[test]
fn ssh_uri_home_paths_keep_their_user_scope() {
    let alice = remote("ssh://alice@git.example/~/project.git").unwrap().unwrap();
    let bob = remote("ssh://bob@git.example/~/project.git").unwrap().unwrap();
    assert_ne!(alice, bob);
    assert!(!alice.contains("alice"));
    assert_eq!(Some(alice.clone()), remote("alice@git.example:project.git").unwrap());
    for equivalent in [
        "ssh://alice:synthetic-password@git.example/~/project.git",
        "ssh://%61lice@git.example/%7e/project.git",
        "ssh://bob@git.example/~alice/project.git",
        "bob@git.example:~alice/project.git",
    ] {
        assert_eq!(Some(alice.clone()), remote(equivalent).unwrap());
    }
    assert_ne!(Some(alice), remote("alice@git.example:/~alice/project.git").unwrap());
    assert!(remote("ssh://git.example/~/project.git").unwrap().is_none());
    let root = tempfile::tempdir().unwrap();
    assert_ne!(
        target(&root.path().join("one"), Some("ssh://git.example/~/project.git")),
        target(&root.path().join("two"), Some("ssh://git.example/~/project.git"))
    );
    for invalid in [
        "ssh://alice@git.example/%ff/project.git",
        "ssh://alice@git.example/%00/project.git",
        "ssh://alice@git.example/%zz/project.git",
    ] {
        assert!(remote(invalid).is_err());
    }
}

#[test]
fn ipv6_scp_paths_keep_at_signs_out_of_username_detection() {
    let explicit = remote("git@[2001:db8::1]:team/project@v2.git").unwrap();
    assert_eq!(
        explicit,
        remote("ssh://git@[2001:db8::1]/~/team/project@v2.git").unwrap()
    );
    assert_ne!(explicit, remote("git@[2001:db8::1]:team/project@other.git").unwrap());
    assert!(remote("[2001:db8::1]:team/project@v2.git").unwrap().is_none());
    assert_eq!(
        remote("[2001:db8::1]:/team/project@v2.git").unwrap(),
        Some("ssh://[2001:db8::1]/team/project@v2".into())
    );
    assert_eq!(
        remote("git@git.example:team/project@v2.git").unwrap(),
        remote("ssh://git@git.example/~/team/project@v2.git").unwrap()
    );
}
