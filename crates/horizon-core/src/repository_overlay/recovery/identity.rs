use super::{RecoveryComparisonError as Error, check_cancel};
use crate::repository_overlay::{
    bundle::{MAX_BUNDLE_BYTES, RepositoryOverlayBundle},
    namespace::{NamespaceEntry, NamespaceFile},
};
use git2::{ObjectType, Oid};
use std::collections::BTreeMap;

pub(super) struct Identities<'a, 'c, C> {
    objects: BTreeMap<&'a str, Oid>,
    bytes: usize,
    cancelled: &'c C,
    #[cfg(test)]
    pub hashes: usize,
}

impl<'a, 'c, C: Fn() -> bool> Identities<'a, 'c, C> {
    pub fn new(cancelled: &'c C) -> Self {
        Self {
            objects: BTreeMap::new(),
            bytes: 0,
            cancelled,
            #[cfg(test)]
            hashes: 0,
        }
    }

    pub fn equal(
        &mut self,
        left: Option<&'a NamespaceEntry>,
        left_bundle: &'a RepositoryOverlayBundle,
        right: Option<&'a NamespaceEntry>,
        right_bundle: &'a RepositoryOverlayBundle,
    ) -> Result<bool, Error> {
        match (left, right) {
            (None, None) => Ok(true),
            (Some(NamespaceEntry::Symlink { target: a }), Some(NamespaceEntry::Symlink { target: b })) => Ok(a == b),
            (
                Some(NamespaceEntry::File {
                    source: a,
                    executable: am,
                }),
                Some(NamespaceEntry::File {
                    source: b,
                    executable: bm,
                }),
            ) if am == bm && a.bytes() == b.bytes() => match (a, b) {
                (NamespaceFile::Base { object: a, .. }, NamespaceFile::Base { object: b, .. }) => Ok(a == b),
                (NamespaceFile::Overlay { sha256: a, .. }, NamespaceFile::Overlay { sha256: b, .. }) => Ok(a == b),
                _ => Ok(self.object(a, left_bundle)? == self.object(b, right_bundle)?),
            },
            _ => Ok(false),
        }
    }

    fn object(&mut self, file: &'a NamespaceFile, bundle: &'a RepositoryOverlayBundle) -> Result<Oid, Error> {
        let (sha256, bytes) = match file {
            NamespaceFile::Base { object, .. } => return Ok(*object),
            NamespaceFile::Overlay { sha256, bytes } => (sha256, bytes),
        };
        if let Some(object) = self.objects.get(sha256.as_str()) {
            return Ok(*object);
        }
        let payload = bundle
            .blob(sha256)
            .filter(|payload| payload.len() as u64 == *bytes)
            .ok_or(Error::Object)?;
        self.bytes = self
            .bytes
            .checked_add(payload.len())
            .filter(|n| *n <= 2 * MAX_BUNDLE_BYTES)
            .ok_or(Error::Limit)?;
        check_cancel(self.cancelled)?;
        let object = Oid::hash_object(ObjectType::Blob, payload).map_err(|_| Error::Object)?;
        #[cfg(test)]
        {
            self.hashes += 1;
        }
        check_cancel(self.cancelled)?;
        self.objects.insert(sha256.as_str(), object);
        Ok(object)
    }
}
