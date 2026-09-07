use super::*;
use crate::repository_overlay::{OverlayChange, OverlayContent, bundle::codec};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn file<'a>(bundle: &'a RepositoryOverlayBundle, layer: &[OverlayChange], path: &str) -> (&'a [u8], bool) {
    let change = layer.iter().find(|change| change.path() == path).unwrap();
    let OverlayContent::File { sha256, executable, .. } = change.content() else {
        panic!("file expected")
    };
    (bundle.blob(sha256).unwrap(), *executable)
}

#[test]
fn exact_staged_and_working_bytes_modes_links_and_removals_are_independent() {
    let fixture = Fixture::new();
    for path in ["nested/file", "removed", "renamed-from", "unchanged"] {
        fixture.stage(path, b"base", 0o100_644);
        fixture.write(path, b"base");
    }
    fixture.commit();
    fixture.stage("nested/file", b"staged\r\n", 0o100_644);
    fixture.write("nested/file", b"working\0\xff\n");
    fs::set_permissions(fixture.root().join("nested/file"), fs::Permissions::from_mode(0o755)).unwrap();
    fixture.stage("alias", b"nested/file", 0o120_000);
    symlink("unchanged", fixture.root().join("alias")).unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.remove_path(Path::new("removed")).unwrap();
    index.remove_path(Path::new("renamed-from")).unwrap();
    index.write().unwrap();
    fs::remove_file(fixture.root().join("removed")).unwrap();
    fs::remove_file(fixture.root().join("renamed-from")).unwrap();
    fixture.stage("renamed-to", b"base", 0o100_644);
    fixture.write("renamed-to", b"base");
    fixture.write("untracked", b"working\0\xff\n");
    fixture.write("not-selected", b"must never be included");
    let index_before = fs::read(fixture.repository.path().join("index")).unwrap();
    let config_before = fs::read(fixture.repository.path().join("config")).unwrap();
    let bundle = fixture.capture(&[
        "untracked",
        "unchanged",
        "removed",
        "renamed-from",
        "renamed-to",
        "alias",
        "nested/file",
    ]);
    let plan = bundle.plan();
    assert_eq!(file(&bundle, plan.index(), "nested/file"), (&b"staged\r\n"[..], false));
    assert_eq!(
        file(&bundle, plan.working_tree(), "nested/file"),
        (&b"working\0\xff\n"[..], true)
    );
    assert_eq!(
        file(&bundle, plan.working_tree(), "untracked"),
        (&b"working\0\xff\n"[..], false)
    );
    assert_eq!(bundle.blobs().len(), 3);
    assert_eq!(plan.index().len(), 5);
    assert_eq!(plan.working_tree().len(), 3);
    for path in ["removed", "renamed-from"] {
        assert!(
            plan.index()
                .iter()
                .any(|change| change.path() == path && matches!(change.content(), OverlayContent::Remove))
        );
    }
    assert!(plan.index().iter().any(|change| change.path() == "alias"
        && matches!(change.content(), OverlayContent::Symlink { target } if target == "nested/file")));
    assert!(plan.working_tree().iter().any(|change| change.path() == "alias"
        && matches!(change.content(), OverlayContent::Symlink { target } if target == "unchanged")));
    assert_eq!(fs::read(fixture.repository.path().join("index")).unwrap(), index_before);
    assert_eq!(
        fs::read(fixture.repository.path().join("config")).unwrap(),
        config_before
    );
    fixture.write("nested/file", b"later edit");
    assert_eq!(codec::decode(&codec::encode(&bundle).unwrap()).unwrap(), bundle);
}

#[test]
fn lfs_pointer_and_hydrated_bytes_are_literal_without_filters_or_selection_expansion() {
    let fixture = Fixture::new();
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 8\n";
    fixture.stage("asset.bin", pointer, 0o100_644);
    fixture.write("asset.bin", b"hydrated");
    fixture.write(".gitattributes", b"*.bin filter=synthetic diff=synthetic\n");
    fixture.write("ignored", b"not selected");
    fixture.write(".gitignore", b"ignored\n");
    let mut config = fixture.repository.config().unwrap();
    config.set_str("filter.synthetic.clean", "false").unwrap();
    config.set_str("filter.synthetic.smudge", "false").unwrap();
    config.set_bool("filter.synthetic.required", true).unwrap();
    let bundle = fixture.capture(&["asset.bin"]);
    assert_eq!(file(&bundle, bundle.plan().index(), "asset.bin").0, pointer);
    assert_eq!(file(&bundle, bundle.plan().working_tree(), "asset.bin").0, b"hydrated");
    assert_eq!(bundle.blobs().len(), 2);
}

#[test]
fn working_deletion_staged_deletion_with_untracked_replacement_and_unchanged_selection() {
    let fixture = Fixture::new();
    fixture.stage("gone", b"base", 0o100_644);
    fixture.stage("replaced", b"base", 0o100_644);
    fixture.commit();
    fixture.write("replaced", b"untracked after staged deletion");
    let mut index = fixture.repository.index().unwrap();
    index.remove_path(Path::new("replaced")).unwrap();
    index.write().unwrap();
    let bundle = fixture.capture(&["gone", "replaced", "never-existed"]);
    assert_eq!(bundle.plan().index().len(), 1);
    assert_eq!(bundle.plan().working_tree().len(), 2);
    assert!(matches!(
        bundle.plan().working_tree()[0].content(),
        OverlayContent::Remove
    ));
    assert_eq!(
        file(&bundle, bundle.plan().working_tree(), "replaced").0,
        b"untracked after staged deletion"
    );
    let empty = fixture.capture(&[]);
    assert_eq!(empty.blobs().len(), 0);
    assert!(empty.plan().index().is_empty() && empty.plan().working_tree().is_empty());
}

#[test]
fn linked_worktree_selects_its_own_head_index_and_working_bytes() {
    let fixture = Fixture::new();
    let destination = tempfile::tempdir().unwrap();
    let root = destination.path().join("linked");
    fixture.repository.worktree("synthetic-linked", &root, None).unwrap();
    fs::write(root.join("local"), b"linked bytes").unwrap();
    fixture.write("local", b"main bytes");
    let bundle = capture_selected(&root, fixture.source(), &["local"]).unwrap();
    assert_eq!(file(&bundle, bundle.plan().working_tree(), "local").0, b"linked bytes");
}

#[test]
fn case_folding_cannot_select_differently_spelled_index_paths_or_ancestors() {
    for selected_is_indexed in [false, true] {
        let fixture = Fixture::new();
        fixture.stage("File", b"unselected uppercase payload", 0o100_644);
        fixture.stage("Dir", b"unselected ancestor spelling", 0o100_644);
        if selected_is_indexed {
            fixture.stage("file", b"selected staged payload", 0o100_644);
        }
        fixture.write("file", b"selected working payload");
        fixture.write("dir/file", b"selected nested payload");
        fixture
            .repository
            .config()
            .unwrap()
            .set_bool("core.ignorecase", true)
            .unwrap();
        let bundle = fixture.capture(&["file", "dir/file"]);
        assert_eq!(bundle.plan().index().len(), usize::from(selected_is_indexed));
        if selected_is_indexed {
            assert_eq!(
                file(&bundle, bundle.plan().index(), "file").0,
                b"selected staged payload"
            );
        }
        assert_eq!(
            file(&bundle, bundle.plan().working_tree(), "file").0,
            b"selected working payload"
        );
        assert_eq!(
            file(&bundle, bundle.plan().working_tree(), "dir/file").0,
            b"selected nested payload"
        );
        assert!(bundle.blobs().all(|blob| !blob.bytes().starts_with(b"unselected")));
    }
}
