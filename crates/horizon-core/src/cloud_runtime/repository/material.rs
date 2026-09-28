//! Selected-commit LFS objects and recursively pinned local submodules.
use super::super::{Event, progress::Progress};
use super::{Error, Result, Runner};
use git2::{ObjectType, Repository, TreeWalkMode, TreeWalkResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Default, Serialize)]
pub(super) struct Material {
    pub modules: Vec<Module>,
    pub assets: Vec<Asset>,
    #[serde(skip)]
    budget: Option<CollectionBudget>,
}
struct CollectionBudget {
    entries: usize,
    paths: usize,
    assets: usize,
    bytes: u64,
}
impl CollectionBudget {
    fn entry(&mut self, length: usize) -> Result<()> {
        if length > 4096 {
            return Err(Error::Invalid("Source path exceeds its collection limit"));
        }
        self.entries = self
            .entries
            .checked_sub(1)
            .ok_or(Error::Invalid("Source tree exceeds its entry limit"))?;
        self.paths = self
            .paths
            .checked_sub(length)
            .ok_or(Error::Invalid("Source paths exceed their collection limit"))?;
        Ok(())
    }
    fn asset(&mut self, size: u64) -> Result<()> {
        self.assets = self
            .assets
            .checked_sub(1)
            .ok_or(Error::Invalid("Source material exceeds its asset limit"))?;
        self.bytes = self
            .bytes
            .checked_sub(size)
            .ok_or(Error::Invalid("Source material exceeds its verification byte limit"))?;
        Ok(())
    }
}
struct Entry {
    name: String,
    oid: git2::Oid,
    mode: i32,
}

#[derive(Serialize)]
pub(super) struct Module {
    pub path: String,
    pub revision: String,
    #[serde(skip)]
    pub repository: PathBuf,
}
#[derive(Serialize)]
pub(super) struct Asset {
    pub path: String,
    pub oid: String,
    pub size: u64,
    #[serde(skip)]
    pub source: PathBuf,
}
impl Material {
    pub fn collect(repository: &Path, revision: &str, runner: &Runner<'_>) -> Result<Self> {
        let mut result = Self::default();
        result.visit(repository, revision, Path::new(""), 0, runner)?;
        Ok(result)
    }
    #[cfg(target_os = "linux")]
    pub fn collect_bounded(repository: &Path, revision: &str, runner: &Runner<'_>, limit: u64) -> Result<Self> {
        let mut result = Self {
            budget: Some(CollectionBudget {
                entries: 65536,
                paths: 16 * 1024 * 1024,
                assets: 8192,
                bytes: limit,
            }),
            ..Self::default()
        };
        result.visit(repository, revision, Path::new(""), 0, runner)?;
        Ok(result)
    }
    fn entries(&mut self, tree: &git2::Tree<'_>, prefix: &Path, runner: &Runner<'_>) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let mut failure = None;
        let walk = tree.walk(TreeWalkMode::PreOrder, |root, entry| {
            let mut inspect = || -> Result<()> {
                runner.cancel.check()?;
                let name = entry.name().map_err(|_| Error::Invalid("Source paths must be UTF-8"))?;
                if let Some(budget) = &mut self.budget {
                    budget.entry(prefix.as_os_str().len() + root.len() + name.len() + 1)?;
                }
                if entry.kind() != Some(ObjectType::Tree) {
                    entries.push(Entry {
                        name: format!("{root}{name}"),
                        oid: entry.id(),
                        mode: entry.filemode(),
                    });
                }
                Ok(())
            };
            match inspect() {
                Ok(()) => TreeWalkResult::Ok,
                Err(error) => {
                    failure = Some(error);
                    TreeWalkResult::Abort
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        walk.map_err(|_| Error::Invalid("Cannot inspect committed source tree"))?;
        Ok(entries)
    }
    fn visit(
        &mut self,
        directory: &Path,
        revision: &str,
        prefix: &Path,
        depth: usize,
        runner: &Runner<'_>,
    ) -> Result<()> {
        runner.cancel.check()?;
        if depth > 16 || self.modules.len() > 256 {
            return Err(Error::Invalid(
                "Source exceeds the supported submodule nesting or count",
            ));
        }
        let format = runner.run(
            "Inspect repository object format",
            Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(["rev-parse", "--show-object-format"]),
            Duration::from_secs(30),
        )?;
        if format.trim() != "sha1" {
            return Err(Error::Invalid(
                "Cloud source export currently requires a SHA-1 Git repository",
            ));
        }
        let repo = Repository::open(directory)
            .map_err(|_| Error::Invalid("Initialize selected submodules locally before deploying"))?;
        let tree = repo
            .revparse_single(revision)
            .and_then(|object| object.peel_to_tree())
            .map_err(|_| Error::Invalid("Selected submodule commit is not available locally"))?;
        let entries = self.entries(&tree, prefix, runner)?;
        let mut media = None;
        let attributes = super::attributes::Attributes::new(&repo, &tree)?;
        for Entry { name, oid, mode } in entries {
            runner.cancel.check()?;
            safe_path(Path::new(&name))?;
            let path = prefix.join(&name);
            if mode == 0o160_000 {
                if self.modules.len() >= 256 {
                    return Err(Error::Invalid("Source exceeds the supported submodule count"));
                }
                let local = directory.join(&name);
                self.modules.push(Module {
                    path: portable_path(&path)?,
                    revision: oid.to_string(),
                    repository: local.clone(),
                });
                self.visit(&local, &oid.to_string(), &path, depth + 1, runner)?;
            } else if mode == 0o100_644 || mode == 0o100_755 {
                if !attributes.is_lfs(Path::new(&name))? {
                    continue;
                }
                let bytes = blob_prefix(&repo, directory, oid, runner)?;
                let Some((oid, size)) = pointer(&bytes)? else {
                    continue;
                };
                if let Some(budget) = &mut self.budget {
                    budget.asset(size)?;
                }
                if media.is_none() {
                    media = Some(media_directory(directory, runner)?);
                }
                let storage = media.as_ref().ok_or(Error::Invalid("Missing LFS storage"))?;
                let source = storage.join(&oid[..2]).join(&oid[2..4]).join(&oid);
                verify_object(&source, &oid, size, runner)?;
                self.assets.push(Asset {
                    path: portable_path(&path)?,
                    oid,
                    size,
                    source,
                });
            }
        }
        Ok(())
    }
    pub fn hydrate(&self, destination: &Path, runner: &Runner<'_>) -> Result<()> {
        for asset in &self.assets {
            runner.cancel.check()?;
            let path = destination.join(&asset.path);
            let permissions = std::fs::metadata(&path)?.permissions();
            std::fs::copy(&asset.source, &path)?;
            verify_object(&path, &asset.oid, asset.size, runner)?;
            std::fs::set_permissions(path, permissions)?;
        }
        Ok(())
    }
}
fn blob_prefix(repo: &Repository, directory: &Path, oid: git2::Oid, runner: &Runner<'_>) -> Result<Vec<u8>> {
    let size = repo
        .odb()
        .and_then(|odb| odb.read_header(oid))
        .map_err(|_| Error::Invalid("Missing committed source object"))?
        .0;
    if size <= 1024 {
        return repo
            .find_blob(oid)
            .map(|blob| blob.content().to_vec())
            .map_err(|_| Error::Invalid("Missing committed source object"));
    }
    // Packed Git objects do not support libgit2 streaming; isolate their decoding
    // in a cancellable process and retain only enough bytes to classify a pointer.
    let bytes = runner.prefix(
        Command::new("git")
            .arg("--no-replace-objects")
            .arg("-C")
            .arg(directory)
            .args(["cat-file", "blob", &oid.to_string()]),
        1025,
        Duration::from_secs(30),
    )?;
    if bytes.len() != 1025 {
        return Err(Error::Invalid("Cannot read committed source object"));
    }
    Ok(bytes)
}
fn portable_path(path: &Path) -> Result<String> {
    safe_path(path)?;
    path.components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .ok_or(Error::Invalid("Source paths must be UTF-8"))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

fn safe_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(Error::Invalid("Unsupported committed source path"));
    }
    Ok(())
}
fn pointer(bytes: &[u8]) -> Result<Option<(String, u64)>> {
    if !bytes.starts_with(b"version https://git-lfs.github.com/spec/v1\n") {
        return Ok(None);
    }
    if bytes.len() > 1024 {
        return Err(Error::Invalid("Extended or malformed Git LFS pointers are unsupported"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Invalid("Invalid Git LFS pointer"))?;
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 3 {
        return Err(Error::Invalid("Extended or malformed Git LFS pointers are unsupported"));
    }
    let oid = lines[1]
        .strip_prefix("oid sha256:")
        .filter(|oid| oid.len() == 64 && oid.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(Error::Invalid("Invalid Git LFS object identity"))?;
    let size = lines[2]
        .strip_prefix("size ")
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or(Error::Invalid("Invalid Git LFS object size"))?;
    Ok(Some((oid.to_owned(), size)))
}
fn media_directory(repository: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    // Use LFS's own resolution, including worktrees and custom lfs.storage; do not log remote URLs.
    let quiet = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    let output = quiet.run(
        "Locate local LFS objects",
        Command::new("git").arg("-C").arg(repository).args(["lfs", "env"]),
        Duration::from_secs(30),
    )?;
    output
        .lines()
        .find_map(|line| line.strip_prefix("LocalMediaDir="))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(Error::Invalid("Cannot locate local Git LFS objects"))
}
fn verify_object(path: &Path, oid: &str, size: u64, runner: &Runner<'_>) -> Result<()> {
    let mut object = Verified::open(path, oid, size, runner)?;
    let copied = std::io::copy(&mut object, &mut std::io::sink()).map(|_| ());
    object.finish(copied)
}

/// A local LFS object read once, with its size and SHA-256 identity checked as it
/// streams, so its bytes can go straight into an archive without a staged copy.
pub(super) struct Verified<'a, 'r> {
    file: std::fs::File,
    oid: &'a str,
    size: u64,
    runner: &'a Runner<'r>,
    hash: Sha256,
    completed: u64,
    reported: Instant,
    failure: Option<Error>,
}
impl<'a, 'r> Verified<'a, 'r> {
    pub(super) fn open(path: &Path, oid: &'a str, size: u64, runner: &'a Runner<'r>) -> Result<Self> {
        runner.cancel.check()?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed());
        }
        let file = options
            .open(path)
            .map_err(|_| Error::Invalid("Fetch selected Git LFS objects locally before deploying"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != size {
            return Err(Error::Invalid("Local Git LFS object size mismatch"));
        }
        let object = Self {
            file,
            oid,
            size,
            runner,
            hash: Sha256::new(),
            completed: 0,
            reported: Instant::now(),
            failure: None,
        };
        object.progress();
        Ok(object)
    }
    fn progress(&self) {
        (self.runner.emit)(Event::Progress(Progress {
            detail: "Verifying current source asset".into(),
            completed: self.completed,
            total: Some(self.size),
            transferred: Some(self.completed),
            ..Progress::default()
        }));
    }
    fn fill(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.runner.cancel.check()?;
        let length = self.file.read(buffer)?;
        self.completed += length as u64;
        // Never hand out bytes past the recorded size: an archive entry's header
        // already promised exactly that many.
        if self.completed > self.size {
            return Err(Error::Invalid("Local Git LFS object grew during verification"));
        }
        self.hash.update(&buffer[..length]);
        if length == 0 || self.reported.elapsed() >= Duration::from_millis(250) {
            self.progress();
            self.reported = Instant::now();
        }
        Ok(length)
    }
    /// Accepts the object only when every byte was read and matched its identity;
    /// `read` is the outcome of whatever consumed it.
    pub(super) fn finish(mut self, read: std::io::Result<()>) -> Result<()> {
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        read?;
        let mut digest = String::with_capacity(64);
        for byte in self.hash.finalize() {
            use std::fmt::Write as _;
            let _ = write!(digest, "{byte:02x}");
        }
        if self.completed != self.size || digest != self.oid {
            return Err(Error::Invalid("Local Git LFS object checksum mismatch"));
        }
        Ok(())
    }
}
impl Read for Verified<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.fill(buffer).map_err(|failure| {
            let error = std::io::Error::other(failure.to_string());
            self.failure = Some(failure);
            error
        })
    }
}

pub(super) fn archive(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    archive_until(
        repository,
        revision,
        root,
        runner,
        Instant::now() + super::archive::TIMEOUT,
    )
}

/// Leaves no partial archive behind when writing it fails.
pub(super) fn archive_until(
    repository: &Path,
    revision: &str,
    root: &Path,
    runner: &Runner<'_>,
    deadline: Instant,
) -> Result<PathBuf> {
    let material = Material::collect(repository, revision, runner)?;
    let packs = root.join("material");
    std::fs::create_dir(&packs)?;
    for (index, module) in material.modules.iter().enumerate() {
        super::pack(
            &module.repository,
            &module.revision,
            &packs.join(format!("module-{index}.pack")),
            runner,
        )?;
    }
    let manifest = serde_json::to_vec(&material).map_err(|_| Error::Json)?;
    let archive = root.join("source-material.tar");
    let mut output = std::io::BufWriter::new(std::fs::File::create_new(&archive)?);
    let written = super::archive::write(&material, &manifest, &packs, &mut output, runner, deadline)
        .and_then(|()| Ok(std::io::Write::flush(&mut output)?));
    drop(output);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&archive);
        return Err(error);
    }
    // The packs now live in the archive; do not keep a second copy through the upload.
    std::fs::remove_dir_all(&packs)?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_paths_use_portable_slashes() {
        let nested = std::path::Path::new("modules").join("child").join("asset.bin");
        assert_eq!(super::portable_path(&nested).unwrap(), "modules/child/asset.bin");
    }
}
