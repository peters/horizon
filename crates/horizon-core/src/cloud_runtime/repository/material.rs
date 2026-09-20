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
        let repo = Repository::open(directory)
            .map_err(|_| Error::Invalid("Initialize selected submodules locally before deploying"))?;
        let tree = repo
            .revparse_single(revision)
            .and_then(|object| object.peel_to_tree())
            .map_err(|_| Error::Invalid("Selected submodule commit is not available locally"))?;
        let mut entries = Vec::new();
        tree.walk(TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() != Some(ObjectType::Tree) {
                entries.push((
                    format!("{root}{}", entry.name().unwrap_or_default()),
                    entry.id(),
                    entry.filemode(),
                ));
            }
            TreeWalkResult::Ok
        })
        .map_err(|_| Error::Invalid("Cannot inspect committed source tree"))?;
        let database = repo
            .odb()
            .map_err(|_| Error::Invalid("Cannot open committed source objects"))?;
        let mut media = None;
        let mut attributes = None;
        for (name, oid, mode) in entries {
            runner.cancel.check()?;
            safe_path(Path::new(&name))?;
            let path = prefix.join(&name);
            if mode == 0o160_000 {
                let local = directory.join(&name);
                self.modules.push(Module {
                    path: path.to_string_lossy().into_owned(),
                    revision: oid.to_string(),
                    repository: local.clone(),
                });
                self.visit(&local, &oid.to_string(), &path, depth + 1, runner)?;
            } else if mode == 0o100_644 || mode == 0o100_755 {
                if database
                    .read_header(oid)
                    .map_err(|_| Error::Invalid("Missing source object"))?
                    .0
                    > 1024
                {
                    continue;
                }
                let blob = repo
                    .find_blob(oid)
                    .map_err(|_| Error::Invalid("Missing committed source object"))?;
                if !blob
                    .content()
                    .starts_with(b"version https://git-lfs.github.com/spec/v1\n")
                {
                    continue;
                }
                if attributes.is_none() {
                    attributes = Some(super::attributes::Attributes::new(&repo, &tree)?);
                }
                if !attributes
                    .as_ref()
                    .ok_or(Error::Invalid("Missing committed attributes"))?
                    .is_lfs(Path::new(&name))?
                {
                    continue;
                }
                let Some((oid, size)) = pointer(blob.content())? else {
                    continue;
                };
                if media.is_none() {
                    media = Some(media_directory(directory, runner)?);
                }
                let storage = media.as_ref().ok_or(Error::Invalid("Missing LFS storage"))?;
                let source = storage.join(&oid[..2]).join(&oid[2..4]).join(&oid);
                verify_object(&source, &oid, size, runner)?;
                self.assets.push(Asset {
                    path: path.to_string_lossy().into_owned(),
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
    if bytes.len() > 1024 || !bytes.starts_with(b"version https://git-lfs.github.com/spec/v1\n") {
        return Ok(None);
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
    let mut file = std::fs::File::open(path)
        .map_err(|_| Error::Invalid("Fetch selected Git LFS objects locally before deploying"))?;
    if file.metadata()?.len() != size {
        return Err(Error::Invalid("Local Git LFS object size mismatch"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    let mut completed = 0;
    let mut reported = Instant::now();
    let progress = |completed| {
        (runner.emit)(Event::Progress(Progress {
            detail: "Verifying current source asset".into(),
            completed,
            total: Some(size),
            transferred: Some(completed),
            ..Progress::default()
        }));
    };
    progress(0);
    loop {
        runner.cancel.check()?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        completed += n as u64;
        if reported.elapsed() >= Duration::from_millis(250) {
            progress(completed);
            reported = Instant::now();
        }
    }
    progress(completed);
    let mut digest = String::with_capacity(64);
    for byte in hash.finalize() {
        use std::fmt::Write as _;
        let _ = write!(digest, "{byte:02x}");
    }
    if digest != oid {
        return Err(Error::Invalid("Local Git LFS object checksum mismatch"));
    }
    Ok(())
}

pub(super) fn archive(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    let material = Material::collect(repository, revision, runner)?;
    let directory = root.join("material");
    std::fs::create_dir(&directory)?;
    let objects = directory.join("lfs");
    std::fs::create_dir(&objects)?;
    for asset in &material.assets {
        let path = objects.join(&asset.oid);
        if !path.exists() {
            std::fs::copy(&asset.source, &path)?;
            verify_object(&path, &asset.oid, asset.size, runner)?;
        }
    }
    for (index, module) in material.modules.iter().enumerate() {
        super::pack(
            &module.repository,
            &module.revision,
            &directory.join(format!("module-{index}.pack")),
            runner,
        )?;
    }
    std::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&material).map_err(|_| Error::Json)?,
    )?;
    let archive = root.join("source-material.tar");
    runner.run(
        "Pack source dependencies",
        Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(directory)
            .arg("."),
        Duration::from_secs(300),
    )?;
    Ok(archive)
}
