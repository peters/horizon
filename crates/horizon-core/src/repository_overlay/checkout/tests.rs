use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        OverlayChange, OverlayContent, RepositoryOverlayPlan,
        bundle::{RepositoryOverlayBundle, VerifiedOverlayBlob},
        namespace::resolve_namespaces,
        seed::{GitObjectStream, prepare_git_seed},
    },
};
use flate2::{Compression, write::ZlibEncoder};
use git2::{Index, IndexEntry, IndexTime, ObjectType, Odb, Repository, Signature};
use std::{
    cell::Cell,
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

pub(super) fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

pub(super) struct Source<'a>(pub(super) Odb<'a>);
impl GitObjectSource for Source<'_> {
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        // Synthetic fixture only; the production working writer never maps source objects.
        let (reader, bytes, kind) = self.0.reader(object).map_err(|_| SeedError::Source)?;
        Ok(GitObjectStream {
            reader: Box::new(reader),
            bytes: bytes as u64,
            kind,
        })
    }
}

pub(super) fn fixture(paths: &[(&str, Oid, u32)], repository: &Repository) -> Oid {
    let mut index = Index::new().unwrap();
    for (path, id, mode) in paths {
        index
            .add(&IndexEntry {
                ctime: IndexTime::new(0, 0),
                mtime: IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: *mode,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: *id,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })
            .unwrap();
    }
    let tree = index.write_tree_to(repository).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repository
        .commit(
            None,
            &signature,
            &signature,
            "Synthetic base",
            &repository.find_tree(tree).unwrap(),
            &[],
        )
        .unwrap()
}

pub(super) fn resolved(
    repository: &Repository,
    base: Oid,
    index: Vec<OverlayChange>,
    working: Vec<OverlayChange>,
    blobs: Vec<VerifiedOverlayBlob>,
) -> ResolvedRepositoryOverlay {
    let source = GitSource {
        repository: "synthetic/project".into(),
        commit: GitCommitSha::parse(base.to_string()).unwrap(),
        branch: None,
    };
    resolve_namespaces(
        repository,
        RepositoryOverlayBundle::new(RepositoryOverlayPlan::new(source, index, working).unwrap(), blobs).unwrap(),
    )
    .unwrap()
}

pub(super) fn file(path: &str, bytes: &[u8], executable: bool) -> (OverlayChange, VerifiedOverlayBlob) {
    let blob = VerifiedOverlayBlob::new(bytes.to_vec()).unwrap();
    (
        OverlayChange::new(
            path.into(),
            OverlayContent::File {
                sha256: blob.sha256().clone(),
                bytes: bytes.len() as u64,
                executable,
            },
        )
        .unwrap(),
        blob,
    )
}

pub(super) fn change(path: &str, content: OverlayContent) -> OverlayChange {
    OverlayChange::new(path.into(), content).unwrap()
}

#[test]
fn private_checkout_preserves_independent_layers_raw_bytes_links_and_source() {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let raw = b"base\0binary\xff\r\n";
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:synthetic\nsize 42\n";
    let blob = repository.blob(raw).unwrap();
    let base = fixture(
        &[
            ("kept", blob, 0o100_644),
            ("removed", blob, 0o100_644),
            ("from-file", blob, 0o100_644),
            ("from-dir/leaf", blob, 0o100_644),
            ("asset", repository.blob(pointer).unwrap(), 0o100_644),
        ],
        &repository,
    );
    let source_index = fs::read(repository.path().join("config")).unwrap();
    fs::write(source.path().join("sentinel"), raw).unwrap();
    let (staged, staged_blob) = file("nested/tool", b"staged\0tool", true);
    let (working, working_blob) = file("asset", b"hydrated\0asset", false);
    let (child, child_blob) = file("from-file/child", b"new child", false);
    let (flat, flat_blob) = file("from-dir", b"flat", false);
    let plan = resolved(
        &repository,
        base,
        vec![staged, change("removed", OverlayContent::Remove)],
        vec![
            working,
            change("from-file", OverlayContent::Remove),
            child,
            change("from-dir/leaf", OverlayContent::Remove),
            flat,
            change(
                "link",
                OverlayContent::Symlink {
                    target: "nested/tool".into(),
                },
            ),
            change(
                "dangling",
                OverlayContent::Symlink {
                    target: "missing".into(),
                },
            ),
        ],
        vec![staged_blob, working_blob, child_blob, flat_blob],
    );
    let destination = private();
    fs::write(destination.path().join("existing"), b"preserve").unwrap();
    let checkout = prepare_private_checkout(
        destination.path(),
        &plan,
        &mut Source(repository.odb().unwrap()),
        || false,
    )
    .unwrap();
    let path = checkout.path().to_owned();
    assert_eq!(checkout.base_commit(), base);
    assert_eq!(checkout.manifest_sha256(), plan.bundle().manifest_sha256());
    let actual = Repository::open(&path).unwrap();
    assert!(actual.head_detached().unwrap() && actual.is_shallow());
    assert_eq!(actual.head().unwrap().target(), Some(base));
    let index = actual.index().unwrap();
    assert_eq!(
        actual
            .find_blob(index.get_path(Path::new("asset"), 0).unwrap().id)
            .unwrap()
            .content(),
        pointer
    );
    assert_eq!(index.get_path(Path::new("nested/tool"), 0).unwrap().mode, 0o100_755);
    assert!(index.get_path(Path::new("from-file"), 0).is_some());
    assert_eq!(fs::read(path.join("kept")).unwrap(), raw);
    assert_eq!(fs::read(path.join("asset")).unwrap(), b"hydrated\0asset");
    assert_eq!(fs::read(path.join("nested/tool")).unwrap(), b"staged\0tool");
    assert_eq!(fs::read(path.join("from-file/child")).unwrap(), b"new child");
    assert_eq!(fs::read(path.join("from-dir")).unwrap(), b"flat");
    assert!(!path.join("removed").exists());
    assert_eq!(fs::read_link(path.join("link")).unwrap(), Path::new("nested/tool"));
    assert_eq!(fs::read_link(path.join("dangling")).unwrap(), Path::new("missing"));
    assert_eq!(fs::metadata(path.join("nested/tool")).unwrap().mode() & 0o7777, 0o755);
    assert_eq!(fs::metadata(path.join("asset")).unwrap().mode() & 0o7777, 0o644);
    assert_eq!(fs::read(destination.path().join("existing")).unwrap(), b"preserve");
    assert_eq!(fs::read(source.path().join("sentinel")).unwrap(), raw);
    assert_eq!(fs::read(repository.path().join("config")).unwrap(), source_index);
    assert!(!format!("{checkout:?}").contains(path.to_str().unwrap()));
    drop(checkout);
    assert!(path.join("asset").exists());
}

#[test]
fn identical_base_and_index_cannot_mix_different_working_overlays() {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let base = fixture(&[], &repository);
    let parent = private();
    let mut identities = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let (change, blob) = file("working", bytes, false);
        let plan = resolved(&repository, base, vec![], vec![change], vec![blob]);
        let checkout =
            prepare_private_checkout(parent.path(), &plan, &mut Source(repository.odb().unwrap()), || false).unwrap();
        assert_eq!(fs::read(checkout.path().join("working")).unwrap(), bytes);
        assert!(Repository::open(checkout.path()).unwrap().index().unwrap().is_empty());
        identities.push(checkout.manifest_sha256().clone());
    }
    assert_ne!(identities[0], identities[1]);
}

fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::default());
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}

#[test]
fn loose_decoder_rejects_wrong_headers_corruption_truncation_and_trailing_bytes() {
    let good = compressed(b"blob 3\0abc");
    let mut trailing = good.clone();
    trailing.push(0);
    let mut damaged = good.clone();
    *damaged.last_mut().unwrap() ^= 1;
    for encoded in [
        compressed(b"tree 3\0abc"),
        compressed(b"blob 2\0ab"),
        compressed(b"blob 4\0abcd"),
        good[..good.len() - 1].to_vec(),
        trailing,
        damaged,
        compressed(b"blob 3\0ab"),
        compressed(b"blob 3\0abcd"),
    ] {
        let directory = private();
        let path = directory.path().join("object");
        fs::write(&path, encoded).unwrap();
        assert_eq!(
            loose::copy(&File::open(path).unwrap(), 3, &mut Vec::new(), &|| false),
            Err(PrivateCheckoutError::Object)
        );
    }
    let directory = private();
    let path = directory.path().join("object");
    fs::write(&path, good).unwrap();
    let mut output = Vec::new();
    loose::copy(&File::open(path).unwrap(), 3, &mut output, &|| false).unwrap();
    assert_eq!(output, b"abc");
}

#[test]
fn exclusive_writes_reject_collisions_escape_links_and_replaced_names() {
    let directory = private();
    fs::create_dir(directory.path().join(".git")).unwrap();
    let root = files::Root::open(directory.path()).unwrap();
    let outside = private();
    fs::write(outside.path().join("sentinel"), b"safe").unwrap();
    symlink(outside.path(), directory.path().join("escape")).unwrap();
    assert!(root.create("escape/sentinel").is_err());
    assert!(root.create("../outside").is_err());
    fs::hard_link(outside.path().join("sentinel"), directory.path().join("hard")).unwrap();
    assert!(root.create("hard").is_err());
    assert!(root.read_regular("hard").is_err());
    assert!(root.read_regular("escape").is_err());
    fs::create_dir(directory.path().join("collision")).unwrap();
    assert!(root.create("collision").is_err());
    let mut file = root.create("new").unwrap();
    file.write_all(b"original").unwrap();
    assert!(root.create("new").is_err());
    fs::rename(directory.path().join("new"), directory.path().join("moved")).unwrap();
    fs::write(directory.path().join("new"), b"replacement").unwrap();
    let id = Oid::hash_object(ObjectType::Blob, b"original").unwrap();
    assert_eq!(
        root.verify_file("new", &file, 8, false, id),
        Err(PrivateCheckoutError::UnsafeNode)
    );
    assert_eq!(fs::read(directory.path().join("new")).unwrap(), b"replacement");
    assert_eq!(fs::read(outside.path().join("sentinel")).unwrap(), b"safe");
    assert!(root.symlink("hard", "missing").is_err());
    assert!(files::Root::open(directory.path()).is_err());
}

#[test]
fn cancellation_during_working_writes_retains_private_residue_without_links() {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let base = fixture(&[], &repository);
    let (change, blob) = file("large", &vec![9; 48 * 1024], false);
    let plan = resolved(
        &repository,
        base,
        vec![],
        vec![
            change,
            self::change("link", OverlayContent::Symlink { target: "large".into() }),
        ],
        vec![blob],
    );
    let parent = private();
    let failure = prepare_private_checkout(parent.path(), &plan, &mut Source(repository.odb().unwrap()), || {
        fs::read_dir(parent.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .join("large")
                .metadata()
                .is_ok_and(|m| m.len() >= 16 * 1024)
        })
    })
    .unwrap_err();
    assert_eq!(failure.reason, PrivateCheckoutError::Cancelled);
    let residue = failure.residue().unwrap().to_owned();
    assert_eq!(fs::metadata(residue.join("large")).unwrap().len(), 16 * 1024);
    assert!(!residue.join("link").exists());
    assert!(!format!("{failure:?} {failure}").contains(residue.to_str().unwrap()));
    drop(failure);
    assert!(residue.join(".git/HEAD").exists());
    let early =
        prepare_private_checkout(parent.path(), &plan, &mut Source(repository.odb().unwrap()), || true).unwrap_err();
    assert_eq!(early.reason, PrivateCheckoutError::Seed(SeedError::Cancelled));
    assert!(early.residue().is_none());
}

#[test]
fn corrupt_base_identity_and_partial_write_never_succeed() {
    struct FailingWriter(bool);
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            if self.0 {
                Err(io::Error::other("private sentinel"))
            } else {
                self.0 = true;
                Ok(1)
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let blob = repository.blob(b"abc").unwrap();
    let base = fixture(&[("file", blob, 0o100_644)], &repository);
    let plan = resolved(&repository, base, vec![], vec![], vec![]);
    let parent = private();
    let seed = prepare_git_seed(parent.path(), &plan, &mut Source(repository.odb().unwrap()), || false).unwrap();
    let id = blob.to_string();
    let object_path = seed.path().join(format!(".git/objects/{}/{}", &id[..2], &id[2..]));
    fs::set_permissions(&object_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(object_path, compressed(b"blob 3\0xyz")).unwrap();
    assert!(matches!(
        linux::write(seed.path(), &plan, &|| false),
        Err(PrivateCheckoutError::Object)
    ));
    assert_eq!(fs::read(seed.path().join("file")).unwrap(), b"xyz");
    let input = parent.path().join("input");
    fs::write(&input, compressed(b"blob 3\0abc")).unwrap();
    assert_eq!(
        loose::copy(&File::open(input).unwrap(), 3, &mut FailingWriter(false), &|| false),
        Err(PrivateCheckoutError::Storage)
    );
}

#[test]
fn large_incompressible_base_streams_to_repeated_working_files() {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let bytes = 64 * 1024 * 1024 + 1;
    let database = repository.odb().unwrap();
    let mut writer = database.writer(bytes, ObjectType::Blob).unwrap();
    let mut state = 0x1234_5678_u32;
    let mut chunk = [0; 16 * 1024];
    let mut remaining = bytes;
    while remaining > 0 {
        let count = remaining.min(chunk.len());
        for byte in &mut chunk[..count] {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state.to_le_bytes()[0];
        }
        writer.write_all(&chunk[..count]).unwrap();
        remaining -= count;
    }
    let blob = writer.finalize().unwrap();
    drop(writer);
    let base = fixture(&[("one", blob, 0o100_644), ("two", blob, 0o100_755)], &repository);
    let plan = resolved(&repository, base, vec![], vec![], vec![]);
    let parent = private();
    let checks = Cell::new(0);
    let checkout = prepare_private_checkout(parent.path(), &plan, &mut Source(database), || {
        checks.set(checks.get() + 1);
        false
    })
    .unwrap();
    for name in ["one", "two"] {
        let path = checkout.path().join(name);
        assert_eq!(fs::metadata(&path).unwrap().len(), bytes as u64);
        assert_eq!(Oid::hash_file(ObjectType::Blob, &path).unwrap(), blob);
    }
    assert!(checks.get() > 8_000);
}

#[test]
fn deep_directories_empty_payloads_and_special_nodes_remain_bounded() {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let base = fixture(&[], &repository);
    let path = format!("{}leaf", "nested/".repeat(120));
    let (entry, blob) = file(&path, b"", true);
    let plan = resolved(&repository, base, vec![], vec![entry], vec![blob]);
    let parent = private();
    let checkout =
        prepare_private_checkout(parent.path(), &plan, &mut Source(repository.odb().unwrap()), || false).unwrap();
    let metadata = fs::metadata(checkout.path().join(path)).unwrap();
    assert_eq!(
        (metadata.len(), metadata.mode() & 0o7777, metadata.nlink()),
        (0, 0o755, 1)
    );
    let directory = private();
    fs::create_dir(directory.path().join(".git")).unwrap();
    let root = files::Root::open(directory.path()).unwrap();
    rustix::fs::mkfifoat(rustix::fs::CWD, directory.path().join("fifo"), rustix::fs::Mode::RUSR).unwrap();
    assert!(root.create("fifo").is_err());
    assert!(root.read_regular("fifo").is_err());
}

#[test]
fn shared_directory_prefixes_are_collected_once_with_bounded_cancellation_gaps() {
    let prefix = "a/".repeat(1_000);
    let paths: Vec<_> = (0..1_000).map(|index| format!("{prefix}leaf{index}")).collect();
    let checks = Cell::new(0);
    let directories = linux::directories(paths.iter().map(String::as_str), &|| {
        checks.set(checks.get() + 1);
        false
    })
    .unwrap();
    assert_eq!(directories.len(), 1_000);
    assert_eq!(checks.get(), 2_999);
    assert_eq!(
        linux::directories(paths.iter().map(String::as_str), &|| true),
        Err(PrivateCheckoutError::Cancelled)
    );
}
