//! Private dependency packages a repository restores on this computer before a worker is
//! allocated. The restore runs with the owner's own credentials, so no package credential
//! reaches the worker, which receives only the restored folder with the source.
use super::{Error, Event, Result, command::Runner, progress::Progress};
use horizon_cloud::Packages;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// How long a restore may run.
const TIMEOUT: Duration = Duration::from_mins(30);
const MAX_ENTRIES: usize = 500_000;
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Package manager settings files, which hold feed credentials rather than packages.
const CREDENTIAL_FILES: [&str; 9] = [
    "nuget.config",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pypirc",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".dockercfg",
];

/// The owner's permission for one local checkout to run exactly this restore command on
/// this computer. Machine-local, like Git credential bindings: a repository's
/// configuration can ask for a restore but never allow one, and neither can an agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub local_repository: PathBuf,
    pub restore: Vec<String>,
    pub env: String,
}

impl Approval {
    #[must_use]
    pub fn new(local_repository: PathBuf, packages: &Packages) -> Self {
        Self {
            local_repository,
            restore: packages.restore.clone(),
            env: packages.env.clone(),
        }
    }

    /// # Errors
    /// Rejects a relative checkout path.
    pub fn validate(&self) -> Result<()> {
        if !self.local_repository.is_absolute() {
            return Err(Error::Invalid(
                "Package restore approvals require an absolute machine-local checkout path",
            ));
        }
        Ok(())
    }

    /// Whether this approval allows `packages` for the checkout at `repository`.
    #[must_use]
    pub fn allows(&self, repository: &Path, packages: &Packages) -> bool {
        let same_checkout = self.local_repository == repository
            || matches!(
                (self.local_repository.canonicalize(), repository.canonicalize()),
                (Ok(approved), Ok(requested)) if approved == requested
            );
        same_checkout && self.restore == packages.restore && self.env == packages.env
    }
}

/// One folder or file of a restored package folder, by its portable relative path.
pub(crate) struct Entry {
    pub name: String,
    pub path: PathBuf,
    /// `None` for a folder.
    pub size: Option<u64>,
    pub executable: bool,
}

/// A restored package folder, checked and listed for the source archive.
pub(crate) struct Restored {
    pub env: String,
    pub entries: Vec<Entry>,
    pub bytes: u64,
}

impl Restored {
    pub(crate) fn files(&self) -> usize {
        self.entries.iter().filter(|entry| entry.size.is_some()).count()
    }
}

/// Runs `packages`' restore for the committed tree of `repository` at `revision` into a
/// new folder under `root`, once an approval allows it. Nothing else of this computer's
/// checkout is visible to the command: it runs in an export of the committed tree,
/// without submodules, and LFS content stays as pointers.
///
/// # Errors
/// Refuses an unapproved restore, a failed or timed out command, and a folder holding a
/// link, a credential file or more than the transfer limits allow.
pub(crate) fn restore(
    repository: &Path,
    revision: &str,
    packages: &Packages,
    approvals: &[Approval],
    root: &Path,
    runner: &Runner<'_>,
) -> Result<Restored> {
    if !approvals.iter().any(|approval| approval.allows(repository, packages)) {
        return Err(Error::PackageRestoreNotApproved);
    }
    (runner.emit)(Event::Progress(Progress::activity("Restore packages on this computer")));
    let tree = root.join("package-restore-tree");
    std::fs::create_dir(&tree)?;
    super::repository::export_tree(repository, revision, &tree, runner)?;
    let directory = root.join("packages");
    std::fs::create_dir(&directory)?;
    let folder = directory
        .to_str()
        .ok_or(Error::Invalid("The package folder path is not valid Unicode"))?;
    let arguments = packages.arguments(folder);
    let mut command = Command::new(program(&tree, &arguments[0]));
    command.args(&arguments[1..]).current_dir(&tree);
    let restored = runner.run("Package restore", &mut command, TIMEOUT);
    let _ = std::fs::remove_dir_all(&tree);
    restored?;
    let (entries, bytes) = scan(&directory)?;
    Ok(Restored {
        env: packages.env.clone(),
        entries,
        bytes,
    })
}

/// A program named by a relative path, such as `./restore.sh`, is the committed one.
fn program(tree: &Path, program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.is_relative() && path.components().count() > 1 {
        tree.join(path)
    } else {
        path.to_owned()
    }
}

/// Lists `directory` in a stable order, refusing what a package folder must not send.
fn scan(directory: &Path) -> Result<(Vec<Entry>, u64)> {
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    let mut pending = vec![(directory.to_owned(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        let mut children = std::fs::read_dir(&folder)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children.into_iter().rev() {
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| Error::Invalid("A restored package path is not valid Unicode"))?;
            if CREDENTIAL_FILES.contains(&name.to_ascii_lowercase().as_str()) {
                return Err(Error::Invalid(
                    "The restored package folder holds a package manager settings file, which can hold feed credentials; restore only packages into {dir}",
                ));
            }
            let relative = format!("{prefix}{name}");
            let metadata = std::fs::symlink_metadata(child.path())?;
            if entries.len() >= MAX_ENTRIES {
                return Err(Error::Invalid(
                    "The restored package folder holds more than 500,000 files and folders",
                ));
            }
            if metadata.is_dir() {
                pending.push((child.path(), format!("{relative}/")));
                entries.push(Entry {
                    name: relative,
                    path: child.path(),
                    size: None,
                    executable: false,
                });
            } else if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_BYTES)
                    .ok_or(Error::Invalid("The restored package folder exceeds 32 GiB"))?;
                entries.push(Entry {
                    name: relative,
                    path: child.path(),
                    size: Some(metadata.len()),
                    executable: executable(&metadata),
                });
            } else {
                return Err(Error::Invalid(
                    "The restored package folder holds a link or special file; only files and folders are sent",
                ));
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((entries, bytes))
}

#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests;
