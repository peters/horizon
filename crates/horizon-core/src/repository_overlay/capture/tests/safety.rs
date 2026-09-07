use super::*;
use crate::repository_overlay::{
    bundle::OverlayBundleError,
    reader::{MAX_READ_BYTES, SelectedRepositoryNode},
};
use std::{fs, os::unix::fs::symlink};

#[test]
fn roots_and_exact_heads_never_fall_back_to_parent_or_unborn_repository() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root().join("nested")).unwrap();
    assert_eq!(
        capture_selected(&fixture.root().join("nested"), fixture.source(), &[]),
        Err(GitCaptureError::Repository)
    );
    assert_eq!(
        capture_selected(fixture.root(), source(), &[]),
        Err(GitCaptureError::BaseMismatch)
    );
    let bare = tempfile::tempdir().unwrap();
    git2::Repository::init_bare(bare.path()).unwrap();
    assert_eq!(
        capture_selected(bare.path(), source(), &[]),
        Err(GitCaptureError::Repository)
    );
    let unborn = tempfile::tempdir().unwrap();
    git2::Repository::init(unborn.path()).unwrap();
    assert_eq!(
        capture_selected(unborn.path(), source(), &[]),
        Err(GitCaptureError::BaseMismatch)
    );
    let linked = tempfile::tempdir().unwrap();
    symlink(fixture.root(), linked.path().join("root")).unwrap();
    assert!(matches!(
        capture_selected(&linked.path().join("root"), fixture.source(), &[]),
        Err(GitCaptureError::Read(_))
    ));
}

#[test]
fn selected_working_nodes_never_follow_linked_parents_or_read_special_nodes() {
    let fixture = Fixture::new();
    fixture.write("destination/file", b"outside selection");
    symlink("destination", fixture.root().join("linked")).unwrap();
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["linked/file"]),
        Err(RepositoryReadError::UnsafePath.into())
    );
    symlink("../outside", fixture.root().join("escape")).unwrap();
    assert!(capture_selected(fixture.root(), fixture.source(), &["escape"]).is_err());
    fs::hard_link(fixture.root().join("destination/file"), fixture.root().join("hard")).unwrap();
    rustix::fs::mkfifoat(rustix::fs::CWD, fixture.root().join("fifo"), rustix::fs::Mode::RUSR).unwrap();
    for path in ["hard", "fifo", "destination"] {
        assert_eq!(
            capture_selected(fixture.root(), fixture.source(), &[path]),
            Err(RepositoryReadError::UnsupportedNode.into())
        );
    }
    assert_eq!(
        fs::read(fixture.root().join("destination/file")).unwrap(),
        b"outside selection"
    );
}

#[test]
fn unsafe_staged_links_and_gitlink_ancestors_are_rejected() {
    let fixture = Fixture::new();
    fixture.stage("link", b"../outside", 0o120_000);
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["link"]),
        Err(OverlayPlanError::InvalidLink.into())
    );
    fixture.stage("module", b"not a gitlink target", 0o160_000);
    fixture.write("module/file", b"nested repository contents");
    for path in ["module", "module/file"] {
        assert_eq!(
            capture_selected(fixture.root(), fixture.source(), &[path]),
            Err(GitCaptureError::UnsupportedNode)
        );
    }
}

#[test]
fn sparse_intent_to_add_and_unmerged_indexes_fail_closed() {
    let fixture = Fixture::new();
    fixture.stage("file", b"staged", 0o100_644);
    for flag in [
        git2::IndexEntryExtendedFlag::SKIP_WORKTREE,
        git2::IndexEntryExtendedFlag::INTENT_TO_ADD,
    ] {
        let mut index = fixture.repository.index().unwrap();
        let mut entry = index.get_path(Path::new("file"), 0).unwrap();
        entry.flags_extended = flag.bits();
        index.add(&entry).unwrap();
        index.write().unwrap();
        assert_eq!(
            capture_selected(fixture.root(), fixture.source(), &["file"]),
            Err(GitCaptureError::UnsupportedIndex)
        );
    }
    let mut index = fixture.repository.index().unwrap();
    let mut entry = index.get_path(Path::new("file"), 0).unwrap();
    index.clear().unwrap();
    entry.flags_extended = 0;
    entry.flags = 1 << 12;
    index.add(&entry).unwrap();
    index.write().unwrap();
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["file"]),
        Err(GitCaptureError::UnsupportedIndex)
    );
}

#[test]
fn index_and_head_drift_fail_final_verification() {
    let fixture = Fixture::new();
    fixture.stage("file", b"before", 0o100_644);
    let state = linux::State::open(fixture.root(), &fixture.source()).unwrap();
    let baseline = state.selected_index(&["file"]).unwrap();
    fixture.stage("file", b"after", 0o100_644);
    assert_eq!(state.verify(&["file"], &baseline), Err(GitCaptureError::Changed));
    fixture.commit();
    assert_eq!(state.verify(&["file"], &baseline), Err(GitCaptureError::BaseMismatch));
}

#[test]
fn oversized_staged_and_working_files_fail_without_publishing_partial_results() {
    let fixture = Fixture::new();
    fixture.write("huge", b"");
    fs::File::options()
        .write(true)
        .open(fixture.root().join("huge"))
        .unwrap()
        .set_len(MAX_READ_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["huge"]),
        Err(RepositoryReadError::TooLarge.into())
    );
    fixture.stage("staged", &vec![0; MAX_READ_BYTES + 1], 0o100_644);
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["staged"]),
        Err(OverlayBundleError::FileLimit.into())
    );
}

#[test]
fn private_payload_collection_deduplicates_before_enforcing_aggregate_limit() {
    let mut contents = content::Contents::default();
    let node = |value, count| {
        Some(SelectedRepositoryNode::File {
            bytes: vec![value; count],
            executable: false,
        })
    };
    contents.change("first", node(0, MAX_READ_BYTES)).unwrap();
    contents.change("identical", node(0, MAX_READ_BYTES)).unwrap();
    contents.change("second", node(1, MAX_READ_BYTES)).unwrap();
    assert_eq!(
        contents.change("excess", node(2, 1)),
        Err(OverlayBundleError::BundleLimit.into())
    );
}

#[test]
fn configured_workdir_redirect_cannot_rebind_the_selected_root() {
    let fixture = Fixture::new();
    let unrelated = tempfile::tempdir().unwrap();
    fixture
        .repository
        .config()
        .unwrap()
        .set_str("core.worktree", unrelated.path().to_str().unwrap())
        .unwrap();
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &[]),
        Err(GitCaptureError::Repository)
    );
    assert_eq!(fs::read_dir(unrelated.path()).unwrap().count(), 0);
}

#[test]
fn removed_base_gitlink_still_cannot_select_nested_repository_payloads() {
    let fixture = Fixture::new();
    fixture.stage("module", b"temporary fixture object", 0o160_000);
    let mut index = fixture.repository.index().unwrap();
    let mut entry = index.get_path(Path::new("module"), 0).unwrap();
    entry.id = fixture.repository.head().unwrap().peel_to_commit().unwrap().id();
    index.add(&entry).unwrap();
    index.write().unwrap();
    fixture.commit();
    index.remove_path(Path::new("module")).unwrap();
    index.write().unwrap();
    fixture.write("module/file", b"nested repository payload");
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["module/file"]),
        Err(GitCaptureError::UnsupportedNode)
    );
}

#[test]
fn overlong_staged_link_is_rejected_before_payload_copy() {
    let fixture = Fixture::new();
    fixture.stage("link", &vec![b'a'; paths::MAX_PATH_BYTES + 1], 0o120_000);
    assert_eq!(
        capture_selected(fixture.root(), fixture.source(), &["link"]),
        Err(OverlayPlanError::InvalidLink.into())
    );
}
