use super::*;
use std::{cell::RefCell, process::Command};

fn git(repository: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn repository_host_and_credentials_cannot_select_the_publisher_or_reach_output() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "--quiet"]);
    git(
        root.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://source-user:synthetic-secret@gitlab.example/group/app.git",
        ],
    );
    let cancel = Cancellation::default();
    let events = RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let destination = preparation::automatic("Alice", root.path(), "cpu", &runner).unwrap();
    assert_eq!(
        destination,
        Destination::automatic_github("alice", "https://gitlab.example/group/app", "cpu").unwrap()
    );
    assert!(events.borrow().is_empty());
    git(
        root.path(),
        &["remote", "set-url", "origin", "git@github.com:group/app.git"],
    );
    let another = preparation::automatic("alice", root.path(), "cpu", &runner).unwrap();
    assert_ne!(destination.repository, another.repository);
    assert!(another.repository.starts_with("ghcr.io/alice/"));
    git(
        root.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://gitlab.example:8443/group/app.git",
        ],
    );
    let port = preparation::automatic("alice", root.path(), "cpu", &runner).unwrap();
    assert_ne!(destination.repository, port.repository);
    cancel.cancel();
    assert!(preparation::automatic("alice", root.path(), "cpu", &runner).is_err());
}

#[test]
#[ignore = "set HORIZON_TEST_REPOSITORY and HORIZON_TEST_SOURCE for a read-only real checkout check"]
fn real_repository_identity() {
    let repository = PathBuf::from(std::env::var_os("HORIZON_TEST_REPOSITORY").unwrap());
    let source = std::env::var("HORIZON_TEST_SOURCE").unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert_eq!(
        preparation::automatic("synthetic-account", &repository, "cpu", &runner).unwrap(),
        Destination::automatic_github("synthetic-account", &source, "cpu").unwrap()
    );
}
