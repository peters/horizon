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
