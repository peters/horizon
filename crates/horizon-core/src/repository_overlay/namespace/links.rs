use super::{MAX_METADATA_BYTES, NamespaceEntry, NamespaceError as Error, RepositoryNamespace};
use crate::repository_overlay::paths::MAX_PATH_BYTES;
use std::collections::VecDeque;

const MAX_LINK_HOPS: usize = 40;
const MAX_PENDING_COMPONENTS: usize = 8192;
const MAX_LINK_WORK: usize = 8 * MAX_METADATA_BYTES;

#[derive(Default)]
pub(super) struct Work(usize);

impl Work {
    fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.0 = self
            .0
            .checked_add(bytes.max(1))
            .filter(|total| *total <= MAX_LINK_WORK)
            .ok_or(Error::Limit)?;
        Ok(())
    }
}

pub(super) fn validate(namespace: &RepositoryNamespace, work: &mut Work) -> Result<(), Error> {
    for (path, entry) in &namespace.entries {
        if matches!(entry, NamespaceEntry::Symlink { .. }) {
            resolve(namespace, path, work)?;
        }
    }
    Ok(())
}

fn resolve(namespace: &RepositoryNamespace, path: &str, work: &mut Work) -> Result<(), Error> {
    let mut pending: VecDeque<_> = path.split('/').collect();
    let mut resolved = String::with_capacity(MAX_PATH_BYTES);
    let mut boundaries = Vec::new();
    let mut hops = 0;
    while let Some(component) = pending.pop_front() {
        work.charge(component.len() + resolved.len())?;
        match component {
            "" | "." => continue,
            ".." => {
                resolved.truncate(boundaries.pop().ok_or(Error::Link)?);
                continue;
            }
            _ => {}
        }
        let previous = resolved.len();
        if previous + usize::from(previous != 0) + component.len() > MAX_PATH_BYTES {
            return Err(Error::Limit);
        }
        if previous != 0 {
            resolved.push('/');
        }
        resolved.push_str(component);
        boundaries.push(previous);
        match namespace.entries.get(&resolved) {
            Some(NamespaceEntry::Symlink { target }) => {
                hops += 1;
                if hops > MAX_LINK_HOPS {
                    return Err(Error::Link);
                }
                // Relative targets begin at the actual containing directory, after
                // earlier links have resolved. Normalize `..` only while traversing.
                resolved.truncate(boundaries.pop().ok_or(Error::Link)?);
                for part in target.rsplit('/') {
                    if pending.len() == MAX_PENDING_COMPONENTS {
                        return Err(Error::Limit);
                    }
                    pending.push_front(part);
                    work.charge(part.len())?;
                }
            }
            Some(NamespaceEntry::File { .. }) if !pending.is_empty() => return Err(Error::Link),
            _ => {}
        }
    }
    Ok(())
}
