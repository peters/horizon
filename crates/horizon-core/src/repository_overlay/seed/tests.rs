use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        OverlayChange, OverlayContent, RepositoryOverlayPlan,
        bundle::{RepositoryOverlayBundle, VerifiedOverlayBlob},
        namespace::resolve_namespaces,
    },
};
use git2::{Index, IndexEntry, IndexTime, Odb, Repository, Signature};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    io::{self, Cursor, Write},
    os::unix::fs::{PermissionsExt, symlink},
};

struct Fixture {
    directory: tempfile::TempDir,
    repository: Repository,
    commit: Oid,
    ancestor: Oid,
    private_blob: Oid,
    base_blob: Oid,
}

fn private_fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

impl Fixture {
    fn new() -> Self {
        let directory = private_fixture();
        let repository = Repository::init(directory.path()).unwrap();
        let private_blob = repository.blob(b"unrelated older private data").unwrap();
        let base_blob = repository.blob(b"base\0literal\xff").unwrap();
        let ancestor = commit(&repository, &[("old", private_blob, 0o100_644)], &[]);
        let commit = commit(
            &repository,
            &[("kept", base_blob, 0o100_644), ("removed", base_blob, 0o100_644)],
            &[ancestor],
        );
        repository
            .config()
            .unwrap()
            .set_str("remote.private.url", "synthetic-private-remote")
            .unwrap();
        Self {
            directory,
            repository,
            commit,
            ancestor,
            private_blob,
            base_blob,
        }
    }

    fn resolve(
        &self,
        changes: Vec<OverlayChange>,
        working: Vec<OverlayChange>,
        blobs: Vec<VerifiedOverlayBlob>,
    ) -> ResolvedRepositoryOverlay {
        let source = GitSource {
            repository: "synthetic/project".into(),
            commit: GitCommitSha::parse(self.commit.to_string()).unwrap(),
            branch: None,
        };
        let bundle =
            RepositoryOverlayBundle::new(RepositoryOverlayPlan::new(source, changes, working).unwrap(), blobs).unwrap();
        resolve_namespaces(&self.repository, bundle).unwrap()
    }

    fn source(&self) -> LooseFixtureSource<'_> {
        LooseFixtureSource {
            database: self.repository.odb().unwrap(),
            calls: BTreeMap::new(),
        }
    }
}

// Fixture-only adapter: production does not mistake libgit2 loose streaming for packed support.
struct LooseFixtureSource<'a> {
    database: Odb<'a>,
    calls: BTreeMap<Oid, usize>,
}
impl GitObjectSource for LooseFixtureSource<'_> {
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        *self.calls.entry(object).or_default() += 1;
        let (reader, bytes, kind) = self.database.reader(object).map_err(|_| SeedError::Source)?;
        Ok(GitObjectStream {
            kind,
            bytes: bytes as u64,
            reader: Box::new(reader),
        })
    }
}

fn entry(path: &str, id: Oid, mode: u32) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

fn commit(repository: &Repository, files: &[(&str, Oid, u32)], parents: &[Oid]) -> Oid {
    let mut index = Index::new().unwrap();
    for (path, id, mode) in files {
        index.add(&entry(path, *id, *mode)).unwrap();
    }
    let tree = index.write_tree_to(repository).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let parents: Vec<_> = parents.iter().map(|id| repository.find_commit(*id).unwrap()).collect();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "Synthetic base",
            &repository.find_tree(tree).unwrap(),
            &parents.iter().collect::<Vec<_>>(),
        )
        .unwrap()
}

fn change(path: &str, content: OverlayContent) -> OverlayChange {
    OverlayChange::new(path.into(), content).unwrap()
}

fn file(path: &str, bytes: &[u8], executable: bool) -> (OverlayChange, VerifiedOverlayBlob) {
    let blob = VerifiedOverlayBlob::new(bytes.to_vec()).unwrap();
    (
        change(
            path,
            OverlayContent::File {
                sha256: blob.sha256().clone(),
                bytes: bytes.len() as u64,
                executable,
            },
        ),
        blob,
    )
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32)> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                files.insert(
                    entry.path(),
                    (
                        fs::read(entry.path()).unwrap(),
                        entry.metadata().unwrap().permissions().mode(),
                    ),
                );
            }
        }
    }
    files
}

#[test]
fn seed_preserves_exact_base_and_independent_index_without_worktree_or_private_history() {
    let fixture = Fixture::new();
    let before = snapshot(fixture.directory.path());
    let parent = private_fixture();
    let (staged, staged_blob) = file("nested/staged", b"staged\0\xff", true);
    let (working, working_blob) = file("nested/staged", b"working-only", false);
    let working_id = Oid::hash_object(ObjectType::Blob, working_blob.bytes()).unwrap();
    let resolved = fixture.resolve(
        vec![
            staged,
            change("removed", OverlayContent::Remove),
            change(
                "link",
                OverlayContent::Symlink {
                    target: "nested/staged".into(),
                },
            ),
        ],
        vec![working],
        vec![staged_blob, working_blob],
    );
    let mut source = fixture.source();
    let seed = prepare_git_seed(parent.path(), &resolved, &mut source, || false).unwrap();
    let repository = Repository::open(seed.path()).unwrap();
    assert_eq!(fs::metadata(seed.path()).unwrap().permissions().mode() & 0o7777, 0o700);
    assert_eq!(seed.base_commit(), fixture.commit);
    assert_eq!(repository.head().unwrap().target(), Some(fixture.commit));
    assert!(repository.head_detached().unwrap());
    assert!(repository.is_shallow());
    assert_eq!(repository.references().unwrap().count(), 0);
    let database = repository.odb().unwrap();
    assert!(!database.exists(fixture.ancestor));
    assert!(!database.exists(fixture.private_blob));
    assert!(!database.exists(working_id));
    assert!(database.exists(fixture.base_blob));
    let base = repository.find_commit(fixture.commit).unwrap().tree().unwrap();
    assert_eq!(base.get_name("removed").unwrap().id(), fixture.base_blob);
    assert_eq!(
        database.read(fixture.commit).unwrap().data(),
        fixture.repository.odb().unwrap().read(fixture.commit).unwrap().data()
    );
    let index = repository.index().unwrap();
    assert_eq!(index.len(), 3);
    assert!(index.get_path(Path::new("removed"), 0).is_none());
    let staged = index.get_path(Path::new("nested/staged"), 0).unwrap();
    assert_eq!(staged.mode, 0o100_755);
    assert_eq!(repository.find_blob(staged.id).unwrap().content(), b"staged\0\xff");
    let link = index.get_path(Path::new("link"), 0).unwrap();
    assert_eq!(link.mode, 0o120_000);
    assert_eq!(repository.find_blob(link.id).unwrap().content(), b"nested/staged");
    assert!(!index.has_conflicts());
    assert_eq!(fs::read_dir(seed.path()).unwrap().count(), 1);
    assert!(!repository.path().join("objects/info/alternates").exists());
    assert!(!repository.path().join("logs").exists());
    assert!(
        !fs::read_to_string(repository.path().join("config"))
            .unwrap()
            .contains("private")
    );
    assert!(source.calls.values().all(|count| *count == 1));
    assert_eq!(snapshot(fixture.directory.path()), before);
    assert!(!format!("{seed:?}").contains(seed.path().to_str().unwrap()));
    let retained = seed.path().to_path_buf();
    drop(seed);
    assert!(retained.exists());
}

#[test]
fn streams_large_removed_base_blob_without_the_capture_limit() {
    let mut fixture = Fixture::new();
    let size = 64 * 1024 * 1024 + 1;
    let database = fixture.repository.odb().unwrap();
    let mut writer = database.writer(size, ObjectType::Blob).unwrap();
    let buffer = [19u8; 16 * 1024];
    for _ in 0..4096 {
        writer.write_all(&buffer).unwrap();
    }
    writer.write_all(&[19]).unwrap();
    let id = writer.finalize().unwrap();
    drop(writer);
    drop(database);
    fixture.commit = commit(&fixture.repository, &[("large", id, 0o100_644)], &[fixture.commit]);
    let resolved = fixture.resolve(vec![change("large", OverlayContent::Remove)], vec![], vec![]);
    let parent = private_fixture();
    let mut source = fixture.source();
    let seed = prepare_git_seed(parent.path(), &resolved, &mut source, || false).unwrap();
    let repository = Repository::open(seed.path()).unwrap();
    assert!(repository.index().unwrap().is_empty());
    assert_eq!(
        repository.odb().unwrap().read_header(id).unwrap(),
        (size, ObjectType::Blob)
    );
    assert_eq!(source.calls.get(&id), Some(&1));
    assert_eq!(seed.imported_objects(), 3);
}

struct BrokenSource {
    kind: ObjectType,
    declared: u64,
    data: Vec<u8>,
    fail: bool,
}
impl GitObjectSource for BrokenSource {
    fn open(&mut self, _: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        if self.fail {
            return Err(SeedError::Source);
        }
        Ok(GitObjectStream {
            kind: self.kind,
            bytes: self.declared,
            reader: Box::new(Cursor::new(&self.data)),
        })
    }
}

#[test]
fn inconsistent_sources_retain_only_private_unready_residues() {
    let fixture = Fixture::new();
    let resolved = fixture.resolve(vec![], vec![], vec![]);
    let raw = fixture
        .repository
        .odb()
        .unwrap()
        .read(fixture.commit)
        .unwrap()
        .data()
        .to_vec();
    for (kind, declared, data, fail, expected) in [
        (
            ObjectType::Blob,
            raw.len() as u64,
            raw.clone(),
            false,
            SeedError::Object,
        ),
        (
            ObjectType::Commit,
            raw.len() as u64 + 1,
            raw.clone(),
            false,
            SeedError::Object,
        ),
        (
            ObjectType::Commit,
            raw.len() as u64 - 1,
            raw.clone(),
            false,
            SeedError::Object,
        ),
        (ObjectType::Commit, 3, b"bad".to_vec(), false, SeedError::Object),
        (
            ObjectType::Commit,
            super::super::MAX_METADATA_BYTES as u64 + 1,
            vec![],
            false,
            SeedError::Limit,
        ),
        (ObjectType::Commit, 0, vec![], true, SeedError::Source),
    ] {
        let parent = private_fixture();
        fs::write(parent.path().join("existing"), b"preserve").unwrap();
        let failure = prepare_git_seed(
            parent.path(),
            &resolved,
            &mut BrokenSource {
                kind,
                declared,
                data,
                fail,
            },
            || false,
        )
        .unwrap_err();
        assert_eq!(failure.reason, expected);
        let residue = failure.residue().unwrap().to_path_buf();
        assert_eq!(residue.parent(), Some(parent.path()));
        assert_eq!(fs::read(parent.path().join("existing")).unwrap(), b"preserve");
        assert!(!format!("{failure:?}: {failure}").contains(residue.to_str().unwrap()));
        assert!(!Repository::open(&residue).unwrap().head_detached().unwrap());
        drop(failure);
        assert!(residue.is_dir());
    }
}

#[test]
fn unsafe_parents_and_early_cancellation_do_not_create_residues_or_read_source() {
    let fixture = Fixture::new();
    let resolved = fixture.resolve(vec![], vec![], vec![]);
    let parent = private_fixture();
    let target = private_fixture();
    symlink(target.path(), parent.path().join("alias")).unwrap();
    let mut source = fixture.source();
    for path in [
        parent.path().join("alias"),
        PathBuf::from("relative"),
        parent.path().join("../missing"),
    ] {
        let error = prepare_git_seed(&path, &resolved, &mut source, || false).unwrap_err();
        assert_eq!(error.reason, SeedError::UnsafeParent);
        assert!(error.residue().is_none());
    }
    fs::set_permissions(target.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        prepare_git_seed(target.path(), &resolved, &mut source, || false)
            .unwrap_err()
            .reason,
        SeedError::UnsafeParent
    );
    let error = prepare_git_seed(parent.path(), &resolved, &mut source, || true).unwrap_err();
    assert_eq!(error.reason, SeedError::Cancelled);
    assert!(error.residue().is_none());
    assert!(source.calls.is_empty());
    assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
}

#[test]
fn cancellation_after_reservation_or_first_stream_chunk_keeps_an_unready_directory() {
    let fixture = Fixture::new();
    let resolved = fixture.resolve(vec![], vec![], vec![]);
    for cancel_at in [2, 5] {
        let parent = private_fixture();
        let calls = Cell::new(0);
        let mut source = fixture.source();
        let error = prepare_git_seed(parent.path(), &resolved, &mut source, || {
            calls.set(calls.get() + 1);
            calls.get() >= cancel_at
        })
        .unwrap_err();
        assert_eq!(error.reason, SeedError::Cancelled);
        if cancel_at == 2 {
            assert_eq!(fs::read_dir(error.residue().unwrap()).unwrap().count(), 0);
            assert!(source.calls.is_empty());
        } else {
            assert_eq!(source.calls.len(), 1);
            assert!(
                !Repository::open(error.residue().unwrap())
                    .unwrap()
                    .head_detached()
                    .unwrap()
            );
        }
    }
}

#[test]
fn importer_budgets_exact_boundaries_and_rejects_overflow() {
    let mut bytes = 0;
    let mut metadata = 0;
    assert!(
        import::charge(
            0,
            &mut bytes,
            &mut metadata,
            ObjectType::Commit,
            super::super::MAX_METADATA_BYTES as u64
        )
        .is_ok()
    );
    assert_eq!(
        import::charge(1, &mut bytes, &mut metadata, ObjectType::Tree, 1),
        Err(SeedError::Limit)
    );
    bytes = u64::MAX;
    assert_eq!(
        import::charge(0, &mut bytes, &mut metadata, ObjectType::Blob, 1),
        Err(SeedError::Limit)
    );
    assert_eq!(
        import::charge(super::super::MAX_CHANGES * 2 + 1, &mut 0, &mut 0, ObjectType::Blob, 0),
        Err(SeedError::Limit)
    );
}

struct FailedRead;
impl Read for FailedRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("private source detail"))
    }
}

#[test]
fn transfer_read_errors_are_redacted() {
    struct Source;
    impl GitObjectSource for Source {
        fn open(&mut self, _: Oid) -> Result<GitObjectStream<'_>, SeedError> {
            Ok(GitObjectStream {
                kind: ObjectType::Commit,
                bytes: 1,
                reader: Box::new(FailedRead),
            })
        }
    }
    let fixture = Fixture::new();
    let parent = private_fixture();
    let error = prepare_git_seed(
        parent.path(),
        &fixture.resolve(vec![], vec![], vec![]),
        &mut Source,
        || false,
    )
    .unwrap_err();
    assert_eq!(error.reason, SeedError::Source);
    assert!(!format!("{error:?}").contains("private source detail"));
}
