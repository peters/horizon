//! Attribute checks use only the committed index, without workstation overrides.
use super::{Error, Result};
use git2::{AttrCheckFlags, Repository, Tree};
use std::path::Path;

pub(super) struct Attributes {
    repository: Repository,
    _root: tempfile::TempDir,
}
impl Attributes {
    pub fn new(source: &Repository, tree: &Tree<'_>) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let repository = Repository::init_bare(root.path()).map_err(invalid)?;
        let objects = source.commondir().join("objects").canonicalize()?;
        std::fs::write(
            root.path().join("objects/info/alternates"),
            format!("{}\n", objects.display()),
        )?;
        let empty = root.path().join("empty-attributes");
        std::fs::write(&empty, "")?;
        repository
            .config()
            .map_err(invalid)?
            .set_str("core.attributesFile", &empty.to_string_lossy())
            .map_err(invalid)?;
        let mut index = repository.index().map_err(invalid)?;
        index.read_tree(tree).map_err(invalid)?;
        index.write().map_err(invalid)?;
        Ok(Self {
            repository,
            _root: root,
        })
    }
    pub fn is_lfs(&self, path: &Path) -> Result<bool> {
        self.repository
            .get_attr(path, "filter", AttrCheckFlags::INDEX_ONLY | AttrCheckFlags::NO_SYSTEM)
            .map(|value| value == Some("lfs"))
            .map_err(invalid)
    }
}
fn invalid(_: git2::Error) -> Error {
    Error::Invalid("Cannot read selected commit attributes")
}
