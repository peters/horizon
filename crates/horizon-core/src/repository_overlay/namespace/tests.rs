use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{OverlayChange, OverlayContent, RepositoryOverlayPlan, bundle::VerifiedOverlayBlob},
};
use git2::{Index, IndexEntry, IndexTime, Signature};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    directory: tempfile::TempDir,
    repository: Repository,
    commit: Oid,
}

impl Fixture {
    fn new(files: &[(&str, &[u8], u32)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init(directory.path()).unwrap();
        let mut index = Index::new().unwrap();
        for (path, bytes, mode) in files {
            index
                .add(&IndexEntry {
                    ctime: IndexTime::new(0, 0),
                    mtime: IndexTime::new(0, 0),
                    dev: 0,
                    ino: 0,
                    mode: *mode,
                    uid: 0,
                    gid: 0,
                    file_size: u32::try_from(bytes.len()).unwrap(),
                    id: repository.blob(bytes).unwrap(),
                    flags: 0,
                    flags_extended: 0,
                    path: path.as_bytes().to_vec(),
                })
                .unwrap();
        }
        let tree = index.write_tree_to(&repository).unwrap();
        let commit = commit(&repository, tree);
        Self {
            directory,
            repository,
            commit,
        }
    }

    fn bundle(
        &self,
        index: Vec<OverlayChange>,
        working: Vec<OverlayChange>,
        blobs: Vec<VerifiedOverlayBlob>,
    ) -> RepositoryOverlayBundle {
        let source = GitSource {
            repository: "synthetic/project".into(),
            commit: GitCommitSha::parse(self.commit.to_string()).unwrap(),
            branch: None,
        };
        RepositoryOverlayBundle::new(RepositoryOverlayPlan::new(source, index, working).unwrap(), blobs).unwrap()
    }

    fn resolve(
        &self,
        index: Vec<OverlayChange>,
        working: Vec<OverlayChange>,
        blobs: Vec<VerifiedOverlayBlob>,
    ) -> Result<ResolvedRepositoryOverlay, NamespaceError> {
        resolve_namespaces(&self.repository, self.bundle(index, working, blobs))
    }
}

fn commit(repository: &Repository, tree: Oid) -> Oid {
    let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "Synthetic base",
            &repository.find_tree(tree).unwrap(),
            &[],
        )
        .unwrap()
}

fn file(path: &str, bytes: &[u8], executable: bool) -> (OverlayChange, VerifiedOverlayBlob) {
    let blob = VerifiedOverlayBlob::new(bytes.to_vec()).unwrap();
    let change = OverlayChange::new(
        path.into(),
        OverlayContent::File {
            sha256: blob.sha256().clone(),
            bytes: bytes.len() as u64,
            executable,
        },
    )
    .unwrap();
    (change, blob)
}

fn remove(path: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Remove).unwrap()
}

fn link(path: &str, target: &str) -> OverlayChange {
    OverlayChange::new(path.into(), OverlayContent::Symlink { target: target.into() }).unwrap()
}

fn contents(
    fixture: &Fixture,
    result: &ResolvedRepositoryOverlay,
    tree: &RepositoryNamespace,
    path: &str,
) -> (Vec<u8>, bool) {
    let NamespaceEntry::File { source, executable } = tree.entry(path).unwrap() else {
        panic!("file expected")
    };
    let bytes = match source {
        NamespaceFile::Base { object, .. } => fixture.repository.find_blob(*object).unwrap().content().to_vec(),
        NamespaceFile::Overlay { sha256, .. } => result.bundle().blob(sha256).unwrap().to_vec(),
    };
    (bytes, *executable)
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                files.insert(entry.path(), fs::read(entry.path()).unwrap());
            }
        }
    }
    files
}

#[test]
fn exact_base_and_two_layers_preserve_independent_file_semantics_without_writes() {
    let fixture = Fixture::new(&[
        ("keep", b"base", 0o100_755),
        ("nested/file", b"original", 0o100_644),
        ("old-name", b"rename", 0o100_644),
        ("delete-working", b"gone", 0o100_644),
    ]);
    let (staged, index_blob) = file("nested/file", b"staged\r\n", false);
    let (working, working_blob) = file("nested/file", b"working\0\xff", true);
    let (renamed, rename_blob) = file("new-name", b"rename", false);
    let (untracked, untracked_blob) = file("untracked", b"local", false);
    fs::write(fixture.directory.path().join("not-selected"), b"never read").unwrap();
    let before = snapshot(fixture.directory.path());
    let result = fixture
        .resolve(
            vec![staged, remove("old-name"), renamed, link("alias", "keep")],
            vec![
                working,
                untracked,
                remove("delete-working"),
                link("alias", "nested/file"),
            ],
            vec![index_blob, working_blob, rename_blob, untracked_blob],
        )
        .unwrap();
    assert_eq!(result.base_commit(), fixture.commit);
    assert_eq!(
        result.base_tree(),
        fixture.repository.find_commit(fixture.commit).unwrap().tree_id()
    );
    assert_eq!(
        contents(&fixture, &result, result.base(), "nested/file"),
        (b"original".to_vec(), false)
    );
    assert_eq!(
        contents(&fixture, &result, result.index(), "nested/file"),
        (b"staged\r\n".to_vec(), false)
    );
    assert_eq!(
        contents(&fixture, &result, result.working_tree(), "nested/file"),
        (b"working\0\xff".to_vec(), true)
    );
    assert_eq!(
        contents(&fixture, &result, result.working_tree(), "keep"),
        (b"base".to_vec(), true)
    );
    assert!(result.base().entry("old-name").is_some() && result.index().entry("old-name").is_none());
    assert!(
        result.index().entry("delete-working").is_some() && result.working_tree().entry("delete-working").is_none()
    );
    assert!(result.index().entry("untracked").is_none() && result.working_tree().entry("untracked").is_some());
    assert_eq!(
        result.index().entry("alias"),
        Some(&NamespaceEntry::Symlink { target: "keep".into() })
    );
    assert_eq!(
        result.working_tree().entry("alias"),
        Some(&NamespaceEntry::Symlink {
            target: "nested/file".into()
        })
    );
    assert!(result.working_tree().entry("not-selected").is_none());
    assert_eq!(snapshot(fixture.directory.path()), before);
}

#[test]
fn whole_layer_transitions_never_remove_unselected_descendants_recursively() {
    let fixture = Fixture::new(&[("dir/a", b"a", 0o100_644), ("dir/b", b"b", 0o100_644)]);
    assert!(matches!(
        fixture.resolve(vec![remove("dir")], vec![], vec![]),
        Err(NamespaceError::Topology)
    ));
    let (replacement, blob) = file("dir", b"replacement", false);
    assert!(matches!(
        fixture.resolve(vec![remove("dir/a"), replacement], vec![], vec![blob]),
        Err(NamespaceError::Topology)
    ));
    let (replacement, blob) = file("dir", b"replacement", false);
    let (child, child_blob) = file("dir/new/child", b"new", false);
    let result = fixture
        .resolve(
            vec![remove("dir/a"), remove("dir/b"), replacement],
            vec![remove("dir"), child],
            vec![blob, child_blob],
        )
        .unwrap();
    assert!(result.index().entry("dir").is_some() && result.index().entry("dir/a").is_none());
    assert!(result.working_tree().entry("dir").is_none() && result.working_tree().entry("dir/new/child").is_some());
    let fixture = Fixture::new(&[("file", b"base", 0o100_644)]);
    let (child, blob) = file("file/child", b"new", false);
    assert!(matches!(
        fixture.resolve(vec![child], vec![], vec![blob]),
        Err(NamespaceError::Topology)
    ));
}

#[test]
fn effective_links_reject_escapes_cycles_and_file_ancestors_in_every_namespace() {
    for layers in [0, 1, 2] {
        let files = if layers == 0 {
            vec![
                ("d/up", &b".."[..], 0o120_000),
                ("escape", &b"d/up/../outside"[..], 0o120_000),
            ]
        } else {
            vec![]
        };
        let fixture = Fixture::new(&files);
        let bad = vec![link("d/up", ".."), link("escape", "d/up/../outside")];
        let (index, working) = match layers {
            0 => (vec![], vec![]),
            1 => (bad, vec![]),
            _ => (vec![], bad),
        };
        assert!(matches!(
            fixture.resolve(index, working, vec![]),
            Err(NamespaceError::Link)
        ));
    }
    let fixture = Fixture::new(&[("ordinary", b"base", 0o100_644)]);
    for changes in [
        vec![link("a", "b"), link("b", "a")],
        vec![link("a", "a")],
        vec![link("a", "ordinary/child")],
    ] {
        assert!(matches!(
            fixture.resolve(changes, vec![], vec![]),
            Err(NamespaceError::Link)
        ));
    }
    let result = fixture
        .resolve(
            vec![
                link("nested/dangling", "../missing/leaf"),
                link("root", "."),
                link("alias", "root/ordinary"),
            ],
            vec![],
            vec![],
        )
        .unwrap();
    assert_eq!(result.index().entries().len(), 4);
}

#[test]
fn exact_commit_is_independent_of_head_and_lfs_filters_do_not_run() {
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 8\n";
    let fixture = Fixture::new(&[
        ("asset.bin", pointer, 0o100_644),
        (".gitattributes", b"*.bin filter=synthetic\n", 0o100_644),
    ]);
    fixture.repository.set_head_detached(fixture.commit).unwrap();
    fixture
        .repository
        .config()
        .unwrap()
        .set_str("filter.synthetic.smudge", "false")
        .unwrap();
    fixture
        .repository
        .config()
        .unwrap()
        .set_bool("filter.synthetic.required", true)
        .unwrap();
    fixture.repository.set_head("refs/heads/unborn-other-head").unwrap();
    let (hydrated, blob) = file("asset.bin", b"hydrated", false);
    let before = snapshot(fixture.directory.path());
    let result = fixture.resolve(vec![], vec![hydrated], vec![blob]).unwrap();
    assert_eq!(contents(&fixture, &result, result.index(), "asset.bin").0, pointer);
    assert_eq!(
        contents(&fixture, &result, result.working_tree(), "asset.bin").0,
        b"hydrated"
    );
    assert_eq!(snapshot(fixture.directory.path()), before);
}

#[test]
fn excluded_base_paths_and_missing_base_fail_with_redacted_diagnostics() {
    let fixture = Fixture::new(&[(".env.private-marker", b"private-marker", 0o100_644)]);
    let error = fixture.resolve(vec![], vec![], vec![]).unwrap_err();
    assert!(matches!(error, NamespaceError::Policy(_)));
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains("private-marker"));
    }
    let empty = Fixture::new(&[]);
    assert!(resolve_namespaces(&empty.repository, fixture.bundle(vec![], vec![], vec![])).is_err());
    let result = empty.resolve(vec![], vec![], vec![]).unwrap();
    assert!(!format!("{result:?}").contains("synthetic/project"));
}

#[test]
fn expanded_budgets_count_paths_logical_bytes_and_shared_implicit_directories() {
    let mut budget = Budget {
        entries: MAX_CHANGES,
        ..Budget::default()
    };
    assert_eq!(budget.add("next", None, 0), Err(NamespaceError::Limit));
    let mut budget = Budget {
        metadata: MAX_METADATA_BYTES,
        ..Budget::default()
    };
    assert_eq!(budget.add("next", None, 0), Err(NamespaceError::Limit));
    let mut budget = Budget {
        logical: MAX_CONTENT_BYTES,
        ..Budget::default()
    };
    assert_eq!(budget.add("next", None, 1), Err(NamespaceError::Limit));
    let fixture = Fixture::new(&[("same/a", b"repeat", 0o100_644), ("same/b", b"repeat", 0o100_644)]);
    let result = fixture.resolve(vec![], vec![], vec![]).unwrap();
    assert_eq!(result.base().logical_bytes(), 12);
    let mut namespace = RepositoryNamespace::default();
    for n in 0..=(MAX_CHANGES / 2) {
        namespace.entries.insert(
            format!("d{n}/leaf"),
            NamespaceEntry::File {
                source: NamespaceFile::Base {
                    object: fixture.commit,
                    bytes: 0,
                },
                executable: false,
            },
        );
    }
    assert_eq!(
        tree::validate(&mut namespace, &mut links::Work::default()),
        Err(NamespaceError::Limit)
    );
    let leaf = namespace.entries.values().next().unwrap().clone();
    namespace.entries.clear();
    for n in 0..MAX_CHANGES - 1 {
        namespace.entries.insert(format!("shared/{n}"), leaf.clone());
    }
    tree::validate(&mut namespace, &mut links::Work::default()).unwrap();
    namespace.entries.insert("sibling".into(), leaf.clone());
    assert_eq!(
        tree::validate(&mut namespace, &mut links::Work::default()),
        Err(NamespaceError::Limit)
    );
    let prefix = "d/".repeat(1000);
    namespace.entries = (0..1000).map(|n| (format!("{prefix}{n}"), leaf.clone())).collect();
    tree::validate(&mut namespace, &mut links::Work::default()).unwrap();
}

#[test]
fn unsupported_and_inconsistent_raw_git_nodes_are_rejected() {
    let mut fixture = Fixture::new(&[]);
    let database = fixture.repository.odb().unwrap();
    let blob = database.write(git2::ObjectType::Blob, b"literal").unwrap();
    let empty = database.write(git2::ObjectType::Tree, b"").unwrap();
    for (case, entries) in [
        vec![("100644", &b"valid"[..], blob)],
        vec![("40000", &b"empty-directory"[..], empty)],
        vec![("160000", &b"module"[..], fixture.commit)],
        vec![("100644", &b"wrong-kind"[..], empty)],
        vec![("40000", &b"wrong-kind"[..], blob)],
        vec![("100644", &b"invalid\xff"[..], blob)],
        vec![("100644", &b"name/with-slash"[..], blob)],
        vec![("100644", &b"same"[..], blob), ("100644", &b"same"[..], blob)],
        vec![("40000", &b"same"[..], empty), ("40000", &b"same"[..], empty)],
        vec![("100644", &b"same"[..], blob), ("40000", &b"same"[..], empty)],
        vec![("100644", &b"missing"[..], Oid::ZERO_SHA1)],
    ]
    .into_iter()
    .enumerate()
    {
        let mut bytes = Vec::new();
        for (mode, name, object) in entries {
            bytes.extend_from_slice(mode.as_bytes());
            bytes.push(b' ');
            bytes.extend_from_slice(name);
            bytes.push(0);
            bytes.extend_from_slice(object.as_bytes());
        }
        let tree = database.write(git2::ObjectType::Tree, &bytes).unwrap();
        let raw_commit = format!(
            "tree {tree}\nauthor Fixture <fixture@example.invalid> 1 +0000\ncommitter Fixture <fixture@example.invalid> 1 +0000\n\nSynthetic base\n"
        );
        fixture.commit = database.write(git2::ObjectType::Commit, raw_commit.as_bytes()).unwrap();
        let result = fixture.resolve(vec![], vec![], vec![]);
        if case == 0 {
            let result = result.unwrap();
            assert_eq!(result.base().entries().len(), 1);
            assert_eq!(contents(&fixture, &result, result.base(), "valid").0, b"literal");
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn link_hops_and_expanded_resolution_work_are_bounded() {
    let fixture = Fixture::new(&[]);
    let chain = |length: usize| {
        (0..length)
            .map(|n| link(&format!("link{n:02}"), &format!("link{:02}", n + 1)))
            .collect()
    };
    fixture.resolve(chain(40), vec![], vec![]).unwrap();
    assert!(matches!(
        fixture.resolve(chain(41), vec![], vec![]),
        Err(NamespaceError::Link)
    ));
    // Each target is bounded, but repeatedly traversing deep paths amplifies work.
    let prefix = "d/".repeat(1000);
    let mut namespace = RepositoryNamespace::default();
    for n in 0..80 {
        namespace.entries.insert(
            format!("link{n}"),
            NamespaceEntry::Symlink {
                target: format!("{prefix}missing"),
            },
        );
    }
    assert_eq!(
        tree::validate(&mut namespace, &mut links::Work::default()),
        Err(NamespaceError::Limit)
    );
}
