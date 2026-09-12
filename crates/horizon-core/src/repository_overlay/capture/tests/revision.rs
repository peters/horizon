use super::*;
use crate::repository_overlay::{OverlayChange, OverlayContent, bundle::codec};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn branch(fixture: &Fixture) -> String {
    fixture.repository.head().unwrap().shorthand().unwrap().to_owned()
}

fn file<'a>(bundle: &'a RepositoryOverlayBundle, layer: &[OverlayChange], path: &str) -> (&'a [u8], bool) {
    let change = layer.iter().find(|change| change.path() == path).unwrap();
    let OverlayContent::File { sha256, executable, .. } = change.content() else {
        panic!("file expected")
    };
    (bundle.blob(sha256).unwrap(), *executable)
}

#[test]
fn full_selected_layers_survive_commit_without_expanding_selection_or_changing_v1() {
    let fixture = Fixture::new();
    fixture.stage("selected", b"committed", 0o100_644);
    fixture.write("selected", b"committed");
    fixture.stage("unselected", b"private unselected bytes", 0o100_644);
    fixture.commit();
    let enrolled = fixture.source();
    let branch = branch(&fixture);
    let capture =
        || capture_selected_revision(fixture.root(), enrolled.clone(), &branch, &["selected", "absent"]).unwrap();
    let first = capture();
    for layer in [first.plan().index(), first.plan().working_tree()] {
        assert_eq!(file(&first, layer, "selected"), (&b"committed"[..], false));
        assert!(matches!(layer[0].content(), OverlayContent::Remove));
        assert_eq!(layer.len(), 2);
    }
    assert_eq!(first.blobs().len(), 1);
    fixture.stage("selected", b"new committed bytes", 0o100_644);
    fixture.write("selected", b"new committed bytes");
    fixture.commit();
    let next = capture();
    assert_eq!(next.plan().source().commit, fixture.source().commit);
    assert_eq!(next.plan().source().branch.as_deref(), Some(branch.as_str()));
    assert_ne!(first.manifest_sha256(), next.manifest_sha256());
    for layer in [next.plan().index(), next.plan().working_tree()] {
        assert_eq!(file(&next, layer, "selected").0, b"new committed bytes");
    }
    assert_eq!(file(&first, first.plan().index(), "selected").0, b"committed");
    assert_eq!(
        capture_selected(fixture.root(), enrolled, &["selected"]),
        Err(GitCaptureError::BaseMismatch)
    );
    assert_eq!(fixture.capture(&["selected"]).blobs().len(), 0);
    let encoded = codec::encode(&next).unwrap();
    assert!(
        !encoded
            .windows(b"private unselected bytes".len())
            .any(|bytes| bytes == b"private unselected bytes")
    );
    assert_eq!(codec::decode(&encoded).unwrap(), next);
}

#[test]
fn literal_modes_links_untracked_removal_and_lfs_pointer_remain_independent() {
    let fixture = Fixture::new();
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 8\n";
    fixture.stage("asset", pointer, 0o100_644);
    fixture.write("asset", b"hydrated");
    fixture.stage("gone", b"committed gone", 0o100_644);
    fixture.stage("link", b"asset", 0o120_000);
    fixture.commit();
    fixture.write("untracked", b"literal\0\xff");
    symlink("untracked", fixture.root().join("link")).unwrap();
    fs::set_permissions(fixture.root().join("asset"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = fixture.repository.config().unwrap();
    config.set_str("filter.synthetic.clean", "false").unwrap();
    config.set_bool("filter.synthetic.required", true).unwrap();
    fixture.write(".gitattributes", b"asset filter=synthetic\n");
    let bundle = capture_selected_revision(
        fixture.root(),
        fixture.source(),
        &branch(&fixture),
        &["asset", "gone", "link", "untracked"],
    )
    .unwrap();
    assert_eq!(file(&bundle, bundle.plan().index(), "asset"), (&pointer[..], false));
    assert_eq!(
        file(&bundle, bundle.plan().working_tree(), "asset"),
        (&b"hydrated"[..], true)
    );
    assert_eq!(
        file(&bundle, bundle.plan().working_tree(), "untracked").0,
        b"literal\0\xff"
    );
    assert!(matches!(bundle.plan().index()[3].content(), OverlayContent::Remove));
    assert!(matches!(
        bundle.plan().working_tree()[1].content(),
        OverlayContent::Remove
    ));
    assert!(matches!(bundle.plan().index()[2].content(), OverlayContent::Symlink { target } if target == "asset"));
    assert!(
        matches!(bundle.plan().working_tree()[2].content(), OverlayContent::Symlink { target } if target == "untracked")
    );
}

#[test]
fn wrong_detached_unborn_and_redirected_worktrees_are_refused() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let branch = branch(&fixture);
    assert_eq!(
        capture_selected_revision(fixture.root(), source.clone(), "other", &[]),
        Err(GitCaptureError::BaseMismatch)
    );
    fixture
        .repository
        .set_head_detached(fixture.repository.head().unwrap().target().unwrap())
        .unwrap();
    assert_eq!(
        capture_selected_revision(fixture.root(), source.clone(), &branch, &[]),
        Err(GitCaptureError::BaseMismatch)
    );
    fixture.repository.set_head("refs/heads/unborn").unwrap();
    assert_eq!(
        capture_selected_revision(fixture.root(), source.clone(), "unborn", &[]),
        Err(GitCaptureError::BaseMismatch)
    );
    fixture.repository.set_head(&format!("refs/heads/{branch}")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    fixture
        .repository
        .config()
        .unwrap()
        .set_str("core.worktree", outside.path().to_str().unwrap())
        .unwrap();
    assert_eq!(
        capture_selected_revision(fixture.root(), source, &branch, &[]),
        Err(GitCaptureError::Repository)
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn moving_head_index_or_symbolic_branch_during_attempt_is_retryable_changed() {
    for movement in ["commit", "index", "branch", "detached"] {
        let fixture = Fixture::new();
        fixture.stage("file", b"before", 0o100_644);
        let branch = branch(&fixture);
        let state = linux::State::open_with(fixture.root(), |repo| {
            super::super::revision::branch_head(repo, &branch)
        })
        .unwrap();
        let baseline = state.selected_index(&["file"]).unwrap();
        match movement {
            "commit" => fixture.commit(),
            "index" => fixture.stage("file", b"after", 0o100_644),
            "branch" => {
                let commit = fixture.repository.find_commit(state.base).unwrap();
                fixture.repository.branch("other", &commit, false).unwrap();
                fixture.repository.set_head("refs/heads/other").unwrap();
            }
            _ => fixture.repository.set_head_detached(state.base).unwrap(),
        }
        assert_eq!(
            super::super::revision::verify(&state, &branch, &["file"], &baseline),
            Err(GitCaptureError::Changed),
            "{movement}"
        );
    }
}

#[test]
fn exclusions_removed_gitlink_ancestry_and_oversize_files_still_fail_closed() {
    let fixture = Fixture::new();
    let branch = branch(&fixture);
    for selected in [
        vec![".env"],
        vec![".git/config"],
        vec!["../escape"],
        vec!["same", "same"],
    ] {
        assert!(matches!(
            capture_selected_revision(fixture.root(), fixture.source(), &branch, &selected),
            Err(GitCaptureError::Policy(_))
        ));
    }
    fixture.stage("module", b"placeholder", 0o160_000);
    let mut index = fixture.repository.index().unwrap();
    let mut entry = index.get_path(Path::new("module"), 0).unwrap();
    entry.id = fixture.repository.head().unwrap().target().unwrap();
    index.add(&entry).unwrap();
    index.write().unwrap();
    fixture.commit();
    index.remove_path(Path::new("module")).unwrap();
    index.write().unwrap();
    fixture.write("module/file", b"not selected repository");
    assert_eq!(
        capture_selected_revision(fixture.root(), fixture.source(), &branch, &["module/file"]),
        Err(GitCaptureError::UnsupportedNode)
    );
    fixture.write("huge", b"");
    fs::File::options()
        .write(true)
        .open(fixture.root().join("huge"))
        .unwrap()
        .set_len(u64::try_from(crate::repository_overlay::reader::MAX_READ_BYTES).unwrap() + 1)
        .unwrap();
    assert!(matches!(
        capture_selected_revision(fixture.root(), fixture.source(), &branch, &["huge"]),
        Err(GitCaptureError::Read(RepositoryReadError::TooLarge))
    ));
}

#[test]
fn revision_keeps_unsupported_index_and_working_node_refusals() {
    let fixture = Fixture::new();
    fixture.stage("file", b"staged", 0o100_644);
    let branch = branch(&fixture);
    for flags in [
        git2::IndexEntryExtendedFlag::SKIP_WORKTREE,
        git2::IndexEntryExtendedFlag::INTENT_TO_ADD,
    ] {
        let mut index = fixture.repository.index().unwrap();
        let mut entry = index.get_path(Path::new("file"), 0).unwrap();
        entry.flags_extended = flags.bits();
        index.add(&entry).unwrap();
        index.write().unwrap();
        assert_eq!(
            capture_selected_revision(fixture.root(), fixture.source(), &branch, &["file"]),
            Err(GitCaptureError::UnsupportedIndex)
        );
    }
    let mut index = fixture.repository.index().unwrap();
    index.clear().unwrap();
    index.write().unwrap();
    fixture.write("parent/file", b"outside literal selection");
    symlink("parent", fixture.root().join("linked")).unwrap();
    fs::hard_link(fixture.root().join("parent/file"), fixture.root().join("hard")).unwrap();
    for path in ["linked/file", "hard", "parent"] {
        assert!(matches!(
            capture_selected_revision(fixture.root(), fixture.source(), &branch, &[path]),
            Err(GitCaptureError::Read(_))
        ));
    }
}
