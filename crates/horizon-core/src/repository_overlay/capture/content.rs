use super::GitCaptureError as Error;
use crate::repository_overlay::{
    OverlayChange, OverlayContent, OverlayPlanError, RepositoryOverlayPlan,
    bundle::{MAX_BUNDLE_BYTES, OverlayBundleError, RepositoryOverlayBundle, VerifiedOverlayBlob},
    paths::MAX_PATH_BYTES,
    reader::{MAX_READ_BYTES, SelectedRepositoryNode as Node},
};
use git2::{ObjectType, Oid, Repository};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct GitNode {
    pub mode: u32,
    pub oid: Oid,
}

impl GitNode {
    pub fn new(mode: u32, oid: Oid) -> Result<Self, Error> {
        if !matches!(mode, 0o100_644 | 0o100_755 | 0o120_000) {
            return Err(Error::UnsupportedNode);
        }
        Ok(Self { mode, oid })
    }

    pub fn read(self, repository: &Repository) -> Result<Node, Error> {
        let database = repository.odb().map_err(|_| Error::GitRead)?;
        let (length, kind) = database.read_header(self.oid).map_err(|_| Error::GitRead)?;
        if kind != ObjectType::Blob {
            return Err(Error::InvalidObject);
        }
        if length > MAX_READ_BYTES {
            return Err(OverlayBundleError::FileLimit.into());
        }
        if self.mode == 0o120_000 && length > MAX_PATH_BYTES {
            return Err(OverlayPlanError::InvalidLink.into());
        }
        let blob = repository.find_blob(self.oid).map_err(|_| Error::GitRead)?;
        if blob.size() != length || hash(blob.content())? != self.oid {
            return Err(Error::InvalidObject);
        }
        if self.mode == 0o120_000 {
            let target = std::str::from_utf8(blob.content()).map_err(|_| Error::InvalidObject)?;
            Ok(Node::Symlink {
                target: target.to_owned(),
            })
        } else {
            Ok(Node::File {
                bytes: blob.content().to_vec(),
                executable: self.mode == 0o100_755,
            })
        }
    }

    pub fn matches(self, node: &Node) -> Result<bool, Error> {
        let (mode, bytes) = match node {
            Node::File { bytes, executable } => (if *executable { 0o100_755 } else { 0o100_644 }, bytes.as_slice()),
            Node::Symlink { target } => (0o120_000, target.as_bytes()),
        };
        Ok(self.mode == mode && self.oid == hash(bytes)?)
    }
}

fn hash(bytes: &[u8]) -> Result<Oid, Error> {
    Oid::hash_object(ObjectType::Blob, bytes).map_err(|_| Error::InvalidObject)
}

#[derive(Default)]
pub(super) struct Contents {
    blobs: BTreeMap<String, VerifiedOverlayBlob>,
    bytes: usize,
}

impl Contents {
    pub fn change(&mut self, path: &str, node: Option<Node>) -> Result<OverlayChange, Error> {
        let content = match node {
            None => OverlayContent::Remove,
            Some(Node::Symlink { target }) => OverlayContent::Symlink { target },
            Some(Node::File { bytes, executable }) => {
                let blob = VerifiedOverlayBlob::new(bytes)?;
                let content = OverlayContent::File {
                    sha256: blob.sha256().clone(),
                    bytes: blob.bytes().len() as u64,
                    executable,
                };
                let key = blob.sha256().as_str();
                if let Some(existing) = self.blobs.get(key) {
                    if existing.bytes() != blob.bytes() {
                        return Err(Error::InvalidObject);
                    }
                } else {
                    self.bytes = self
                        .bytes
                        .checked_add(blob.bytes().len())
                        .filter(|total| *total <= MAX_BUNDLE_BYTES)
                        .ok_or(OverlayBundleError::BundleLimit)?;
                    self.blobs.insert(key.to_owned(), blob);
                }
                content
            }
        };
        Ok(OverlayChange::new(path.to_owned(), content)?)
    }

    pub fn finish(self, plan: RepositoryOverlayPlan) -> Result<RepositoryOverlayBundle, Error> {
        Ok(RepositoryOverlayBundle::new(plan, self.blobs.into_values())?)
    }
}
