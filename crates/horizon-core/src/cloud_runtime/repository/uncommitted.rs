//! The uncommitted files a rebuild lays over its committed snapshot, so that the image
//! builds from the working copy: tracked files with their working-tree content, and
//! untracked files that Git does not ignore, inside the recipe's build context and its
//! Dockerfile. Git-ignored files never go in, and `.dockerignore` still applies when
//! Docker reads the context. The application source a worker gets stays the committed
//! archive.
//!
//! An image can be pushed to a public registry, so a person confirms this list before the
//! rebuild; its [`Uncommitted::hash`] covers each file's content, so a file that changes
//! after the confirmation refuses the build. The names of common secret files are refused
//! even when Git does not ignore them.
//!
//! The checkout is read with git2, which runs no program that a repository's configuration
//! names (a content filter or a file system monitor), and no link is followed: a link goes
//! into the context as a link, a file is never read through a folder that is a link, and a
//! hard link is refused (on Unix; Windows does not report hard links here). Submodules
//! come from their commits, and untracked nested repositories are left out.
mod overlay;
#[cfg(test)]
mod tests;

use super::{Error, Result};
use git2::{Delta, DiffOptions, Oid, Repository};
use horizon_cloud::Build;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path},
};

pub use overlay::overlay;

const UNREADABLE: &str = "Cannot read the uncommitted changes of the repository's working copy";
const MOVED: &str = "The working copy changed while it was read; rebuild again";
const OTHER_WORKDIR: &str = "The repository's Git configuration names another working folder (core.worktree), so its uncommitted files cannot be read safely; remove that setting before you rebuild";

/// An uncommitted file of the build context that a rebuild does not take, named by its
/// path from the repository root, with what to do about it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(
        "Uncommitted file {0} in the image's build context has the name of a common secret file, and the image can be pushed to a public registry. A rebuild leaves out files that Git ignores: add it to .gitignore, or commit or remove the change, before you rebuild"
    )]
    Secret(String),
    #[error(
        "Uncommitted file {0:?} in the image's build context has a name that is not UTF-8 or holds a control or text-direction character, which a list can show wrongly; rename it, commit it or add it to .gitignore before you rebuild"
    )]
    Name(String),
    #[error(
        "A folder on the way to uncommitted file {0} in the image's build context is a link, and a rebuild does not follow links; replace it with a folder or add it to .gitignore before you rebuild"
    )]
    LinkedFolder(String),
    #[error(
        "Uncommitted file {0} in the image's build context is a hard link, which can share its content with a file outside the repository; replace it with a copy or add it to .gitignore before you rebuild"
    )]
    HardLink(String),
    #[error(
        "Uncommitted entry {0} in the image's build context is not a file, a folder or a link; remove it or add it to .gitignore before you rebuild"
    )]
    Special(String),
}

/// How a file in the build context differs from the commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Modified,
    New,
    Deleted,
}

impl Change {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::New => "new",
            Self::Deleted => "deleted",
        }
    }
}

/// What a changed path holds in the working copy, with the SHA-256 of its content or link
/// target, so a confirmation binds the content and not only the name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    File { executable: bool, digest: String },
    Link { digest: String },
    Absent,
}

/// One uncommitted file of the build context, by its path from the repository root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub change: Change,
    content: Content,
}

/// The uncommitted files of a build context, sorted by path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uncommitted {
    files: Vec<File>,
}

impl Uncommitted {
    #[must_use]
    pub fn files(&self) -> &[File] {
        &self.files
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The SHA-256 of every path, change, kind and content digest, as 64 lowercase
    /// hexadecimal digits; `None` when nothing is uncommitted. Equal files give an equal
    /// hash, so a cached build of the same files keeps its image digest.
    #[must_use]
    pub fn hash(&self) -> Option<String> {
        if self.files.is_empty() {
            return None;
        }
        let mut hasher = Sha256::new();
        for file in &self.files {
            let (kind, digest) = match &file.content {
                Content::File {
                    executable: true,
                    digest,
                } => ("executable", digest.as_str()),
                Content::File { digest, .. } => ("file", digest.as_str()),
                Content::Link { digest } => ("link", digest.as_str()),
                Content::Absent => ("absent", ""),
            };
            for field in [file.path.as_str(), file.change.label(), kind, digest] {
                hasher.update(field.as_bytes());
                hasher.update([0]);
            }
        }
        Some(hex(&hasher.finalize()))
    }
}

/// Whether `hash` has the form [`Uncommitted::hash`] gives.
#[must_use]
pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The uncommitted files of `repository`'s working copy against `revision` that the image
/// recipe `build` reads: those inside its context folder, and its Dockerfile.
/// # Errors
/// Refuses a name that is not UTF-8, holds a control character or names a common secret
/// file, a file reached through a folder that is a link, a hard link and an entry that is
/// not a file or a link; reports a working copy git2 cannot read.
pub fn collect(repository: &Path, revision: &str, build: &Build) -> Result<Uncommitted> {
    let scope = Scope::new(repository, build);
    let mut changes = BTreeMap::new();
    let opened = Repository::open(repository).map_err(|_| Error::Invalid(UNREADABLE))?;
    // The configuration can name another working folder (`core.worktree`); then Git's view
    // and the files read here would differ.
    let same = opened
        .workdir()
        .is_some_and(|workdir| workdir.canonicalize().ok() == repository.canonicalize().ok());
    if !same {
        return Err(Error::Invalid(OTHER_WORKDIR));
    }
    for (path, change) in deltas(&opened, revision, &scope)? {
        // Out of the index but still on disk, git2 reports only the deletion; the file is
        // untracked then, so it counts only while Git does not ignore it.
        let change = if change == Change::Deleted
            && overlay::on_disk(repository, &path)
            && !opened.is_path_ignored(&path).map_err(|_| Error::Invalid(UNREADABLE))?
        {
            Change::Modified
        } else {
            change
        };
        changes
            .entry(path)
            .and_modify(|known| {
                // Listed as deleted and as new: the commit's path has new content.
                if *known != change {
                    *known = Change::Modified;
                }
            })
            .or_insert(change);
    }
    let files = changes
        .into_iter()
        .map(|(path, change)| {
            refuse_unclear(&path)?;
            let content = if change == Change::Deleted {
                Content::Absent
            } else {
                refuse_secret(&path)?;
                overlay::content(repository, &path)?
            };
            Ok(File { path, change, content })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Uncommitted { files })
}

/// Each path in `scope` that differs between `revision` and the working copy, as `git diff
/// <revision>` reads it through the index, with untracked files that are not ignored.
/// Submodules come from the commit and are left out.
fn deltas(opened: &Repository, revision: &str, scope: &Scope) -> Result<Vec<(String, Change)>> {
    let invalid = |_: git2::Error| Error::Invalid(UNREADABLE);
    let commit = Oid::from_str(revision)
        .and_then(|oid| opened.find_commit(oid))
        .map_err(invalid)?;
    let tree = commit.tree().map_err(invalid)?;
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .include_typechange(true)
        .ignore_submodules(true);
    let diff = opened
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut options))
        .map_err(invalid)?;
    diff.deltas()
        .filter_map(|delta| {
            let change = match delta.status() {
                Delta::Added | Delta::Untracked => Change::New,
                Delta::Deleted => Change::Deleted,
                Delta::Modified | Delta::Typechange | Delta::Conflicted => Change::Modified,
                _ => return None,
            };
            let file = if change == Change::Deleted {
                delta.old_file()
            } else {
                delta.new_file()
            };
            let path = file.path_bytes()?;
            // An untracked nested repository is listed as its folder; like a submodule, it
            // is not part of this repository's build.
            // A name outside the scope never refuses the rebuild, whatever its bytes.
            if path.ends_with(b"/") || !scope.covers(&String::from_utf8_lossy(path)) {
                return None;
            }
            Some(
                std::str::from_utf8(path)
                    .map(|path| (path.to_owned(), change))
                    .map_err(|_| Refusal::Name(String::from_utf8_lossy(path).into_owned()).into()),
            )
        })
        .collect()
}

/// The part of the checkout a recipe's build reads: its context folder and its
/// Dockerfile, which may lie outside the context.
struct Scope {
    /// `None` when the context is not a plain relative folder, so all of the checkout counts.
    context: Option<Vec<String>>,
    dockerfile: Option<Vec<String>>,
}

impl Scope {
    /// The build resolves its paths through links (`image::contained`), so a build path
    /// that passes a link in the working copy, or one that is not plain, counts the whole
    /// checkout: a narrower scope could leave out a file the build reads.
    fn new(repository: &Path, build: &Build) -> Self {
        let context = parts(&build.context);
        let dockerfile = parts(&build.dockerfile);
        let linked = |parts: &Option<Vec<String>>| {
            parts.as_ref().is_none_or(|parts| {
                let mut path = repository.to_path_buf();
                parts.iter().any(|part| {
                    path.push(part);
                    fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
                })
            })
        };
        if linked(&context) || linked(&dockerfile) {
            return Self {
                context: None,
                dockerfile,
            };
        }
        Self { context, dockerfile }
    }

    /// Whether `path` lies in the context or is the Dockerfile. Names compare without
    /// case, so a build path spelled with other case on a checkout that ignores case still
    /// matches; on a checkout that does not, this only lists more.
    fn covers(&self, path: &str) -> bool {
        let starts = |prefix: &[String]| {
            let mut parts = path.split('/');
            prefix.iter().all(|part| {
                parts
                    .next()
                    .is_some_and(|name| name.to_lowercase() == part.to_lowercase())
            })
        };
        self.context.as_deref().is_none_or(starts)
            || self
                .dockerfile
                .as_deref()
                .is_some_and(|dockerfile| starts(dockerfile) && path.split('/').count() == dockerfile.len())
    }
}

/// The parts of the build path `value` from the repository root; `None` when it is not a
/// plain relative path. A `..` counts as not plain: the build follows a link before it
/// applies `..` (`image::contained`), so no lexical reading can say where it lands.
fn parts(value: &str) -> Option<Vec<String>> {
    Path::new(value)
        .components()
        .filter(|component| *component != Component::CurDir)
        .map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

/// Refuses a name that a list could show as something else.
fn refuse_unclear(path: &str) -> Result<()> {
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{fffd}'))
    {
        return Err(Refusal::Name(path.to_owned()).into());
    }
    Ok(())
}

/// Refuses the name of a common secret file: an image can be pushed to a public registry,
/// and an untracked secret that is not in `.gitignore` is an easy mistake to make.
fn refuse_secret(path: &str) -> Result<()> {
    if secret_name(path) {
        return Err(Refusal::Secret(path.to_owned()).into());
    }
    Ok(())
}

/// `.env`, `.env.*` and `*.env` but not an example of one, private key and certificate stores,
/// SSH private keys, password stores, Terraform variables and state, and the credential
/// files of common tools.
fn secret_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ends = |suffixes: &[&str]| suffixes.iter().any(|suffix| name.ends_with(suffix));
    let environment = (name == ".env" || name.starts_with(".env.") || ends(&[".env"]))
        && !ends(&[".example", ".sample", ".template", ".dist"]);
    let key_store = ends(&[".pem", ".key", ".p12", ".p8", ".pfx", ".jks", ".keystore"]);
    let ssh_key = ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].iter().any(|key| {
        let variant = name
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(['_', '-', '.']))
            .is_some();
        name == *key || variant && !ends(&[".pub"])
    });
    let credentials = [
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        ".envrc",
        ".htpasswd",
        ".pgpass",
    ]
    .contains(&name.as_str())
        || ends(&[".kdbx", ".tfvars", ".tfstate"]);
    let lower = path.to_ascii_lowercase();
    let tool_logins = [".aws/credentials", ".docker/config.json"]
        .iter()
        .any(|login| lower == *login || lower.ends_with(&format!("/{login}")));
    environment || key_store || ssh_key || credentials || tool_logins
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}
