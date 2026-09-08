use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        RepositoryOverlayPlan,
        bundle::RepositoryOverlayBundle,
        namespace::resolve_namespaces_from_source,
        seed::{GitObjectMetadata, GitObjectSource, GitObjectStream},
    },
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{self, Read},
    rc::Rc,
};

#[derive(Clone)]
struct Record {
    kind: ObjectType,
    length: u64,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Source {
    records: BTreeMap<Oid, Record>,
    opened: Vec<Oid>,
    inspected: Vec<Oid>,
    reads: Rc<Cell<usize>>,
    fail: bool,
    cancel: Option<Rc<Cell<bool>>>,
}

impl Source {
    fn add(&mut self, kind: ObjectType, bytes: &[u8]) -> Oid {
        let id = Oid::hash_object(kind, bytes).unwrap();
        self.records.insert(
            id,
            Record {
                kind,
                length: bytes.len() as u64,
                bytes: bytes.to_vec(),
            },
        );
        id
    }
    fn commit(&mut self, tree: Oid) -> Oid {
        self.add(ObjectType::Commit, &commit_bytes(tree))
    }
    fn resolve(&mut self, commit: Oid) -> Result<super::super::ResolvedRepositoryOverlay, Error> {
        let plan = RepositoryOverlayPlan::new(
            GitSource {
                repository: "synthetic/project".into(),
                commit: GitCommitSha::parse(commit.to_string()).unwrap(),
                branch: None,
            },
            vec![],
            vec![],
        )
        .unwrap();
        resolve_namespaces_from_source(self, RepositoryOverlayBundle::new(plan, vec![]).unwrap(), || false)
    }
}

impl GitObjectSource for Source {
    fn open(&mut self, id: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        self.opened.push(id);
        let record = self.records.get(&id).ok_or(SeedError::Source)?;
        Ok(GitObjectStream {
            kind: record.kind,
            bytes: record.length,
            reader: Box::new(Reader {
                bytes: io::Cursor::new(record.bytes.clone()),
                reads: self.reads.clone(),
                fail: self.fail,
                cancel: self.cancel.clone(),
            }),
        })
    }
}

impl GitObjectInspector for Source {
    fn inspect(&mut self, id: Oid) -> Result<GitObjectMetadata, SeedError> {
        self.inspected.push(id);
        let record = self.records.get(&id).ok_or(SeedError::Source)?;
        Ok(GitObjectMetadata {
            kind: record.kind,
            bytes: record.length,
        })
    }
}

struct Reader {
    bytes: io::Cursor<Vec<u8>>,
    reads: Rc<Cell<usize>>,
    fail: bool,
    cancel: Option<Rc<Cell<bool>>>,
}

impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.reads.set(self.reads.get() + 1);
        if let Some(cancel) = &self.cancel {
            cancel.set(true);
        }
        if self.fail {
            return Err(io::Error::other("private-fixture-marker"));
        }
        let limit = bytes.len().min(37);
        self.bytes.read(&mut bytes[..limit])
    }
}

fn commit_bytes(tree: Oid) -> Vec<u8> {
    format!("tree {tree}\nauthor Fixture <fixture@example.invalid> 1 +0000\ncommitter Fixture <fixture@example.invalid> 1 +0000\n\nSynthetic\n").into_bytes()
}

fn tree_bytes(entries: &[(&str, &[u8], Oid)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (mode, name, id) in entries {
        bytes.extend_from_slice(mode.as_bytes());
        bytes.push(b' ');
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.extend_from_slice(id.as_bytes());
    }
    bytes
}

#[test]
fn namespace_inspects_large_files_but_only_reads_commit_tree_and_literal_links() {
    let mut source = Source::default();
    let blob = source.add(ObjectType::Blob, b"unread bytes");
    let length = 65 * 1024 * 1024 + 1;
    source.records.get_mut(&blob).unwrap().length = length;
    let link = source.add(ObjectType::Blob, b"large");
    let tree = source.add(
        ObjectType::Tree,
        &tree_bytes(&[("100755", b"large", blob), ("120000", b"alias", link)]),
    );
    let commit = source.commit(tree);
    let result = source.resolve(commit).unwrap();
    assert_eq!(result.base().logical_bytes(), length);
    assert!(matches!(
        result.base().entry("large"),
        Some(NamespaceEntry::File { executable: true, .. })
    ));
    assert_eq!(
        result.base().entry("alias"),
        Some(&NamespaceEntry::Symlink { target: "large".into() })
    );
    assert_eq!(source.opened, [commit, tree, link]);
    assert_eq!(source.inspected, [blob]);
}

#[test]
fn exact_metadata_reader_rejects_wrong_kind_size_hash_truncation_and_extra_bytes() {
    for case in 0..5 {
        let mut source = Source::default();
        let id = source.add(ObjectType::Tree, b"literal");
        let record = source.records.get_mut(&id).unwrap();
        match case {
            0 => record.kind = ObjectType::Blob,
            1 => record.length += 1,
            2 => record.bytes[0] ^= 1,
            3 => {
                record.bytes.pop();
            }
            _ => record.bytes.push(0),
        }
        let mut objects = objects::Objects::new(&mut source, &|| false);
        assert_eq!(objects.read(id, ObjectType::Tree, 100), Err(Error::Object));
        if case > 0 {
            assert!(source.reads.get() > 0);
        }
    }
}

#[test]
fn metadata_bounds_reject_headers_before_reads_and_charge_actual_aggregate_work() {
    let mut source = Source::default();
    let blob = source.add(ObjectType::Blob, b"unread");
    source.records.get_mut(&blob).unwrap().length = u64::MAX;
    assert_eq!(
        objects::Objects::new(&mut source, &|| false).read(blob, ObjectType::Blob, 4096),
        Err(Error::Limit)
    );
    assert_eq!(source.reads.get(), 0);

    let tree = source.add(ObjectType::Tree, &tree_bytes(&[("100644", b"file", blob)]));
    let mut bytes = commit_bytes(tree);
    bytes.resize(MAX_METADATA_BYTES, b'x');
    let commit = source.add(ObjectType::Commit, &bytes);
    assert!(matches!(source.resolve(commit), Err(Error::Limit)));
    assert_eq!(source.opened, [blob, commit, tree]);
    assert!(source.inspected.is_empty());
    assert!(source.reads.get() > MAX_METADATA_BYTES / 37);
}

#[test]
fn malformed_tree_records_and_unsupported_modes_never_return_a_namespace() {
    let oid = Oid::hash_object(ObjectType::Blob, b"literal").unwrap();
    for raw in [
        b"100644 name".to_vec(),
        b"100644 name\0short".to_vec(),
        tree_bytes(&[("100644", b"", oid)]),
        tree_bytes(&[("100644", b"bad\xff", oid)]),
        tree_bytes(&[("100644", b"a/b", oid)]),
        tree_bytes(&[("160000", b"module", oid)]),
        tree_bytes(&[("999999", b"mode", oid)]),
    ] {
        let mut source = Source::default();
        let tree = source.add(ObjectType::Tree, &raw);
        let commit = source.commit(tree);
        assert!(source.resolve(commit).is_err());
        assert!(source.inspected.is_empty());
    }
}

#[test]
fn repeated_tree_expansion_charges_real_pending_directories_and_each_leaf() {
    use crate::repository_overlay::MAX_CHANGES;
    for count in [MAX_CHANGES / 2, MAX_CHANGES / 2 + 1] {
        let mut source = Source::default();
        let blob = source.add(ObjectType::Blob, b"x");
        let shared = source.add(ObjectType::Tree, &tree_bytes(&[("100644", b"file", blob)]));
        let names: Vec<_> = (0..count).map(|n| format!("d{n}")).collect();
        let entries: Vec<_> = names.iter().map(|name| ("40000", name.as_bytes(), shared)).collect();
        let root = source.add(ObjectType::Tree, &tree_bytes(&entries));
        let commit = source.commit(root);
        let result = source.resolve(commit);
        if count == MAX_CHANGES / 2 {
            let result = result.unwrap();
            assert_eq!(result.base().entries().len(), count);
            assert_eq!(result.base().logical_bytes(), count as u64);
            assert_eq!(source.opened.len(), count + 2);
            assert_eq!(source.inspected.len(), count);
        } else {
            assert!(matches!(result, Err(Error::Limit)));
            assert_eq!(source.inspected.len(), MAX_CHANGES - count + 1);
        }
        assert!(!source.opened.contains(&blob));
    }
}

#[test]
fn bounded_commit_structure_keeps_signed_headers_non_utf8_message_and_parent_ids() {
    let tree = Oid::hash_object(ObjectType::Tree, b"").unwrap();
    let mut valid = commit_bytes(tree);
    let extras = format!(
        "tree {tree}\nparent {tree}\nauthor Fixture <fixture@example.invalid> -1 +0130\nauthor Second <second@example.invalid> 1 -0200\ncommitter Fixture <fixture@example.invalid> 1 +0000\ngpgsig-sha256 synthetic\n continuation\nencoding synthetic\n\n"
    );
    assert_eq!(
        records::commit_tree(&[extras.as_bytes(), b"message\xff\0"].concat()),
        Ok(tree)
    );
    assert_eq!(records::commit_tree(&valid), Ok(tree));
    for raw in [
        b"tree short\n\n".to_vec(),
        format!("tree {tree}\n\n").into_bytes(),
        extras.replace("parent ", "parent invalid").into_bytes(),
        extras.replace("committer ", "missing ").into_bytes(),
        extras.replace("1 +0000", "invalid +0000").into_bytes(),
        extras.replace("+0130", "0130").into_bytes(),
        extras
            .replace("gpgsig-sha256 synthetic", &format!("tree {tree}"))
            .into_bytes(),
        extras.replace("gpgsig-sha256 synthetic\n", "").into_bytes(),
        extras
            .replace("encoding synthetic", "encoding\0 synthetic")
            .into_bytes(),
    ] {
        assert_eq!(records::commit_tree(&raw), Err(Error::Object));
    }
    let boundary = valid.windows(2).position(|p| p == b"\n\n").unwrap();
    valid.truncate(boundary + 1);
    assert_eq!(records::commit_tree(&valid), Err(Error::Object));
}

#[test]
fn actual_payload_and_framing_failures_preserve_caller_cancel_and_redact_source_errors() {
    for length in [0, 1] {
        for cancellation in [false, true] {
            let cancelled = Rc::new(Cell::new(false));
            let mut source = Source {
                fail: true,
                cancel: cancellation.then(|| cancelled.clone()),
                ..Source::default()
            };
            let id = source.add(ObjectType::Blob, &vec![0; length]);
            let check = || cancelled.get();
            let error = objects::Objects::new(&mut source, &check)
                .read(id, ObjectType::Blob, 100)
                .unwrap_err();
            assert_eq!(source.reads.get(), 1);
            assert_eq!(
                error,
                Error::Source(if cancellation {
                    SeedError::Cancelled
                } else {
                    SeedError::Source
                })
            );
            assert!(!format!("{error:?} {error}").contains("private-fixture-marker"));
        }
    }
    let mut source = Source::default();
    let id = source.add(ObjectType::Blob, b"unread");
    assert_eq!(
        objects::Objects::new(&mut source, &|| true).read(id, ObjectType::Blob, 100),
        Err(Error::Source(SeedError::Cancelled))
    );
    assert!(source.opened.is_empty());
}
